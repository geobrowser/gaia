/**
 * The account_weights shadow (GEODB_ACCOUNT_WEIGHTS_SHADOW, off by default).
 *
 * `readAccounts` (pairFit/queries.ts) serves each pair-fit candidate's `account_weights.weight` from Postgres. geo-db holds
 * the same table in derived collection 5 (ranking-indexer's account_weights_mirror, GEODB_DERIVED_MIRROR): group 0, key the
 * 16-byte user id, score the weight. This compares the two for a sample of the candidates of one request at most every
 * `intervalMs`, in the background, after the response's data is read; it never changes, delays or fails a response.
 * `gaia_api_account_weights_shadow_total{result}` counts candidates: match (same weight, or no row on both sides), mismatch
 * (a different weight, or a geo-db row Postgres lacks), unsupported (geo-db has no row for an account Postgres has: the
 * mirror has not run or lags; or the row's layout is not the one this reads; or the shadow was busy) or failed (the read
 * failed).
 *
 * With GEODB_ACCOUNT_EXCLUSIONS_SHADOW also on, each sampled candidate's `excluded` (a row in `account_exclusions`) is compared
 * with collection 12 (ranking-indexer's account_exclusions_mirror): held there means excluded. Counted in
 * `gaia_api_account_exclusions_shadow_total{result}`: match, mismatch (a row on one side only, where geo-db has the row or
 * Postgres lacks it), unsupported (Postgres has the row and geo-db does not: the mirror has not run or lags; or the value is
 * not 9 bytes) or failed. Only the exclusion list is compared: the weights row's reason bit 1 is the refresh's own flag and
 * trails the list by up to an hour.
 */

import {GeoDbError} from "../geodb/client"
import type {DerivedReader} from "../geodb/derived"

export type AccountWeightsShadowConfig = {
	enabled: boolean
	maxInFlight: number
	intervalMs: number
	sample: number
	/** Also compare `excluded` against collection 12. Needs `enabled`. */
	exclusions: boolean
}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readAccountWeightsShadowConfig(env: Record<string, string | undefined>): AccountWeightsShadowConfig {
	const flag = env.GEODB_ACCOUNT_WEIGHTS_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		maxInFlight: wholeNumber(env.GEODB_ACCOUNT_WEIGHTS_SHADOW_MAX_IN_FLIGHT, 1, 8),
		intervalMs: wholeNumber(env.GEODB_ACCOUNT_WEIGHTS_SHADOW_INTERVAL_MS, 10_000, 3_600_000),
		sample: wholeNumber(env.GEODB_ACCOUNT_WEIGHTS_SHADOW_SAMPLE, 5, 50),
		exclusions: ["1", "on", "true"].includes(env.GEODB_ACCOUNT_EXCLUSIONS_SHADOW?.trim().toLowerCase() ?? ""),
	}
}

/** geo-db's registry slot for account_weights. */
export const ACCOUNT_WEIGHTS_COLLECTION = 5
/** The mirror's value: vote_count i32, stance_vote_count i32, first_vote_at i64, computed_at i64, reason bits u32. */
export const ACCOUNT_WEIGHTS_VALUE_BYTES = 28
/** geo-db's registry slot for account_exclusions. */
export const ACCOUNT_EXCLUSIONS_COLLECTION = 12
/** The mirror's value: source tag u8, synced_at i64. */
export const ACCOUNT_EXCLUSIONS_VALUE_BYTES = 9
/** Scores are f64 on both sides; Postgres's numeric text round trip may differ in the last digits. */
const WEIGHT_TOLERANCE = 1e-9

export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
export const UNSUPPORTED_REASONS = ["row_missing", "bad_layout", "busy"] as const
type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]

