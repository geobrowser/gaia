/**
 * The entity-at-version shadow (GEODB_ENTITY_AT_VERSION_SHADOW, off by default; flag-gated like the versions shadow).
 *
 * Two routes answer from Postgres with no geo-db twin:
 * - `snapshot`: `GET /versioned/entities/:id?editId=` (the entity's values and relations at a version);
 * - `enrich_names`: the name resolution of `GET /v2/versioned/entities/:id/diff` (`enrichNames`), which stamps names on
 *   property, relation type and target ids. (`enrichBlocks`, `enrichBlockConfig` and the media enrichment are not twinned
 *   here: they read block, data-block config and media tables geo-db holds no shape for.)
 * When the flag is on, a sampled request also reads the same entity from geo-db after the response is built, fire and
 * forget, never awaited by the request, every error swallowed: it never changes, delays or fails a response. Metrics are
 * counts only and carry fixed labels, never an id. A mismatch warn (first 20) logs the request identity so a live
 * mismatch can be diagnosed, as the versions shadow does.
 *
 * What geo-db can answer today decides the compare:
 * - A scoped request names one Space, so an edit other than the newest is one row of that Space's version log and the card
 *   is read as the Space stood at that version (`readCardAt`, pipe line `<space>@<seq>`, needs a geodb-client from the geo-db
 *   at-version read onward): values and relations compared as the card shadows do (`diffCards`, empty text forgiven). The
 *   newest version of any request is the head, read as before (`entity-versions`, wire revision 12; newest across the Public
 *   Spaces holding the entity, or in the one Space when scoped). Still `unsupported{not_head}`: an unscoped request for an
 *   older edit (the Spaces each stand at their own version; one pin cannot express it), an edit older than the `LOG_ROWS`
 *   newest versions asked for, and a client without `readCardAt`. Block relations are left out of both sides; block
 *   children are not compared. Edit-id equality between `edit_versions.edit_id` and geo-db's sealing EditId is not verified:
 *   a store where they differ shows as `not_head` on every comparison. A geodb-client older than the syntax refuses the
 *   `@` line, which counts as `failed`.
 * - `enrich_names` reads names from the live values table (the request Space's name preferred, any Space's as fallback).
 *   geo-db is asked for the request Space's Name rows of the stamped ids; an id with no Name in that Space was served from
 *   another one, counted `unsupported{name_elsewhere}` (nothing compared) unless another id of the response did compare.
 *
 * `gaia_api_entity_at_version_shadow_total{route,result}`: match, mismatch, unsupported (geo-db cannot answer),
 * absent_ok (neither side holds anything to compare) or failed (a geo-db read failed). `..._mismatch_total{route,field}`:
 * only_postgres, only_geo_db, name. `..._unsupported_total{route,reason}`: not_head, unmapped_space, name_elsewhere, client,
 * server, busy. `..._skipped_total{reason}`: sample, rate. After a `client` or `server` answer the shadow stops asking for
 * `PROBE_COOLDOWN_MS`. There is no `reference_timeout`: Postgres has already answered when the shadow runs, and the shadow
 * makes no Postgres read of its own.
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {
	type CardStatement,
	conformingId,
	diffCards,
	type GeoCardRow,
	type GeoVersionRow,
	type RelationRow,
	statementsFromPostgres,
	type ValueRow,
} from "./card"
import {entityVersionsUnsupported, type GeoDbCardClient, GeoDbError, MAX_CARD_IDS} from "./client"
import {NAME_PROPERTY} from "./rankedReads/typeCounts"

export type EntityAtVersionShadowConfig = {
	enabled: boolean
	/** One request in this many is compared. */
	sampleEvery: number
	/** Most comparisons started a minute. */
	maxPerMinute: number
	/** Most comparisons running at once; a new one past it is dropped (counted `busy`). */
	maxInFlight: number
}

const DEFAULTS = {sampleEvery: 10, maxPerMinute: 30, maxInFlight: 2}
const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readEntityAtVersionShadowConfig(env: Record<string, string | undefined>): EntityAtVersionShadowConfig {
	const flag = env.GEODB_ENTITY_AT_VERSION_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		sampleEvery: wholeNumber(env.GEODB_ENTITY_AT_VERSION_SHADOW_SAMPLE_EVERY, DEFAULTS.sampleEvery, 100_000),
		maxPerMinute: wholeNumber(env.GEODB_ENTITY_AT_VERSION_SHADOW_MAX_PER_MINUTE, DEFAULTS.maxPerMinute, 6000),
		maxInFlight: wholeNumber(env.GEODB_ENTITY_AT_VERSION_SHADOW_MAX_IN_FLIGHT, DEFAULTS.maxInFlight, 16),
	}
}

