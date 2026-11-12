/**
 * The relation-lookup shadow (GEODB_RELATION_LOOKUP_SHADOW, off by default).
 *
 * A proposal diff resolves the relation ids of its update, delete and restore ops to their from-entities from Postgres
 * (`batchLookupRelationEntities` in versioned/proposal-diff.ts, the live `relations` table only). geo-db answers the same
 * question: its `entity` root with `.args.statement = "<relation id>"` returns the relation's from-entity, one id a request,
 * and an empty page when the relation is closed (geo-db RELATION-BY-ID-READ). When the flag is on, a sampled lookup sends
 * one such read per relation id, at most `concurrency` (8) at a time, and compares each answer with the map Postgres gave.
 * It runs after the Postgres map is built, fire-and-forget, never awaited by the request, and swallows every error: it never
 * changes, delays or fails a response. Metrics are counts only; the first 20 mismatches log counts, never an id.
 *
 * A closed relation is absent from Postgres's live table and empty in geo-db, so both agree on "nothing": a match. A
 * relation only one side holds is a mismatch (`missing_in_geodb`, `extra_in_geodb`); one both hold with different
 * from-entities is `from_entity`. Ids are compared after the loader's remap (`conformingId`).
 *
 * `gaia_api_relation_lookup_shadow_total{result}`: match, mismatch, unsupported or failed, one per sampled lookup.
 * `..._relations_total{outcome}`: match, mismatch or failed, one per relation id compared.
 * `..._mismatch_total{field}`, `..._unsupported_total{reason}` (client, too_many, busy), `..._skipped_total{reason}` (sample, rate).
 */

import {conformingId} from "./card"
import {type GeoDbCardClient, GeoDbError} from "./client"
import {
	activateServeGate,
	createMatchLedger,
	failureReason,
	recordServed,
	recordServeFallback,
	type ServeFallbackReason,
	type ServeGateConfig,
	withServeTimeout,
} from "./serveGate"

const compact = (id: string) => id.replaceAll("-", "").toLowerCase()

export type RelationLookupShadowConfig = {
	enabled: boolean
	sampleEvery: number
	maxPerMinute: number
	maxInFlight: number
	/** Reads in flight at once within one lookup. */
	concurrency: number
}

const DEFAULTS = {sampleEvery: 10, maxPerMinute: 30, maxInFlight: 2, concurrency: 8}
const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readRelationLookupShadowConfig(env: Record<string, string | undefined>): RelationLookupShadowConfig {
	const flag = env.GEODB_RELATION_LOOKUP_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		sampleEvery: wholeNumber(env.GEODB_RELATION_LOOKUP_SHADOW_SAMPLE_EVERY, DEFAULTS.sampleEvery, 100_000),
		maxPerMinute: wholeNumber(env.GEODB_RELATION_LOOKUP_SHADOW_MAX_PER_MINUTE, DEFAULTS.maxPerMinute, 6000),
		maxInFlight: wholeNumber(env.GEODB_RELATION_LOOKUP_SHADOW_MAX_IN_FLIGHT, DEFAULTS.maxInFlight, 16),
		concurrency: wholeNumber(env.GEODB_RELATION_LOOKUP_SHADOW_CONCURRENCY, DEFAULTS.concurrency, 16),
	}
}

/** Most relation ids compared in one lookup; a bigger proposal is counted `unsupported{too_many}`. */
export const MAX_RELATION_IDS = 64

export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
export const OUTCOMES = ["match", "mismatch", "failed"] as const
export const MISMATCH_FIELDS = ["from_entity", "missing_in_geodb", "extra_in_geodb"] as const
export const UNSUPPORTED_REASONS = ["client", "too_many", "busy"] as const
export const SKIP_REASONS = ["sample", "rate"] as const
type Result = (typeof RESULTS)[number]
type Outcome = (typeof OUTCOMES)[number]
type MismatchField = (typeof MISMATCH_FIELDS)[number]
type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]
type SkipReason = (typeof SKIP_REASONS)[number]

