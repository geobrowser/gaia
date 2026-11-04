/**
 * The reverse id map: a geo-db id back to the gaia id it was conformed from.
 *
 * geo-db holds every non-v4 gaia id under a one-way hash (`conformingId`), so an answer read from geo-db names
 * those entities by ids PostGraphile has never returned. Postgres answers the reverse with one index probe
 * (migration 0116: `geodb.conform` and the partial expression index over legacy entity ids), and the same
 * query finds a v4 id as itself. An id that neither finds is a miss: geo-db holds an entity Postgres does not
 * (yet), and the caller must not serve it.
 *
 * Both kinds of answer are permanent (a gaia id always conforms to the same geo-db id), so they are cached for
 * the life of the process; a miss is not cached, so it is asked again next time.
 */
import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"

/** geo-db ids (32 lowercase hex) -> the gaia id each came from, undashed as the UUID scalar serializes it. An id left out is a miss. */
export type GaiaIdsOf = (geoIds: readonly string[]) => Promise<ReadonlyMap<string, string>>

export type IdMapRow = {geo: string; gaia: string; legacy: boolean}
/** Reads the rows for these geo-db ids. */
export type IdMapQuery = (geoIds: readonly string[]) => Promise<IdMapRow[]>

const HEX32 = /^[0-9a-f]{32}$/

type Database = NodePgDatabase<Record<string, unknown>>

/** How long a read waits before checking again for an index it found missing. */
const INDEX_RECHECK_MS = 60_000

/**
 * One round trip for any number of ids. The ids go in as one array literal; each is checked to be 32 hex first,
 * so the literal cannot carry anything else.
 *
 * Without migration 0116's index (built by hand on a large database) the lookup would hash the whole table, so
 * the reads refuse until it exists: checked on first use, and again at most once a minute while it is missing.
 */
export function idMapQuery(db: Database, now: () => number = Date.now): IdMapQuery {
	let indexed = false
	let checkedAt = Number.NEGATIVE_INFINITY
	return async (geoIds) => {
		if (!indexed && now() - checkedAt >= INDEX_RECHECK_MS) {
			checkedAt = now()
			const found = await db.execute<{ok: boolean}>(
				sql`SELECT to_regclass('public.entities_geodb_conform_idx') IS NOT NULL AS ok`,
			)
			indexed = found.rows[0]?.ok === true
		}
		if (!indexed) throw new Error("entities_geodb_conform_idx does not exist (migration 0116: build it by hand)")
		const ids = geoIds.filter((id) => HEX32.test(id))
		if (ids.length === 0) return []
		const array = `{${ids.join(",")}}`
		const result = await db.execute<IdMapRow>(sql`
			SELECT replace(geodb.conform('entity', id)::text, '-', '') AS geo, replace(id::text, '-', '') AS gaia, true AS legacy
			FROM entities
			WHERE geodb.conform('entity', id) = ANY(${array}::uuid[]) AND NOT geodb.is_v4(id)
			UNION ALL
			SELECT replace(id::text, '-', ''), replace(id::text, '-', ''), false
			FROM entities
			WHERE id = ANY(${array}::uuid[]) AND geodb.is_v4(id)
		`)
		return result.rows
	}
}

// --- metrics -------------------------------------------------------------------------------------------------

/** How each id asked for was answered: from the cache, a legacy id mapped back, a v4 id found as itself, or a miss. */
export const ID_MAP_RESULTS = ["cached", "legacy", "native", "miss"] as const
export type IdMapResult = (typeof ID_MAP_RESULTS)[number]

let active = false
const results = new Map<IdMapResult, number>()
let queriesOk = 0
let queriesFailed = 0
let querySeconds = 0

const bump = (result: IdMapResult, n = 1) => results.set(result, (results.get(result) ?? 0) + n)

export function renderIdMapMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_geodb_id_map_ids_total geo-db ids turned back into gaia ids, by how: cached, legacy (mapped back), native (a v4 id found as itself) or miss (Postgres does not hold it; not served).",
		"# TYPE gaia_api_geodb_id_map_ids_total counter",
		...ID_MAP_RESULTS.map((r) => `gaia_api_geodb_id_map_ids_total{result="${r}"} ${results.get(r) ?? 0}`),
		"# HELP gaia_api_geodb_id_map_queries_total Postgres reads of the reverse id map, by outcome.",
		"# TYPE gaia_api_geodb_id_map_queries_total counter",
		`gaia_api_geodb_id_map_queries_total{outcome="ok"} ${queriesOk}`,
		`gaia_api_geodb_id_map_queries_total{outcome="error"} ${queriesFailed}`,
		"# HELP gaia_api_geodb_id_map_query_seconds_total Time spent in Postgres reads of the reverse id map.",
		"# TYPE gaia_api_geodb_id_map_query_seconds_total counter",
		`gaia_api_geodb_id_map_query_seconds_total ${querySeconds}`,
	]
	return `${lines.join("\n")}\n`
}

/** Tests only. */
export function __resetIdMapMetricsForTests(): void {
	active = false
	results.clear()
	queriesOk = 0
	queriesFailed = 0
	querySeconds = 0
}

// --- the map -------------------------------------------------------------------------------------------------

/** An error the gateway counts under `reason` (a serve fallback), like a GeoDbError. */
export class IdMapError extends Error {
	readonly reason = "id_map"
}

/**
 * The map over `query`, with a bounded cache of answers. Ids are compact lowercase hex; the cache drops its
 * oldest entries past `cacheSize`.
 */
export function createGaiaIdMap(query: IdMapQuery, opts: {cacheSize?: number} = {}): GaiaIdsOf {
	const cacheSize = opts.cacheSize ?? 200_000
	const cache = new Map<string, string>()
	active = true
	return async (geoIds) => {
		const out = new Map<string, string>()
		const ask: string[] = []
		for (const id of new Set(geoIds)) {
			const hit = cache.get(id)
			if (hit !== undefined) {
				out.set(id, hit)
				bump("cached")
			} else {
				ask.push(id)
			}
		}
		if (ask.length === 0) return out
		const start = performance.now()
		let rows: IdMapRow[]
		try {
			rows = await query(ask)
			queriesOk++
		} catch (error) {
			queriesFailed++
			throw new IdMapError(`id map read failed: ${(error as Error)?.message ?? String(error)}`)
		} finally {
			querySeconds += (performance.now() - start) / 1000
		}
		for (const row of rows) {
			out.set(row.geo, row.gaia)
			bump(row.legacy ? "legacy" : "native")
			if (cache.size >= cacheSize) {
				const oldest = cache.keys().next().value
				if (oldest !== undefined) cache.delete(oldest)
			}
			cache.set(row.geo, row.gaia)
		}
		bump("miss", ask.filter((id) => !out.has(id)).length)
		return out
	}
}