export const ROUTES = ["snapshot", "enrich_names"] as const
export const RESULTS = ["match", "mismatch", "unsupported", "absent_ok", "failed"] as const
export const MISMATCH_FIELDS = ["only_postgres", "only_geo_db", "name"] as const
export const UNSUPPORTED_REASONS = ["not_head", "unmapped_space", "name_elsewhere", "client", "server", "busy"] as const
export const SKIP_REASONS = ["sample", "rate"] as const
type Route = (typeof ROUTES)[number]
type Result = (typeof RESULTS)[number]
type MismatchField = (typeof MISMATCH_FIELDS)[number]
type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]
type SkipReason = (typeof SKIP_REASONS)[number]

/** Most rows of the version log one read asks for; it is enough to name the newest version and the Spaces holding the entity. */
const LOG_ROWS = 100

/** How long to leave an old client or server alone after it said it cannot answer. */
export const PROBE_COOLDOWN_MS = 10 * 60_000

let active = false
const results = new Map<string, number>()
const mismatches = new Map<string, number>()
const unsupported = new Map<string, number>()
const skipped = new Map<SkipReason, number>()
const bump = <K>(map: Map<K, number>, key: K): void => {
	map.set(key, (map.get(key) ?? 0) + 1)
}
const recordResult = (route: Route, result: Result) => bump(results, `${route}\t${result}`)
const recordUnsupported = (route: Route, reason: UnsupportedReason) => {
	recordResult(route, "unsupported")
	bump(unsupported, `${route}\t${reason}`)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderEntityAtVersionShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_entity_at_version_shadow_total Sampled entity-at-version responses served from Postgres against geo-db, by route and result: match, mismatch, unsupported (geo-db cannot answer), absent_ok (nothing on either side) or failed (a geo-db read failed).",
		"# TYPE gaia_api_entity_at_version_shadow_total counter",
	]
	for (const route of ROUTES)
		for (const r of RESULTS)
			lines.push(
				`gaia_api_entity_at_version_shadow_total{route="${route}",result="${r}"} ${results.get(`${route}\t${r}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_entity_at_version_shadow_mismatch_total Entity-at-version mismatches by route and what differs: only_postgres, only_geo_db (statements) or name.",
		"# TYPE gaia_api_entity_at_version_shadow_mismatch_total counter",
	)
	for (const route of ROUTES)
		for (const f of MISMATCH_FIELDS)
			lines.push(
				`gaia_api_entity_at_version_shadow_mismatch_total{route="${route}",field="${f}"} ${mismatches.get(`${route}\t${f}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_entity_at_version_shadow_unsupported_total Entity-at-version comparisons geo-db could not answer, by route and reason.",
		"# TYPE gaia_api_entity_at_version_shadow_unsupported_total counter",
	)
	for (const route of ROUTES)
		for (const u of UNSUPPORTED_REASONS)
			lines.push(
				`gaia_api_entity_at_version_shadow_unsupported_total{route="${route}",reason="${u}"} ${unsupported.get(`${route}\t${u}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_entity_at_version_shadow_skipped_total Eligible requests not compared, by reason: sample (not the 1-in-N picked) or rate (over the per-minute cap).",
		"# TYPE gaia_api_entity_at_version_shadow_skipped_total counter",
	)
	for (const k of SKIP_REASONS)
		lines.push(`gaia_api_entity_at_version_shadow_skipped_total{reason="${k}"} ${skipped.get(k) ?? 0}`)
	return lines.join("\n")
}

export function __resetEntityAtVersionShadowMetricsForTests(): void {
	active = false
	results.clear()
	mismatches.clear()
	unsupported.clear()
	skipped.clear()
}

/** A value as `GET /versioned/entities/:id` serves it (the columns the card compare reads). */
export type ServedValue = {
	propertyId: string
	text?: string | null
	integer?: string | number | null
	float?: string | number | null
	decimal?: string | number | null
	boolean?: boolean | null
	datetime?: string | null
	date?: string | null
	point?: string | null
}

/** What `GET /versioned/entities/:id?editId=` served: the entity's values and non-block relations at the version. */
export type ServedSnapshot = {
	entityId: string
	editId: string
	/** The request's Space, when it was scoped. */
	spaceId?: string | undefined
	values: readonly ServedValue[]
	relations: readonly {typeId: string; toEntityId: string; toSpaceId?: string | null | undefined}[]
}

