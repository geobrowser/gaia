/**
 * POST /geodb/replay-pairs: the (entity, space) pairs geodb-card-replay drives the card route with, read from Postgres
 * here so the replay pod needs no database credentials (GEODB_REPLAY_PAIRS=on; off by default, route not mounted).
 *
 * The three reads are the ones geodb-feeder/src/replay.ts `loadPools` runs itself when REPLAY_PAIR_SOURCE is unset:
 * top (each Space's highest `local_scores`), recent (latest `entities.updated_at`) and random (a `values` TABLESAMPLE).
 * Every pair has at least one value in its Space. The answer is cached for `cacheMs`, so a caller cannot make the
 * sampling queries run more than once a minute whatever it sends.
 */

import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Hono} from "hono"

import {isValidUuid, normalizeUuid} from "../utils/uuid"

type Database = NodePgDatabase<Record<string, unknown>>

export const MAX_REPLAY_SPACES = 4096
export const MAX_PER_SPACE = 100
export const MAX_POOL = 3000
const QUERY_TIMEOUT = "60s"

export type ReplayPairs = {
	top: {entity: string; space: string; score: string}[]
	recent: {entity: string; space: string}[]
	random: {entity: string; space: string}[]
}

export type ReplayPairsRequest = {spaces: string[]; perSpace: number; limit: number}

const clamp = (raw: unknown, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

/** A valid request, or null: `spaces` is 0 to MAX_REPLAY_SPACES UUIDs (either spelling); the caps clamp. */
export function parseReplayPairsRequest(body: unknown): ReplayPairsRequest | null {
	if (typeof body !== "object" || body === null) return null
	const b = body as {spaces?: unknown; perSpace?: unknown; limit?: unknown}
	if (!Array.isArray(b.spaces) || b.spaces.length > MAX_REPLAY_SPACES) return null
	if (!b.spaces.every((s) => typeof s === "string" && isValidUuid(s))) return null
	return {
		spaces: [...new Set((b.spaces as string[]).map((s) => normalizeUuid(s)))].sort(),
		perSpace: clamp(b.perSpace, 50, MAX_PER_SPACE),
		limit: clamp(b.limit, 1500, MAX_POOL),
	}
}

const compact = (id: string) => id.replaceAll("-", "").toLowerCase()

export async function readReplayPairs(db: Database, req: ReplayPairsRequest): Promise<ReplayPairs> {
	if (req.spaces.length === 0) return {top: [], recent: [], random: []}
	const ids = sql`ARRAY[${sql.join(
		req.spaces.map((s) => sql`${s}::uuid`),
		sql`, `,
	)}]::uuid[]`
	const run = async <R extends Record<string, unknown>>(q: ReturnType<typeof sql>): Promise<R[]> =>
		db.transaction(async (tx) => {
			await tx.execute(sql.raw(`SET LOCAL statement_timeout = '${QUERY_TIMEOUT}'`))
			await tx.execute(sql.raw("SET TRANSACTION READ ONLY"))
			return (await tx.execute<R>(q)).rows as R[]
		})
	const top = await run<{entity_id: string; space_id: string; score: string}>(sql`
		SELECT t.entity_id::text AS entity_id, s.id::text AS space_id, t.score::text AS score
		FROM unnest(${ids}) s(id), LATERAL (
			SELECT ls.entity_id, ls.score FROM local_scores ls
			WHERE ls.space_id = s.id
			  AND EXISTS (SELECT 1 FROM "values" v WHERE v.entity_id = ls.entity_id AND v.space_id = ls.space_id)
			ORDER BY ls.score DESC LIMIT ${req.perSpace}
		) t`)
	const recent = await run<{entity_id: string; space_id: string}>(sql`
		SELECT v.entity_id::text AS entity_id, v.space_id::text AS space_id
		FROM (SELECT id FROM entities ORDER BY updated_at DESC LIMIT ${req.limit * 2}) e
		JOIN "values" v ON v.entity_id = e.id
		WHERE v.space_id = ANY(${ids}) GROUP BY 1, 2 LIMIT ${req.limit}`)
	const random = await run<{entity_id: string; space_id: string}>(sql`
		SELECT DISTINCT entity_id::text AS entity_id, space_id::text AS space_id
		FROM "values" TABLESAMPLE SYSTEM (0.1) WHERE space_id = ANY(${ids}) LIMIT ${req.limit}`)
	const pair = (r: {entity_id: string; space_id: string}) => ({
		entity: compact(r.entity_id),
		space: compact(r.space_id),
	})
	return {
		top: top.map((r) => ({...pair(r), score: r.score})),
		recent: recent.map(pair),
		random: random.map(pair),
	}
}

export function createReplayPairsRouter(
	db: Database,
	options: {cacheMs?: number; now?: () => number; read?: typeof readReplayPairs} = {},
) {
	const cacheMs = options.cacheMs ?? 60_000
	const now = options.now ?? Date.now
	const read = options.read ?? readReplayPairs
	const cache = new Map<string, {at: number; value: Promise<ReplayPairs>}>()
	const router = new Hono()
	router.post("/replay-pairs", async (c) => {
		const req = parseReplayPairsRequest(await c.req.json().catch(() => null))
		if (!req) return c.json({error: "Invalid parameter", message: "body must be {spaces: uuid[]}"}, 400)
		const key = JSON.stringify(req)
		const hit = cache.get(key)
		if (hit && now() - hit.at < cacheMs) {
			try {
				return c.json(await hit.value)
			} catch {
				return c.json({error: "replay pairs unavailable"}, 503)
			}
		}
		if (cache.size > 4) cache.clear()
		const value = read(db, req)
		cache.set(key, {at: now(), value})
		try {
			return c.json(await value)
		} catch {
			cache.delete(key)
			return c.json({error: "replay pairs unavailable"}, 503)
		}
	})
	return router
}
