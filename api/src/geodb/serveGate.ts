/**
 * The serve gate shared by the governance and versions families (GEODB_GOVERNANCE_SERVE, GEODB_VERSIONS_SERVE; both off
 * by default).
 *
 * A shadow compares a Postgres answer with geo-db's after the response is built. A serve switch lets a family answer
 * from geo-db instead, but only for a key whose shadow comparison last matched. This file holds the three pieces every
 * family needs: the switch (`readServeGateConfig`), the memory of each key's last comparison (`createMatchLedger`) and
 * the counters (`gaia_api_geodb_serve_total{family,read}`, `gaia_api_geodb_serve_fallback_total{family,read,reason}`).
 *
 * Serving never fails a request: every path that cannot answer from geo-db returns null and the caller keeps its
 * Postgres answer. The counters stay empty until a family's switch is on, so a deploy with both off exports nothing.
 */

export const SERVE_FAMILIES = ["governance", "versions"] as const
export type ServeFamily = (typeof SERVE_FAMILIES)[number]

/**
 * Why a read was answered by Postgres although its family's switch is on. A closed set, safe as a label.
 * - shadow_off: the family's shadow is not running, so no comparison exists to trust.
 * - no_comparison: the shadow has not compared this key (it samples), or the entry was evicted.
 * - mismatch: the last comparison for the key differed.
 * - unsupported: the last comparison could not be made (geo-db cannot answer that key).
 * - stale: the last comparison matched but is older than the match TTL.
 * - timeout / error: the geo-db read made to serve failed.
 * - differs: the geo-db read made to serve returned something Postgres's shape cannot take (an id geo-db remapped).
 */
export const SERVE_FALLBACK_REASONS = [
	"shadow_off",
	"no_comparison",
	"mismatch",
	"unsupported",
	"stale",
	"timeout",
	"error",
	"differs",
] as const
export type ServeFallbackReason = (typeof SERVE_FALLBACK_REASONS)[number]

export type ServeGateConfig = {
	enabled: boolean
	/** How long a matched comparison keeps a key servable (GEODB_<FAMILY>_SERVE_MATCH_TTL_MS). */
	matchTtlMs: number
	/** Longest the geo-db read made to serve may take before the request goes to Postgres (GEODB_<FAMILY>_SERVE_TIMEOUT_MS). */
	timeoutMs: number
}

const DEFAULTS = {matchTtlMs: 5 * 60_000, timeoutMs: 500}
const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readServeGateConfig(env: Record<string, string | undefined>, family: ServeFamily): ServeGateConfig {
	const name = `GEODB_${family.toUpperCase()}_SERVE`
	const flag = env[name]?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		matchTtlMs: wholeNumber(env[`${name}_MATCH_TTL_MS`], DEFAULTS.matchTtlMs, 24 * 3_600_000),
		timeoutMs: wholeNumber(env[`${name}_TIMEOUT_MS`], DEFAULTS.timeoutMs, 30_000),
	}
}

export type Comparison = "match" | "mismatch" | "unsupported"

export type MatchLedger<V = undefined> = {
	/** Remember the outcome of the latest comparison for a key (and what the matching side held, when the family needs it). */
	record(key: string, outcome: Comparison, detail?: V): void
	/** The key's last comparison when it may be served, else why not. */
	check(key: string): {ok: true; detail: V | undefined} | {ok: false; reason: ServeFallbackReason}
}

/** A bounded memory of each key's last comparison: the oldest key is dropped first. */
export function createMatchLedger<V = undefined>(opts: {
	maxKeys?: number
	ttlMs: number
	now?: () => number
}): MatchLedger<V> {
	const maxKeys = opts.maxKeys ?? 10_000
	const now = opts.now ?? Date.now
	const entries = new Map<string, {outcome: Comparison; at: number; detail: V | undefined}>()
	return {
		record(key, outcome, detail) {
			entries.delete(key)
			entries.set(key, {outcome, at: now(), detail})
			if (entries.size > maxKeys) {
				const oldest = entries.keys().next().value
				if (oldest !== undefined) entries.delete(oldest)
			}
		},
		check(key) {
			const e = entries.get(key)
			if (!e) return {ok: false, reason: "no_comparison"}
			if (e.outcome === "mismatch") return {ok: false, reason: "mismatch"}
			if (e.outcome === "unsupported") return {ok: false, reason: "unsupported"}
			if (now() - e.at > opts.ttlMs) return {ok: false, reason: "stale"}
			return {ok: true, detail: e.detail}
		},
	}
}

/** Rejects with `Timeout` when the read outlasts `ms`; the read itself is not cancelled. */
export class ServeTimeout extends Error {
	constructor() {
		super("serve read timed out")
	}
}
export function withServeTimeout<T>(read: Promise<T>, ms: number): Promise<T> {
	return new Promise<T>((resolve, reject) => {
		const timer = setTimeout(() => reject(new ServeTimeout()), ms)
		read.then(
			(v) => {
				clearTimeout(timer)
				resolve(v)
			},
			(e) => {
				clearTimeout(timer)
				reject(e)
			},
		)
	})
}

/** `timeout` for a slow read, `error` for anything else a read threw. */
export const failureReason = (error: unknown): "timeout" | "error" =>
	error instanceof ServeTimeout || (error instanceof Error && /timed? ?out/i.test(error.message))
		? "timeout"
		: "error"

const activeFamilies = new Set<ServeFamily>()
const served = new Map<string, number>()
const fallbacks = new Map<string, number>()
const bump = (map: Map<string, number>, key: string) => map.set(key, (map.get(key) ?? 0) + 1)
const reads = new Map<ServeFamily, Set<string>>()

/** Register a family whose switch is on, and the reads it can serve, so every series exists at zero. */
export function activateServeGate(family: ServeFamily, readNames: readonly string[]): void {
	activeFamilies.add(family)
	const set = reads.get(family) ?? new Set<string>()
	for (const r of readNames) set.add(r)
	reads.set(family, set)
}

export function recordServed(family: ServeFamily, read: string): void {
	bump(served, `${family}\t${read}`)
}

export function recordServeFallback(family: ServeFamily, read: string, reason: ServeFallbackReason): void {
	bump(fallbacks, `${family}\t${read}\t${reason}`)
}

export function renderServeGateMetrics(): string {
	if (activeFamilies.size === 0) return ""
	const lines = [
		"# HELP gaia_api_geodb_serve_total Reads of a family whose serve switch is on that were answered from geo-db, by family and read.",
		"# TYPE gaia_api_geodb_serve_total counter",
	]
	for (const family of activeFamilies)
		for (const read of reads.get(family) ?? [])
			lines.push(
				`gaia_api_geodb_serve_total{family="${family}",read="${read}"} ${served.get(`${family}\t${read}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_geodb_serve_fallback_total Reads of a family whose serve switch is on that Postgres answered instead, by family, read and reason.",
		"# TYPE gaia_api_geodb_serve_fallback_total counter",
	)
	for (const family of activeFamilies)
		for (const read of reads.get(family) ?? [])
			for (const reason of SERVE_FALLBACK_REASONS)
				lines.push(
					`gaia_api_geodb_serve_fallback_total{family="${family}",read="${read}",reason="${reason}"} ${fallbacks.get(`${family}\t${read}\t${reason}`) ?? 0}`,
				)
	return lines.join("\n")
}

/** Reset accumulated state. Tests only. */
export function __resetServeGateForTests(): void {
	activeFamilies.clear()
	reads.clear()
	served.clear()
	fallbacks.clear()
}
