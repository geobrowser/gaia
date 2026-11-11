/**
 * The entity-diff shadow (GEODB_ENTITY_DIFF_SHADOW, off by default; flag-gated like the proposal-diff shadow).
 *
 * Two diff routes that proposalDiffShadow.ts and the versioned queries' shadows do not cover answer from Postgres:
 * - `entity_diff`: `GET /versioned/entities/:id/diff` and `GET /v2/versioned/entities/:id/diff`, the entity laid
 *   between two edits (`fromEditId`, `toEditId`) in one Space.
 * - `review`: `POST /v2/versioned/review`, a Space's unpublished edit laid against the live knowledge graph.
 *
 * When the flag is on, a sampled request reads the entity's state from geo-db after the response is built,
 * fire-and-forget, never awaited by the request, swallowing every error: it never changes, delays or fails a
 * response. Metrics are counts only; the first 20 mismatches log the route and counts, never an entity, Space or edit id.
 *
 * `review` has a live base, so it reuses the proposal-diff shadow's base-side check: for every UPDATE or REMOVE
 * statement in the request's Space, does geo-db's card (`readCards`, one Space) hold it (`base_missing` when not).
 *
 * `entity_diff` is between two historical versions, which geo-db cannot rebuild. The one side it can check is the
 * after side when `toEditId` is the entity's newest version in the Space (the version log's head, read with
 * `readEntityVersions`, as entityAtVersionShadow.ts does): then the after side IS the live state, so every
 * value and relation the diff shows as added or updated must be on geo-db's card (`after_missing` when not).
 *
 * What it cannot answer, and why (each is counted `unsupported{reason}`):
 * - `not_head`: `toEditId` is not the newest version geo-db lists for the entity in the Space, so the after side
 *   is a past state. Needs an as-of read the engine does not have (the gap gaia#1437 names).
 * - the before side of `entity_diff`, always: also an as-of read. It is not counted, because it never could be.
 * - `no_after` (entity_diff) / `no_base` (review): the diff has nothing on the side that can be compared.
 * - `unmapped_space`, `client` (a geo-db client without the reads), `busy` (the pool or the in-flight bound).
 * Block changes and statements in other Spaces are not compared.
 * Removed relations are compared by id at the head: geo-db's relation-by-id read (`readRelationEntity`) returns an
 * empty page for a closed relation, so each removal must read empty (`removed_open` when geo-db still holds it).
 * A client without that read skips the removals (and counts `unsupported{client}` when they are all there is).
 *
 * `gaia_api_entity_diff_shadow_total{route,result}`: match, mismatch, unsupported or failed.
 * `..._mismatch_total{route,field}`: after_missing and removed_open (entity_diff) or base_missing (review).
 * `..._statements_total{route,outcome}`: found or missing, the statements compared.
 * `..._unsupported_total{route,reason}` and `..._skipped_total{reason}` (sample, rate) as in the proposal-diff shadow.
 */

import {conformingId, type GeoCardRow, type GeoVersionRow} from "./card"
import {type GeoDbCardClient, GeoDbError, MAX_CARD_IDS} from "./client"
import {baseStatements, compareBase} from "./proposalDiffShadow"

export type EntityDiffShadowConfig = {
	enabled: boolean
	sampleEvery: number
	maxPerMinute: number
	maxInFlight: number
}

const DEFAULTS = {sampleEvery: 10, maxPerMinute: 30, maxInFlight: 2}
const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readEntityDiffShadowConfig(env: Record<string, string | undefined>): EntityDiffShadowConfig {
	const flag = env.GEODB_ENTITY_DIFF_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		sampleEvery: wholeNumber(env.GEODB_ENTITY_DIFF_SHADOW_SAMPLE_EVERY, DEFAULTS.sampleEvery, 100_000),
		maxPerMinute: wholeNumber(env.GEODB_ENTITY_DIFF_SHADOW_MAX_PER_MINUTE, DEFAULTS.maxPerMinute, 6000),
		maxInFlight: wholeNumber(env.GEODB_ENTITY_DIFF_SHADOW_MAX_IN_FLIGHT, DEFAULTS.maxInFlight, 16),
	}
}

