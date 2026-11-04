/**
 * geodb-card-replay: steady, small, realistic traffic on the api's entity-card route, so the shadow compare
 * (`gaia_api_geodb_card_shadow_total{result}`) has a week of samples to read even when people are not opening
 * cards. In cluster only, through Service `api`; never the public endpoint.
 *
 * Pairs (entity, space) come from gaia Postgres and are refreshed every few minutes, weighted toward what
 * people open (there is no view log, so the proxies are):
 * - top: each mapped Space's highest-scored entities (`local_scores`), Zipf-weighted by rank in the Space;
 * - recent: entities edited most recently (`entities.updated_at`);
 * - random: a uniform sample of (entity, space) with values, so the long tail is covered too.
 * Every pair has at least one value in its Space: an entity with no rows on either side would score a
 * trivial `match`.
 */
import type {Series} from "./metrics.js"
import {compactId} from "./pgAdapter.js"
import type {ReadOnlyPg} from "./pgSource.js"

export type Pair = {entity: string; space: string; weight: number}
export type Pools = {top: Pair[]; recent: Pair[]; random: Pair[]}

/** How a request is split between the pools; a pool that is empty passes its share to the others. */
export const POOL_SHARE = {top: 0.5, recent: 0.3, random: 0.2} as const

/** The only hosts the driver will call: Service `api` in namespace `gaia`, as cluster DNS spells it. */
const ALLOWED_HOSTS = new Set(["api", "api.gaia", "api.gaia.svc", "api.gaia.svc.cluster.local"])

/** The api base URL, refusing anything but plain http to Service `api` on port 3000. */
export function checkTarget(raw: string): URL {
	const u = new URL(raw)
	if (u.protocol !== "http:" || !ALLOWED_HOSTS.has(u.hostname) || u.port !== "3000" || u.username || u.password) {
		throw new Error(`REPLAY_API_URL must be http://api:3000 (Service api in namespace gaia), not ${u.origin}`)
	}
	return u
}

/** Requests per second, default 0.5; refuses anything above 1. */
export function parseRate(raw: string | undefined): number {
	const r = raw === undefined || raw.trim() === "" ? 0.5 : Number(raw)
	if (!Number.isFinite(r) || r <= 0 || r > 1) throw new Error(`REPLAY_RATE must be in (0, 1], not ${raw}`)
	return r
}

/**
 * Space ids the api can map (the `space-map.json` the feeder publishes): founded and not marked
 * `loaded: false`, which the api reads as "not loaded", so a card there would only be a skipped read.
 */
export function mappedSpaces(spaceMapText: string): Set<string> {
	const m = JSON.parse(spaceMapText) as Record<string, {geo?: string; loaded?: boolean}>
	return new Set(
		Object.entries(m)
			.filter(([, v]) => typeof v?.geo === "string" && v.loaded !== false)
			.map(([k]) => compactId(k)),
	)
}

const dashed = (id: string) => `${id.slice(0, 8)}-${id.slice(8, 12)}-${id.slice(12, 16)}-${id.slice(16, 20)}-${id.slice(20)}`