let active = false
let exclusionsActive = false
const exclusionResults = new Map<string, number>()
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const bump = (map: Map<string, number>, key: string): void => {
	map.set(key, (map.get(key) ?? 0) + 1)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderAccountWeightsMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_account_weights_shadow_total Pair-fit candidates' account weight from geo-db's derived collection 5 against the one served from Postgres, by result: match, mismatch, unsupported (geo-db cannot answer) or failed (geo-db read failed).",
		"# TYPE gaia_api_account_weights_shadow_total counter",
	]
	for (const r of RESULTS) lines.push(`gaia_api_account_weights_shadow_total{result="${r}"} ${results.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_account_weights_shadow_unsupported_total account weight comparisons geo-db could not answer, by reason.",
		"# TYPE gaia_api_account_weights_shadow_unsupported_total counter",
	)
	for (const r of UNSUPPORTED_REASONS)
		lines.push(`gaia_api_account_weights_shadow_unsupported_total{reason="${r}"} ${unsupported.get(r) ?? 0}`)
	if (exclusionsActive) {
		lines.push(
			"# HELP gaia_api_account_exclusions_shadow_total Pair-fit candidates' exclusion (a row in account_exclusions) from geo-db's derived collection 12 against Postgres, by result: match, mismatch, unsupported (geo-db cannot answer) or failed (geo-db read failed).",
			"# TYPE gaia_api_account_exclusions_shadow_total counter",
		)
		for (const r of RESULTS)
			lines.push(`gaia_api_account_exclusions_shadow_total{result="${r}"} ${exclusionResults.get(r) ?? 0}`)
	}
	return lines.join("\n")
}

export function __resetAccountWeightsMetricsForTests(): void {
	active = false
	exclusionsActive = false
	exclusionResults.clear()
	results.clear()
	unsupported.clear()
}

/** What the served weight and geo-db's row say about each other: "match", "mismatch" or an unsupported reason. Pure. */
export function compareAccountWeight(
	served: number | null,
	held: {score: number; value: Buffer} | null,
): "match" | "mismatch" | UnsupportedReason {
	if (held === null) return served === null ? "match" : "row_missing"
	if (held.value.length !== ACCOUNT_WEIGHTS_VALUE_BYTES) return "bad_layout"
	if (served === null) return "mismatch"
	return Math.abs(served - held.score) <= WEIGHT_TOLERANCE * Math.max(1, Math.abs(served)) ? "match" : "mismatch"
}

/** What the served `excluded` flag and geo-db's collection 12 row say about each other. Pure. */
export function compareAccountExclusion(
	served: boolean,
	held: {score: number; value: Buffer} | null,
): "match" | "mismatch" | "unsupported" {
	if (held === null) return served ? "unsupported" : "match"
	if (held.value.length !== ACCOUNT_EXCLUSIONS_VALUE_BYTES) return "unsupported"
	return served ? "match" : "mismatch"
}

export type AccountWeightsShadow = {
	/** `accounts` is what `readAccounts` served, keyed by 32-hex user id. Returns at once; the comparison runs in the background. */
	compare(accounts: Map<string, {weight: number | null; excluded?: boolean}>): void
}

export function createAccountWeightsShadow(deps: {
	config: AccountWeightsShadowConfig
	reader: DerivedReader
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): AccountWeightsShadow {
	const {config, reader, log} = deps
	const now = deps.now ?? Date.now
	active = true
	if (config.exclusions) exclusionsActive = true
	let inFlight = 0
	let lastAt = Number.NEGATIVE_INFINITY
	let logged = 0

	async function run(sampled: [string, number | null, boolean][]) {
		for (const [userId, served, servedExcluded] of sampled) {
			try {
				const held = await reader.get(ACCOUNT_WEIGHTS_COLLECTION, userId)
				const outcome = compareAccountWeight(served, held)
				if (outcome === "match" || outcome === "mismatch") bump(results, outcome)
				else {
					bump(results, "unsupported")
					bump(unsupported, outcome)
				}
				// Counts only: never who.
				if (outcome === "mismatch" && logged < 20) {
					logged++
					log.warn("account weights shadow mismatch", {servedNull: served === null})
				}
			} catch (error) {
				bump(results, "failed")
				if (!(error instanceof GeoDbError)) log.warn("account weights shadow failed", {error: String(error)})
			}
			if (!config.exclusions) continue
			try {
				const held = await reader.get(ACCOUNT_EXCLUSIONS_COLLECTION, userId)
				const outcome = compareAccountExclusion(servedExcluded, held)
				bump(exclusionResults, outcome)
				if (outcome === "mismatch" && logged < 20) {
					logged++
					log.warn("account exclusions shadow mismatch", {servedExcluded})
				}
			} catch (error) {
				bump(exclusionResults, "failed")
				if (!(error instanceof GeoDbError)) log.warn("account exclusions shadow failed", {error: String(error)})
			}
		}
	}

	return {
		compare(accounts) {
			if (!config.enabled) return
			const t = now()
			if (t - lastAt < config.intervalMs) return
			lastAt = t
			if (inFlight >= config.maxInFlight) {
				bump(results, "unsupported")
				bump(unsupported, "busy")
				return
			}
			const sampled: [string, number | null, boolean][] = []
			for (const [userId, a] of accounts) {
				if (sampled.length >= config.sample) break
				if (/^[0-9a-f]{32}$/.test(userId)) sampled.push([userId, a.weight, a.excluded === true])
			}
			if (sampled.length === 0) return
			inFlight++
			void run(sampled)
				.catch(() => {})
				.finally(() => {
					inFlight--
				})
		},
	}
}