export const ROUTES = ["entity_diff", "review"] as const
export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
export const MISMATCH_FIELDS = ["after_missing", "base_missing", "removed_open"] as const
export const OUTCOMES = ["found", "missing"] as const
export const UNSUPPORTED_REASONS = ["not_head", "no_after", "no_base", "unmapped_space", "client", "busy"] as const
export const SKIP_REASONS = ["sample", "rate"] as const
type Route = (typeof ROUTES)[number]
type Result = (typeof RESULTS)[number]
type MismatchField = (typeof MISMATCH_FIELDS)[number]
type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]
type SkipReason = (typeof SKIP_REASONS)[number]

/** Most rows of the version log one read asks for: enough to name the newest version in the Space. */
const LOG_ROWS = 100

let active = false
const results = new Map<string, number>()
const mismatches = new Map<string, number>()
const outcomes = new Map<string, number>()
const unsupported = new Map<string, number>()
const skipped = new Map<SkipReason, number>()
const bump = <K>(map: Map<K, number>, key: K, by = 1): void => {
	map.set(key, (map.get(key) ?? 0) + by)
}
const recordResult = (route: Route, result: Result) => bump(results, `${route}\t${result}`)
const recordUnsupported = (route: Route, reason: UnsupportedReason) => {
	recordResult(route, "unsupported")
	bump(unsupported, `${route}\t${reason}`)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderEntityDiffShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_entity_diff_shadow_total Sampled entity diff and review diff responses served from Postgres, checked against geo-db, by route and result: match, mismatch, unsupported (geo-db cannot answer) or failed (a geo-db read failed).",
		"# TYPE gaia_api_entity_diff_shadow_total counter",
	]
	for (const route of ROUTES)
		for (const r of RESULTS)
			lines.push(
				`gaia_api_entity_diff_shadow_total{route="${route}",result="${r}"} ${results.get(`${route}\t${r}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_entity_diff_shadow_mismatch_total Entity diff shadow mismatches by route and the field that differs: after_missing (an after-side statement not on geo-db's card) or base_missing (a base statement not on it).",
		"# TYPE gaia_api_entity_diff_shadow_mismatch_total counter",
	)
	for (const route of ROUTES)
		for (const f of MISMATCH_FIELDS)
			lines.push(
				`gaia_api_entity_diff_shadow_mismatch_total{route="${route}",field="${f}"} ${mismatches.get(`${route}\t${f}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_entity_diff_shadow_statements_total Statements compared, by route and whether geo-db held them.",
		"# TYPE gaia_api_entity_diff_shadow_statements_total counter",
	)
	for (const route of ROUTES)
		for (const o of OUTCOMES)
			lines.push(
				`gaia_api_entity_diff_shadow_statements_total{route="${route}",outcome="${o}"} ${outcomes.get(`${route}\t${o}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_entity_diff_shadow_unsupported_total Entity diff shadow comparisons geo-db could not answer, by route and reason.",
		"# TYPE gaia_api_entity_diff_shadow_unsupported_total counter",
	)
	for (const route of ROUTES)
		for (const u of UNSUPPORTED_REASONS)
			lines.push(
				`gaia_api_entity_diff_shadow_unsupported_total{route="${route}",reason="${u}"} ${unsupported.get(`${route}\t${u}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_entity_diff_shadow_skipped_total Eligible diff requests not compared, by reason: sample (not the 1-in-N picked) or rate (over the per-minute cap).",
		"# TYPE gaia_api_entity_diff_shadow_skipped_total counter",
	)
	for (const k of SKIP_REASONS)
		lines.push(`gaia_api_entity_diff_shadow_skipped_total{reason="${k}"} ${skipped.get(k) ?? 0}`)
	return lines.join("\n")
}

export function __resetEntityDiffShadowMetricsForTests(): void {
	active = false
	results.clear()
	mismatches.clear()
	outcomes.clear()
	unsupported.clear()
	skipped.clear()
}