export async function loadPools(pg: Pick<ReadOnlyPg, "read">, spaces: string[], perSpace = 50, limit = 1500): Promise<Pools> {
	const ids = spaces.map(dashed)
	if (ids.length === 0) return {top: [], recent: [], random: []}
	const top = await pg.read<{entity_id: string; space_id: string; score: string}>(
		`SELECT t.entity_id::text AS entity_id, s.id::text AS space_id, t.score::text AS score
		 FROM unnest($1::uuid[]) s(id), LATERAL (
			SELECT ls.entity_id, ls.score FROM local_scores ls
			WHERE ls.space_id = s.id
			  AND EXISTS (SELECT 1 FROM "values" v WHERE v.entity_id = ls.entity_id AND v.space_id = ls.space_id)
			ORDER BY ls.score DESC LIMIT $2
		 ) t`,
		[ids, perSpace],
		60_000,
	)
	const recent = await pg.read<{entity_id: string; space_id: string}>(
		`SELECT v.entity_id::text AS entity_id, v.space_id::text AS space_id
		 FROM (SELECT id FROM entities ORDER BY updated_at DESC LIMIT $2 * 2) e
		 JOIN "values" v ON v.entity_id = e.id
		 WHERE v.space_id = ANY($1::uuid[]) GROUP BY 1, 2 LIMIT $2`,
		[ids, limit],
		60_000,
	)
	const random = await pg.read<{entity_id: string; space_id: string}>(
		`SELECT DISTINCT entity_id::text AS entity_id, space_id::text AS space_id
		 FROM "values" TABLESAMPLE SYSTEM (0.1) WHERE space_id = ANY($1::uuid[]) LIMIT $2`,
		[ids, limit],
		60_000,
	)
	const pair = (r: {entity_id: string; space_id: string}, weight = 1): Pair => ({
		entity: compactId(r.entity_id),
		space: compactId(r.space_id),
		weight,
	})
	return {
		top: zipfBySpace(top).map(([r, rank]) => pair(r, 1 / rank)),
		recent: recent.map((r) => pair(r)),
		random: random.map((r) => pair(r)),
	}
}

/** Each row with its 1-based rank by score within its Space. */
export function zipfBySpace<R extends {space_id: string; score: string}>(rows: R[]): [R, number][] {
	const bySpace = new Map<string, R[]>()
	for (const r of rows) {
		const list = bySpace.get(r.space_id)
		if (list) list.push(r)
		else bySpace.set(r.space_id, [r])
	}
	const out: [R, number][] = []
	for (const list of bySpace.values()) {
		list.sort((a, b) => Number(b.score) - Number(a.score))
		list.forEach((r, i) => out.push([r, i + 1]))
	}
	return out
}

function pickWeighted(pairs: Pair[], u: number): Pair {
	const total = pairs.reduce((s, p) => s + p.weight, 0)
	let x = u * total
	for (const p of pairs) {
		x -= p.weight
		if (x < 0) return p
	}
	return pairs[pairs.length - 1] as Pair
}

/** One pair: a pool by its share (empty pools skipped), then a pair within it by weight. */
export function pick(pools: Pools, rand: () => number = Math.random): Pair | null {
	const live = (Object.keys(POOL_SHARE) as (keyof Pools)[]).filter((k) => pools[k].length > 0)
	if (live.length === 0) return null
	const total = live.reduce((s, k) => s + POOL_SHARE[k], 0)
	let x = rand() * total
	let chosen = live[live.length - 1] as keyof Pools
	for (const k of live) {
		x -= POOL_SHARE[k]
		if (x < 0) {
			chosen = k
			break
		}
	}
	return pickWeighted(pools[chosen], rand())
}

export type Fetch = (url: string, init: {signal: AbortSignal}) => Promise<{status: number; text(): Promise<string>}>

export type ReplayCounters = {requests: Series}

/**
 * One request: skips (and counts) a pair whose Space has been mapped out since the pools were built, and
 * never throws.
 */
export async function replayOne(
	target: URL,
	pair: Pair,
	mapped: ReadonlySet<string>,
	counters: ReplayCounters,
	fetchImpl: Fetch,
	timeoutMs = 10_000,
): Promise<string> {
	let result: string
	if (!mapped.has(pair.space)) result = "skipped_unmapped"
	else {
		const url = new URL(`/geodb/entities/${pair.entity}/card`, target)
		url.searchParams.set("spaceId", pair.space)
		try {
			const res = await fetchImpl(url.toString(), {signal: AbortSignal.timeout(timeoutMs)})
			await res.text()
			result = res.status >= 200 && res.status < 300 ? "ok" : "http_error"
		} catch {
			result = "error"
		}
	}
	counters.requests.inc({result})
	return result
}