/** What the v2 diff's name enrichment stamped: each id and the name it was given, in the request's Space. */
export type ServedNames = {
	spaceId: string
	names: ReadonlyMap<string, string>
}

export type EntityAtVersionShadow = {
	/** Compare a served snapshot with geo-db's entity, in the background. */
	snapshot(served: ServedSnapshot): void
	/** Compare served names with geo-db's, in the background. */
	names(served: ServedNames): void
}

const compact = (id: string) => id.replaceAll("-", "").toLowerCase()
const BLOCKS_PROPERTY = conformingId("entity", SystemIds.BLOCKS)
const NAME_PROP = conformingId("entity", NAME_PROPERTY)

/** The fields on which Postgres's snapshot differs from geo-db's card at the head; empty when they agree. */
export function compareSnapshot(served: ServedSnapshot, card: GeoCardRow[]): MismatchField[] {
	const values: ValueRow[] = served.values.map((v) => ({
		property_id: v.propertyId,
		text: v.text ?? null,
		integer: v.integer ?? null,
		float: v.float ?? null,
		decimal: v.decimal ?? null,
		boolean: v.boolean ?? null,
		datetime: v.datetime ?? null,
		date: v.date ?? null,
		point: v.point ?? null,
	}))
	const relations: RelationRow[] = served.relations.map((r) => ({
		type_id: r.typeId,
		to_entity_id: r.toEntityId,
		to_space_id: r.toSpaceId ?? null,
	}))
	const pg = statementsFromPostgres(values, relations)
	const statements: CardStatement[] = pg.statements.filter((s) => s.prop !== BLOCKS_PROPERTY)
	const diff = diffCards(
		{statements, skipped: pg.skipped},
		card.filter((row) => row.prop !== BLOCKS_PROPERTY),
		{skipEmptyText: true},
	)
	const fields: MismatchField[] = []
	if (diff.onlyPostgres.length > 0) fields.push("only_postgres")
	if (diff.onlyGeoDb.length > 0) fields.push("only_geo_db")
	return fields
}

/** How many of the ids the card rows name agree with `names`: `ok` equal, `wrong` named differently, `elsewhere` named nowhere in the Space. */
export function compareNames(
	names: ReadonlyMap<string, string>,
	card: GeoCardRow[],
): {ok: number; wrong: number; elsewhere: number} {
	const held = new Map<string, Set<string>>()
	for (const row of card) {
		if (row.prop !== NAME_PROP || row.entity === undefined) continue
		const set = held.get(row.entity) ?? new Set<string>()
		set.add(row.raw)
		held.set(row.entity, set)
	}
	let ok = 0
	let wrong = 0
	let elsewhere = 0
	for (const [id, name] of names) {
		const geo = held.get(conformingId("entity", id))
		if (geo === undefined) elsewhere++
		else if (geo.has(name)) ok++
		else wrong++
	}
	return {ok, wrong, elsewhere}
}

