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
}

export function createRelationLookupShadow(deps: {
	config: RelationLookupShadowConfig
	geoDb: GeoDbCardClient
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): RelationLookupShadow {
	const {config, geoDb, log} = deps
	const now = deps.now ?? Date.now
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
				continue
			}
			const geo = geoOf.get(id)
			const pg = pgOf.get(id)
			if (pg === (geo ?? undefined)) matched++
			else {
				mismatched++
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

	return {
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