let active = false
const results = new Map<Result, number>()
const outcomes = new Map<Outcome, number>()
const mismatches = new Map<MismatchField, number>()
const unsupported = new Map<UnsupportedReason, number>()
const skipped = new Map<SkipReason, number>()
const bump = <K>(map: Map<K, number>, key: K, by = 1): void => {
	map.set(key, (map.get(key) ?? 0) + by)
}
const recordUnsupported = (reason: UnsupportedReason) => {
	bump(results, "unsupported")
	bump(unsupported, reason)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderRelationLookupShadowMetrics(): string {
	if (!active) return ""
	const lines: string[] = []
	const family = (name: string, help: string, label: string, keys: readonly string[], map: Map<string, number>) => {
		lines.push(`# HELP ${name} ${help}`, `# TYPE ${name} counter`)
		for (const k of keys) lines.push(`${name}{${label}="${k}"} ${map.get(k) ?? 0}`)
	}
	family(
		"gaia_api_relation_lookup_shadow_total",
		"Sampled relation-id to from-entity lookups served from Postgres, checked against geo-db's statement read, by result: match, mismatch, unsupported (geo-db cannot answer) or failed (a geo-db read failed).",
		"result",
		RESULTS,
		results,
	)
	family(
		"gaia_api_relation_lookup_shadow_relations_total",
		"Relation ids compared, by outcome: match (both agree, including both holding nothing for a closed relation), mismatch or failed.",
		"outcome",
		OUTCOMES,
		outcomes,
	)
	family(
		"gaia_api_relation_lookup_shadow_mismatch_total",
		"Relation lookup shadow mismatches by the field that differs.",
		"field",
		MISMATCH_FIELDS,
		mismatches,
	)
	family(
		"gaia_api_relation_lookup_shadow_unsupported_total",
		"Relation lookup shadow comparisons geo-db could not answer, by reason.",
		"reason",
		UNSUPPORTED_REASONS,
		unsupported,
	)
	family(
		"gaia_api_relation_lookup_shadow_skipped_total",
		"Eligible relation lookups not compared, by reason: sample (not the 1-in-N picked) or rate (over the per-minute cap).",
		"reason",
		SKIP_REASONS,
		skipped,
	)
	return lines.join("\n")
}

export function __resetRelationLookupShadowMetricsForTests(): void {
	active = false
	results.clear()
	outcomes.clear()
	mismatches.clear()
	unsupported.clear()
	skipped.clear()
}

export type RelationLookupShadow = {
	/** Compare Postgres's answer (`served`: relation id to from-entity id, absent for a closed relation) with geo-db, in the background. */
	lookup(relationIds: readonly string[], served: ReadonlyMap<string, string>): void
	/**
	 * GEODB_VERSIONS_SERVE: the same map from geo-db, when every relation id's last comparison matched. Null means ask Postgres
	 * (counted `gaia_api_geodb_serve_fallback_total{family="versions",read="relation_lookup"}`). Never rejects.
	 */
	serve(relationIds: readonly string[]): Promise<Map<string, string> | null>
}

/** The serve read's name in `gaia_api_geodb_serve_*{read}`. */
export const RELATION_LOOKUP_SERVE_READ = "relation_lookup"

/** A lookup server for when GEODB_VERSIONS_SERVE is on and GEODB_RELATION_LOOKUP_SHADOW is not: no comparison exists, so Postgres answers (`shadow_off`). */
export function createRelationLookupServeWithoutShadow(): (relationIds: readonly string[]) => Promise<null> {
	activateServeGate("versions", [RELATION_LOOKUP_SERVE_READ])
	return async () => {
		recordServeFallback("versions", RELATION_LOOKUP_SERVE_READ, "shadow_off")
		return null
	}
}

export function createRelationLookupShadow(deps: {
	config: RelationLookupShadowConfig
	geoDb: GeoDbCardClient
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
	/** GEODB_VERSIONS_SERVE; absent or disabled: nothing is served and no serve series exist. */
	serve?: ServeGateConfig
}): RelationLookupShadow {
	const {config, geoDb, log} = deps
	const now = deps.now ?? Date.now
	const serveConfig = deps.serve?.enabled ? deps.serve : null
	if (serveConfig) activateServeGate("versions", [RELATION_LOOKUP_SERVE_READ])
	/** Per relation id. `exact`: Postgres's from-entity id is the very id geo-db returns, not one `conformingId` remapped. False for a closed relation (nothing to compare), so a relation that opens later is not served. */
	const ledger = createMatchLedger<{exact: boolean}>({ttlMs: serveConfig?.matchTtlMs ?? 0, now})
	active = true
	let inFlight = 0
	let seen = 0
	let logged = 0
	let windowStart = now()
	let windowCount = 0

	async function compare(relationIds: string[], served: ReadonlyMap<string, string>): Promise<void> {
		const read = geoDb.readRelationEntity
		if (!read) return recordUnsupported("client")
		const geoOf = new Map<string, string | null>()
		let next = 0
		let busy = false
		const worker = async () => {
			while (next < relationIds.length && !busy) {
				const id = relationIds[next++] as string
				try {
					const rows = await read.call(geoDb, {relationId: conformingId("entity", id)})
					geoOf.set(id, rows[0]?.entity ?? null)
				} catch (error) {
					if (error instanceof GeoDbError && error.reason === "overloaded") busy = true
					else if (!(error instanceof GeoDbError))
						log.warn("relation lookup shadow failed", {error: String(error)})
				}
			}
		}
		await Promise.all(Array.from({length: Math.min(config.concurrency, relationIds.length)}, worker))
		if (busy) return recordUnsupported("busy")
		const pgOf = new Map<string, string>()
		for (const [relation, from] of served) pgOf.set(relation, conformingId("entity", from))
		let matched = 0
		let mismatched = 0
		let failed = 0
		for (const id of relationIds) {
			if (!geoOf.has(id)) {
				failed++
				if (serveConfig) ledger.record(id, "unsupported")
				continue
			}
			const geo = geoOf.get(id)
			const pg = pgOf.get(id)
			if (pg === (geo ?? undefined)) {
				matched++
				if (serveConfig) {
					const own = served.get(id)
					ledger.record(id, "match", {exact: own !== undefined && compact(own) === geo})
				}
			} else {
				mismatched++
				if (serveConfig) ledger.record(id, "mismatch")
				bump(mismatches, pg === undefined ? "extra_in_geodb" : geo == null ? "missing_in_geodb" : "from_entity")
			}
		}
		bump(outcomes, "match", matched)
		bump(outcomes, "mismatch", mismatched)
		bump(outcomes, "failed", failed)
		if (failed > 0) bump(results, "failed")
		else if (mismatched > 0) {
			bump(results, "mismatch")
			if (logged < 20) {
				logged++
				log.warn("relation lookup shadow mismatch", {relations: relationIds.length, matched, mismatched})
			}
		} else bump(results, "match")
	}

	async function serve(relationIds: readonly string[]): Promise<Map<string, string> | null> {
		if (!serveConfig) return null
		const fall = (reason: ServeFallbackReason) => {
			recordServeFallback("versions", RELATION_LOOKUP_SERVE_READ, reason)
			return null
		}
		try {
			if (relationIds.length > MAX_RELATION_IDS) return fall("unsupported")
			const read = geoDb.readRelationEntity
			if (!read) return fall("unsupported")
			const exact = new Map<string, boolean>()
			for (const id of relationIds) {
				const verdict = ledger.check(id)
				if (!verdict.ok) return fall(verdict.reason)
				exact.set(id, verdict.detail?.exact ?? false)
			}
			const answers = new Map<string, string>()
			const lookup = async () => {
				let next = 0
				const worker = async () => {
					while (next < relationIds.length) {
						const id = relationIds[next++] as string
						const rows = await read.call(geoDb, {relationId: conformingId("entity", id)})
						const from = rows[0]?.entity
						if (from) answers.set(id, from)
					}
				}
				await Promise.all(Array.from({length: Math.min(config.concurrency, relationIds.length)}, worker))
			}
			await withServeTimeout(lookup(), serveConfig.timeoutMs)
			// Postgres's map holds the from-entity id as Postgres stores it. geo-db's id is that same id unless the loader remapped a
			// legacy one; the comparison that matched says which, and a remapped id cannot be turned back.
			for (const id of answers.keys()) if (exact.get(id) !== true) return fall("differs")
			recordServed("versions", RELATION_LOOKUP_SERVE_READ)
			return answers
		} catch (error) {
			return fall(failureReason(error))
		}
	}

	return {
		serve,
		lookup(relationIds, served) {
			if (!config.enabled) return
			try {
				if (relationIds.length === 0) return
				if (seen++ % config.sampleEvery !== 0) return bump(skipped, "sample")
				if (!geoDb.readRelationEntity) return recordUnsupported("client")
				if (relationIds.length > MAX_RELATION_IDS) return recordUnsupported("too_many")
				const t = now()
				if (t - windowStart >= 60_000) {
					windowStart = t
					windowCount = 0
				}
				if (windowCount >= config.maxPerMinute) return bump(skipped, "rate")
				if (inFlight >= config.maxInFlight) return recordUnsupported("busy")
				windowCount++
				inFlight++
				void compare([...relationIds], new Map(served))
					.catch(() => bump(results, "failed"))
					.finally(() => {
						inFlight--
					})
			} catch {
				// Never let the shadow reach the response.
			}
		},
	}
}