/** What `GET /(v2/)versioned/entities/:id/diff` served: the change lists, with the after side. */
export type ServedEntityDiff = {
	route: "entity_diff"
	/** The request's Space. */
	spaceId: string
	entityId: string
	toEditId: string
	values: readonly {propertyId: string; spaceId: string; before: string | null; after: string | null}[]
	relations: readonly {
		relationId: string
		typeId: string
		spaceId: string
		before?: {toEntityId: string} | null
		after?: {toEntityId: string} | null
	}[]
}

/** What `POST /v2/versioned/review` served: the page's entity diffs, laid against the live graph. */
export type ServedReviewDiff = {
	route: "review"
	spaceId: string
	entities: readonly {
		entityId: string
		values: readonly {propertyId: string; spaceId: string; before: string | null}[]
		relations: readonly {typeId: string; spaceId: string; before?: {toEntityId: string} | null}[]
	}[]
}

export type EntityDiffShadow = {
	/** Compare a served entity diff's after side with geo-db, in the background. */
	entityDiff(served: ServedEntityDiff): void
	/** Compare a served review diff's base side with geo-db, in the background. */
	review(served: ServedReviewDiff): void
}

type AfterStatements = {
	entityId: string
	values: string[]
	relations: {prop: string; target: string}[]
	/** Relations the diff removes, by id: at the head each must be closed in geo-db. */
	removedRelations: string[]
}

/** Most removed relations read from geo-db for one diff (one read each). */
const MAX_REMOVED = 16

/** The after-side statements (a value or relation the diff adds or updates) in the request's Space, after the id remap. */
export function afterStatements(served: ServedEntityDiff): AfterStatements {
	return {
		entityId: conformingId("entity", served.entityId),
		values: served.values
			.filter((v) => v.after !== null && v.spaceId === served.spaceId)
			.map((v) => conformingId("entity", v.propertyId)),
		relations: served.relations
			.filter((r) => r.after && r.spaceId === served.spaceId)
			.map((r) => ({
				prop: conformingId("entity", r.typeId),
				target: conformingId("entity", (r.after as {toEntityId: string}).toEntityId),
			})),
		removedRelations: served.relations
			.filter((r) => r.before && !r.after && r.spaceId === served.spaceId)
			.map((r) => conformingId("entity", r.relationId))
			.slice(0, MAX_REMOVED),
	}
}

/** How many after statements geo-db's card holds, and how many it lacks. */
export function compareAfter(after: AfterStatements, card: readonly GeoCardRow[]): {found: number; missing: number} {
	let found = 0
	let missing = 0
	for (const prop of after.values) card.some((r) => r.prop === prop) ? found++ : missing++
	for (const r of after.relations) card.some((c) => c.prop === r.prop && c.raw === r.target) ? found++ : missing++
	return {found, missing}
}

const compact = (id: string) => id.replaceAll("-", "").toLowerCase()