export function createEntityAtVersionShadow(deps: {
	config: EntityAtVersionShadowConfig
	geoDb: GeoDbCardClient
	/** The geo-db id of a gaia Space, or null when geo-db does not hold it. */
	geoSpaceOf: (gaiaSpaceId: string) => Promise<string | null>
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): EntityAtVersionShadow {
	const {config, geoDb, log, geoSpaceOf} = deps
	const now = deps.now ?? Date.now
	active = true
	let inFlight = 0
	let seen = 0
	let logged = 0
	let windowStart = now()
	let windowCount = 0
	let blocked: {until: number; reason: "client" | "server"} | null = null

	const finish = (route: Route, fields: MismatchField[], identity: Record<string, unknown>) => {
		recordResult(route, fields.length === 0 ? "match" : "mismatch")
		for (const f of fields) bump(mismatches, `${route}\t${f}`)
		if (fields.length > 0 && logged < 20) {
			logged++
			log.warn("entity-at-version shadow mismatch", {route, fields, ...identity})
		}
	}

	const failure = (route: Route, error: unknown) => {
		const why = entityVersionsUnsupported(error)
		if (why) {
			blocked = {until: now() + PROBE_COOLDOWN_MS, reason: why}
			return recordUnsupported(route, why)
		}
		if (error instanceof GeoDbError && error.reason === "overloaded") return recordUnsupported(route, "busy")
		recordResult(route, "failed")
		if (!(error instanceof GeoDbError)) log.warn("entity-at-version shadow failed", {route, error: String(error)})
	}

	async function compareSnapshotRoute(served: ServedSnapshot): Promise<void> {
		try {
			if (!geoDb.readEntityVersions) return recordUnsupported("snapshot", "client")
			const entity = conformingId("entity", served.entityId)
			let geoSpace: string | null = null
			if (served.spaceId !== undefined) {
				geoSpace = await geoSpaceOf(compact(served.spaceId))
				if (geoSpace === null) return recordUnsupported("snapshot", "unmapped_space")
			}
			const log100: GeoVersionRow[] = await geoDb.readEntityVersions({entityId: entity, first: LOG_ROWS})
			const rows = geoSpace === null ? log100 : log100.filter((r) => r.space === geoSpace)
			if (rows.length === 0) {
				// geo-db lists no version of an entity it holds no live statement of: both empty is agreement, else the version is
				// one the entity has since been emptied of, which only an at-version read could answer.
				if (served.values.length === 0 && served.relations.length === 0)
					return recordResult("snapshot", "absent_ok")
				return recordUnsupported("snapshot", "not_head")
			}
			if (compact(rows[0]?.editId ?? "") !== compact(served.editId)) {
				// Not the newest version: a scoped request names one Space, so the version is one row of its log and the card is
				// read as the Space stood at that version (`<space>@<seq>`). An unscoped request spans Spaces, each at its own
				// version, which one pin cannot express: still not_head. A client or store without the syntax, or an edit
				// beyond the rows asked for, is the same not_head it was before.
				const at =
					geoSpace === null ? undefined : rows.find((r) => compact(r.editId) === compact(served.editId))
				if (geoSpace === null || at === undefined || !geoDb.readCardAt)
					return recordUnsupported("snapshot", "not_head")
				const old = await geoDb.readCardAt({geoSpaceId: geoSpace, entityId: entity, seq: at.seq})
				return finish("snapshot", compareSnapshot(served, old), {
					entityId: served.entityId,
					editId: served.editId,
					spaceId: served.spaceId,
					values: served.values.length,
					relations: served.relations.length,
					seq: at.seq,
				})
			}
			let card: GeoCardRow[]
			if (geoSpace !== null) {
				card = await geoDb.readCard({geoSpaceId: geoSpace, entityId: entity})
			} else {
				if (!geoDb.readCardSpaces) return recordUnsupported("snapshot", "client")
				card = await geoDb.readCardSpaces({
					geoSpaceIds: [...new Set(rows.map((r) => r.space))],
					entityId: entity,
				})
			}
			finish("snapshot", compareSnapshot(served, card), {
				entityId: served.entityId,
				editId: served.editId,
				spaceId: served.spaceId,
				values: served.values.length,
				relations: served.relations.length,
			})
		} catch (error) {
			failure("snapshot", error)
		}
	}

	async function compareNamesRoute(served: ServedNames): Promise<void> {
		try {
			if (served.names.size === 0) return recordResult("enrich_names", "absent_ok")
			if (!geoDb.readCards) return recordUnsupported("enrich_names", "client")
			const geoSpace = await geoSpaceOf(compact(served.spaceId))
			if (geoSpace === null) return recordUnsupported("enrich_names", "unmapped_space")
			const ids = [...served.names.keys()].slice(0, MAX_CARD_IDS)
			const sent = new Map(ids.map((id) => [id, served.names.get(id) as string]))
			const card = await geoDb.readCards({
				geoSpaceIds: [geoSpace],
				entityIds: ids.map((id) => conformingId("entity", id)),
			})
			const {ok, wrong, elsewhere} = compareNames(sent, card)
			if (wrong > 0) return finish("enrich_names", ["name"], {spaceId: served.spaceId, ids: sent.size, wrong})
			if (ok > 0) return finish("enrich_names", [], {})
			recordUnsupported("enrich_names", elsewhere > 0 ? "name_elsewhere" : "name_elsewhere")
		} catch (error) {
			failure("enrich_names", error)
		}
	}

	/** The gates every route shares; true when a comparison may start now. */
	function admit(route: Route): boolean {
		if (seen++ % config.sampleEvery !== 0) {
			bump(skipped, "sample")
			return false
		}
		if (blocked && now() < blocked.until) {
			recordUnsupported(route, blocked.reason)
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

	const run = (route: Route, compare: () => Promise<void>) => {
		if (!config.enabled) return
		try {
			if (!admit(route)) return
			// compare catches everything it can throw; the .catch is the last line of defence for the response.
			void compare()
				.catch(() => {})
				.finally(() => {
					inFlight--
				})
		} catch {
			// Never let the shadow reach the response.
		}
	}

	return {
		snapshot: (served) => run("snapshot", () => compareSnapshotRoute(served)),
		names: (served) => run("enrich_names", () => compareNamesRoute(served)),
	}
}
