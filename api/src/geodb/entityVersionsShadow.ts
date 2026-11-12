/**
 * The entity-versions shadow (GEODB_VERSIONS_SHADOW, off by default; flag-gated like the profile shadow).
 *
 * `GET /versioned/entities/:id/versions` answers from Postgres (`edit_versions`, newest `version_key` first). When the
 * flag is on, a sampled request also reads the same entity's version log from geo-db (`entity-versions` pipe line, wire
 * revision 12) and counts whether the two agree on three things: how many versions there are, their order (newest
 * first, by edit id) and the author of each. It runs after the response is built, fire-and-forget, never awaited by the
 * request, and swallows every error: it never changes, delays or fails a response. Metrics are counts only; a mismatch warn (first 20) logs the request identity (entity, Space, offset, limit) so a live mismatch can be diagnosed.
 *
 * What the two sides mean differs, and the comparison is as strict as the sides allow:
 * - geo-db's log orders by seal timestamp, then Space id, then version_seq, across every Public Space holding the entity
 *   (the cross-Space ordering rules are the serve doc's, docs/api/10-embedded-and-wire.md, `EntityVersions`; they are not
 *   re-derived here). Postgres orders by `version_key`. Where two edits share a seal time the sides may order them
 *   differently: that is an `order` mismatch to read against the serve doc, not necessarily a bug.
 * - Only the unscoped request is compared: a Space-scoped page is filtered to one Space while geo-db's log spans every
 *   Public Space, so it is counted `unsupported{scoped}` and not read. An offset page is compared: geo-db is read for
 *   the first `offset + limit` rows (at most 100; past that it is `unsupported{deep_page}`) and the rows from `offset` on
 *   are the ones compared.
 * - geo-db lists versions only of Spaces that hold the entity live (serve doc, `EntityVersions`: "an entity every
 *   statement of which a Space has removed lists none of that Space's versions"), while Postgres history also lists
 *   the edits that created and removed its values and relations. So an entity whose statements were all removed
 *   answers total 0 in geo-db and several rows in Postgres. That is not a feeder gap and not a read bug, and counting
 *   it as `mismatch{count}` hides the real ones. When geo-db says total 0 and a page was served, the shadow asks
 *   Postgres (`hasLiveStatements`) whether the entity has any live value or relation: none means `unsupported{removed}`;
 *   some (or the question failing) stays a `count` mismatch, which is then a feeder gap or a read bug.
 * - The page compared is the served one: `served.length` must equal `min(max(geo total - offset, 0), limit)`, and the
 *   geo rows from `offset` on must be the served edits.
 * - Edit-id equality between gaia's `edit_versions.edit_id` and geo-db's sealing EditId is not verified: a store where
 *   they differ shows as `edit_ids` on every comparison. Authors: geo-db may hold none for a Public Space edit (ISS-121),
 *   counted apart as `author_unrecorded`.
 *
 * `gaia_api_versions_shadow_total{result}`: match, mismatch, unsupported (geo-db cannot answer) or failed (a read
 * failed). `..._mismatch_total{field}`: count, edit_ids, order, author, author_unrecorded (a mismatch can name several).
 * `..._unsupported_total{reason}`: scoped, deep_page (offset + limit past 100), removed (see below), busy (pool or in-flight bound), client (a geodb-client older than
 * geo-db 25360b28c6, which has no `entity-versions` line), server (a node before wire revision 12). `..._skipped_total{reason}`:
 * sample (not the 1-in-N picked) or rate (over the per-minute cap). After a `client` or `server` answer the shadow stops
 * asking for `PROBE_COOLDOWN_MS`, so an old client loses one pipe worker per cooldown, not one per sample.
 */

import {conformingId, type GeoVersionRow} from "./card"
import {entityVersionsUnsupported, type GeoDbCardClient, GeoDbError} from "./client"

export type EntityVersionsShadowConfig = {
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

export function readEntityVersionsShadowConfig(env: Record<string, string | undefined>): EntityVersionsShadowConfig {
	const flag = env.GEODB_VERSIONS_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		sampleEvery: wholeNumber(env.GEODB_VERSIONS_SHADOW_SAMPLE_EVERY, DEFAULTS.sampleEvery, 100_000),
		maxPerMinute: wholeNumber(env.GEODB_VERSIONS_SHADOW_MAX_PER_MINUTE, DEFAULTS.maxPerMinute, 6000),
		maxInFlight: wholeNumber(env.GEODB_VERSIONS_SHADOW_MAX_IN_FLIGHT, DEFAULTS.maxInFlight, 16),
	}
}