export function createEntityDiffShadow(deps: {
	config: EntityDiffShadowConfig
	geoDb: GeoDbCardClient
	/** gaia Space id (compact) -> geo-db Space id, or null when geo-db does not hold the Space. */
	geoSpaceOf: (gaiaSpaceId: string) => Promise<string | null>
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): EntityDiffShadow {
	const {config, geoDb, log} = deps
	const now = deps.now ?? Date.now
	active = true
	let inFlight = 0
	let seen = 0
	let logged = 0
	let windowStart = now()
	let windowCount = 0

	const finish = (route: Route, field: MismatchField, entities: number, found: number, missing: number) => {
		bump(outcomes, `${route}\tfound`, found)
		bump(outcomes, `${route}\tmissing`, missing)
		recordResult(route, missing === 0 ? "match" : "mismatch")
		if (missing === 0) return
		bump(mismatches, `${route}\t${field}`)
		if (logged < 20) {
			logged++
			log.warn("entity diff shadow mismatch", {route, entities, found, missing})
		}
	}

	const failure = (route: Route, error: unknown) => {
		if (error instanceof GeoDbError && error.reason === "unmapped_space")
			return recordUnsupported(route, "unmapped_space")
		if (error instanceof GeoDbError && error.reason === "overloaded") return recordUnsupported(route, "busy")
		recordResult(route, "failed")
		if (!(error instanceof GeoDbError)) log.warn("entity diff shadow failed", {route, error: String(error)})
	}

	async function compareEntityDiff(served: ServedEntityDiff, after: AfterStatements): Promise<void> {
		try {
			if (!geoDb.readEntityVersions) return recordUnsupported("entity_diff", "client")
			const geoSpaceId = await deps.geoSpaceOf(compact(served.spaceId))
			if (!geoSpaceId) return recordUnsupported("entity_diff", "unmapped_space")
			const log100: GeoVersionRow[] = await geoDb.readEntityVersions({entityId: after.entityId, first: LOG_ROWS})
			const rows = log100.filter((r) => r.space === geoSpaceId)
			if (compact(rows[0]?.editId ?? "") !== compact(served.toEditId))
				return recordUnsupported("entity_diff", "not_head")
			const hasStatements = after.values.length + after.relations.length > 0
			if (!hasStatements && !geoDb.readRelationEntity) return recordUnsupported("entity_diff", "client")
			const card = hasStatements ? await geoDb.readCard({geoSpaceId, entityId: after.entityId}) : []
			const {found, missing} = compareAfter(after, card)
			// A removed relation is closed in geo-db at the head: its by-id read returns an empty page.
			let open = 0
			let closed = 0
			if (geoDb.readRelationEntity)
				for (const relationId of after.removedRelations)
					(await geoDb.readRelationEntity({relationId})).length === 0 ? closed++ : open++
			if (open > 0) bump(mismatches, "entity_diff\tremoved_open")
			finish("entity_diff", "after_missing", 1, found + closed, missing + open)
		} catch (error) {
			failure("entity_diff", error)
		}
	}

	async function compareReview(served: ServedReviewDiff, base: ReturnType<typeof baseStatements>): Promise<void> {
		try {
			if (!geoDb.readCards) return recordUnsupported("review", "client")
			const geoSpaceId = await deps.geoSpaceOf(compact(served.spaceId))
			if (!geoSpaceId) return recordUnsupported("review", "unmapped_space")
			const rows = await geoDb.readCards({
				geoSpaceIds: [geoSpaceId],
				entityIds: base.slice(0, MAX_CARD_IDS).map((b) => b.entityId),
			})
			const {found, missing} = compareBase(base, rows)
			finish("review", "base_missing", base.length, found, missing)
		} catch (error) {
			failure("review", error)
		}
	}

	/** The gates every route shares; true when a comparison may start now. */
	function admit(route: Route): boolean {
		if (seen++ % config.sampleEvery !== 0) {
			bump(skipped, "sample")
			return false
		}
		const t = now()
		if (t - windowStart >= 60_000) {
			windowStart = t
			windowCount = 0
		}
		if (windowCount >= config.maxPerMinute) {
			bump(skipped, "rate")
			return false
		}
		if (inFlight >= config.maxInFlight) {
			recordUnsupported(route, "busy")
			return false
		}
		windowCount++
		inFlight++
		return true
	}

	const start = (compare: () => Promise<void>) => {
		// compare catches everything it can throw; the .catch is the last line of defence for the response.
		void compare()
			.catch(() => {})
			.finally(() => {
				inFlight--
			})
	}

	return {
		entityDiff(served) {
			if (!config.enabled) return
			try {
				if (!admit("entity_diff")) return
				const after = afterStatements(served)
				if (after.values.length + after.relations.length + after.removedRelations.length === 0) {
					inFlight--
					return recordUnsupported("entity_diff", "no_after")
				}
				start(() => compareEntityDiff(served, after))
			} catch {
				// Never let the shadow reach the response.
			}
		},
		review(served) {
			if (!config.enabled) return
			try {
				if (!admit("review")) return
				const base = baseStatements({
					route: "proposal",
					mode: "active",
					spaceId: served.spaceId,
					entities: served.entities.map((e) => ({...e})),
				})
				if (base.length === 0) {
					inFlight--
					return recordUnsupported("review", "no_base")
				}
				start(() => compareReview(served, base))
			} catch {
				// Never let the shadow reach the response.
			}
		},
	}
}