export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
export const MISMATCH_FIELDS = ["count", "edit_ids", "order", "author", "author_unrecorded"] as const
export const UNSUPPORTED_REASONS = ["scoped", "deep_page", "busy", "client", "server", "removed"] as const
export const SKIP_REASONS = ["sample", "rate"] as const
type Result = (typeof RESULTS)[number]
type MismatchField = (typeof MISMATCH_FIELDS)[number]
type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]
type SkipReason = (typeof SKIP_REASONS)[number]

/** Most rows one geo-db read returns; a page ending past it cannot be compared (`deep_page`). */
const MAX_FIRST = 100

/** How long to leave an old client or server alone after it said it cannot answer. */
export const PROBE_COOLDOWN_MS = 10 * 60_000

let active = false
const results = new Map<Result, number>()
const mismatches = new Map<MismatchField, number>()
const unsupported = new Map<UnsupportedReason, number>()
const skipped = new Map<SkipReason, number>()
const bump = <K>(map: Map<K, number>, key: K): void => {
	map.set(key, (map.get(key) ?? 0) + 1)
}
const recordUnsupported = (reason: UnsupportedReason) => {
	bump(results, "unsupported")
	bump(unsupported, reason)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderEntityVersionsShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_versions_shadow_total Sampled GET /versioned/entities/:id/versions responses served from Postgres against geo-db's entity-versions log (version count, order, author), by result: match, mismatch, unsupported (geo-db cannot answer) or failed (a geo-db read failed).",
		"# TYPE gaia_api_versions_shadow_total counter",
	]
	for (const r of RESULTS) lines.push(`gaia_api_versions_shadow_total{result="${r}"} ${results.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_versions_shadow_mismatch_total Version shadow mismatches by the field that differs (one comparison can differ in several).",
		"# TYPE gaia_api_versions_shadow_mismatch_total counter",
	)
	for (const f of MISMATCH_FIELDS)
		lines.push(`gaia_api_versions_shadow_mismatch_total{field="${f}"} ${mismatches.get(f) ?? 0}`)
	lines.push(
		"# HELP gaia_api_versions_shadow_unsupported_total Version shadow comparisons geo-db could not answer, by reason.",
		"# TYPE gaia_api_versions_shadow_unsupported_total counter",
	)
	for (const u of UNSUPPORTED_REASONS)
		lines.push(`gaia_api_versions_shadow_unsupported_total{reason="${u}"} ${unsupported.get(u) ?? 0}`)
	lines.push(
		"# HELP gaia_api_versions_shadow_skipped_total Eligible version requests not compared, by reason: sample (not the 1-in-N picked) or rate (over the per-minute cap).",
		"# TYPE gaia_api_versions_shadow_skipped_total counter",
	)
	for (const k of SKIP_REASONS)
		lines.push(`gaia_api_versions_shadow_skipped_total{reason="${k}"} ${skipped.get(k) ?? 0}`)
	return lines.join("\n")
}

export function __resetEntityVersionsShadowMetricsForTests(): void {
	active = false
	results.clear()
	mismatches.clear()
	unsupported.clear()
	skipped.clear()
}

/** What the route served: the Postgres page, as `getEntityVersions` returned it. */
export type ServedVersions = {
	/** The entity, 32 hex digits. */
	entityId: string
	spaceId?: string | undefined
	limit: number
	offset: number
	versions: readonly {editId: string; createdById: string | null}[]
}

export type EntityVersionsShadow = {
	/** Compare a served page with geo-db's log, in the background. */
	versions(served: ServedVersions): void
}

/** The fields on which geo-db's first rows differ from the served page; empty when they agree. */
export function compareVersions(served: ServedVersions, rows: readonly GeoVersionRow[]): MismatchField[] {
	const fields = new Set<MismatchField>()
	const total = rows[0]?.total ?? 0
	const n = served.versions.length
	if (n !== Math.min(Math.max(total - served.offset, 0), served.limit)) fields.add("count")
	const geo = rows.slice(served.offset, served.offset + n)
	const servedIds = served.versions.map((v) => v.editId)
	const geoIds = geo.map((r) => r.editId)
	const geoSet = new Set(geoIds)
	if (servedIds.length === geoIds.length && servedIds.every((id) => geoSet.has(id))) {
		if (servedIds.some((id, i) => id !== geoIds[i])) fields.add("order")
	} else if (n === geo.length) {
		fields.add("edit_ids")
	}
	const authorsOf = new Map(geo.map((r) => [r.editId, r.authors]))
	for (const v of served.versions) {
		const authors = authorsOf.get(v.editId)
		if (authors === undefined) continue
		if (v.createdById === null) {
			if (authors.length > 0) fields.add("author")
		} else if (authors.length === 0) {
			fields.add("author_unrecorded")
		} else {
			const known = [v.createdById, conformingId("entity", v.createdById), conformingId("space", v.createdById)]
			if (!authors.some((a) => known.includes(a))) fields.add("author")
		}
	}
	return MISMATCH_FIELDS.filter((f) => fields.has(f))
}

export function createEntityVersionsShadow(deps: {
	config: EntityVersionsShadowConfig
	geoDb: GeoDbCardClient
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
	/** Whether Postgres holds any live value or outgoing relation of the entity (decides `removed` on a geo-db total of 0). */
	hasLiveStatements?: (entityId: string) => Promise<boolean>
}): EntityVersionsShadow {
	const {config, geoDb, log} = deps
	const now = deps.now ?? Date.now
	active = true
	let inFlight = 0
	let seen = 0
	let logged = 0
	let windowStart = now()
	let windowCount = 0
	let blocked: {until: number; reason: "client" | "server"} | null = null

	async function compare(served: ServedVersions): Promise<void> {
		try {
			if (!geoDb.readEntityVersions) return recordUnsupported("client")
			const rows = await geoDb.readEntityVersions({
				entityId: served.entityId,
				first: Math.min(served.offset + Math.max(served.limit, 1), MAX_FIRST),
			})
			const fields = compareVersions(served, rows)
			if (rows.length === 0 && served.versions.length > 0 && deps.hasLiveStatements) {
				const live = await deps.hasLiveStatements(served.entityId).catch(() => true)
				if (!live) return recordUnsupported("removed")
			}
			bump(results, fields.length === 0 ? "match" : "mismatch")
			for (const f of fields) bump(mismatches, f)
			if (fields.length > 0 && logged < 20) {
				logged++
				log.warn("versions shadow mismatch", {
					fields,
					entityId: served.entityId,
					spaceId: served.spaceId,
					offset: served.offset,
					limit: served.limit,
					served: served.versions.length,
					total: rows[0]?.total ?? 0,
				})
			}
		} catch (error) {
			const why = entityVersionsUnsupported(error)
			if (why) {
				blocked = {until: now() + PROBE_COOLDOWN_MS, reason: why}
				return recordUnsupported(why)
			}
			if (error instanceof GeoDbError && error.reason === "overloaded") return recordUnsupported("busy")
			bump(results, "failed")
			if (!(error instanceof GeoDbError)) log.warn("versions shadow failed", {error: String(error)})
		}
	}

	return {
		versions(served) {
			if (!config.enabled) return
			try {
				if (served.spaceId !== undefined) return recordUnsupported("scoped")
				if (served.offset + Math.max(served.limit, 1) > MAX_FIRST) return recordUnsupported("deep_page")
				if (seen++ % config.sampleEvery !== 0) return bump(skipped, "sample")
				if (blocked && now() < blocked.until) return recordUnsupported(blocked.reason)
				const t = now()
				if (t - windowStart >= 60_000) {
					windowStart = t
					windowCount = 0
				}
				if (windowCount >= config.maxPerMinute) return bump(skipped, "rate")
				if (inFlight >= config.maxInFlight) return recordUnsupported("busy")
				windowCount++
				inFlight++
				// compare() catches everything it can throw; the .catch is the last line of defence for the response.
				void compare(served)
					.catch(() => {})
					.finally(() => {
						inFlight--
					})
			} catch {
				// Never let the shadow reach the response.
			}
		},
	}
}
