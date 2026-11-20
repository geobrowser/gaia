/**
 * The `search`, `searchConnection` and `relation` shadows, driven by a timer instead of by request traffic.
 *
 * Why a timer. The gateway shadows for these three fields only run when a client sends them as the single root of an
 * operation, and the web app almost never does (`search` goes through the REST endpoint; `relation(id:)` is rare), so
 * `gaia_api_gateway_shadow_total{field="search" | "searchConnection" | "relation"}` showed no samples in 24 hours or 7 days.
 * GATEWAY_PROBE already does this for `relation` and `metas` but is off and cannot sample search text.
 *
 * What a pass does, every GEODB_SEARCH_RELATION_SHADOW_INTERVAL_MS (default 5 minutes): read up to `sample` real entity
 * names and `sample` real relation ids from Postgres, then send each name as a `search` and a `searchConnection`
 * operation and each id as a `relation` operation through a second gateway built from the same shadows with the shadow
 * sample rate forced to 1 (the same one GATEWAY_PROBE uses). PostGraphile answers those operations, and the existing
 * shadow code compares each answer with geo-db and counts it into the existing gaia_api_gateway_shadow_total,
 * gaia_api_gateway_search_* families. No new metric family. Postgres stays the answer; nothing is served from geo-db.
 *
 * Load per pod per pass (default sample 3): two small indexed SELECTs, 6 PostGraphile search operations (3 names x 2
 * fields), 3 relation lookups, and at most 6 geo-db lexical searches (the search client's own concurrency limit applies).
 * That is 9 operations per 5 minutes, sequentially, about 0.03 a second. GEODB_SEARCH_RELATION_SHADOW defaults to on;
 * only an explicit 0 / off / false turns it off.
 *
 * A Postgres failure (the samples, or PostGraphile's answer to an operation) is logged and counts nothing against
 * geo-db: the shadows skip an answer that is not comparable. Logs counts only, never a name or an id.
 */

import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {RELATION_QUERY, sampleRelationIds} from "../gateway/probe"
import {createIntervalSampler, DEFAULT_SAMPLER_INTERVAL_MS, type IntervalSampler} from "./intervalSampler"

type Database = NodePgDatabase<Record<string, unknown>>

export type SearchRelationShadowConfig = {enabled: boolean; intervalMs: number; sample: number}

const MAX_SAMPLE = 10
const DEFAULT_SAMPLE = 3
const MIN_QUERY_LENGTH = 2
const MAX_QUERY_LENGTH = 250
const NAME_PROPERTY_ID = "a126ca53-0c8e-48d5-b888-82c734c38935"

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

/** Default ON; `GEODB_SEARCH_RELATION_SHADOW` of 0, off or false disables it. */
export function readSearchRelationShadowConfig(env: Record<string, string | undefined>): SearchRelationShadowConfig {
	const flag = env.GEODB_SEARCH_RELATION_SHADOW?.trim().toLowerCase()
	return {
		enabled: !(flag === "0" || flag === "off" || flag === "false"),
		intervalMs: wholeNumber(env.GEODB_SEARCH_RELATION_SHADOW_INTERVAL_MS, DEFAULT_SAMPLER_INTERVAL_MS, 86_400_000),
		sample: wholeNumber(env.GEODB_SEARCH_RELATION_SHADOW_SAMPLE, DEFAULT_SAMPLE, MAX_SAMPLE),
	}
}

export const SEARCH_QUERY = "query ProbeSearch($q: String!) { search(query: $q, first: 10) { id } }"
export const SEARCH_CONNECTION_QUERY =
	"query ProbeSearchConnection($q: String!) { searchConnection(query: $q, first: 10) { nodes { id } } }"

const usable = (text: string): boolean =>
	text.length >= MIN_QUERY_LENGTH && text.length <= MAX_QUERY_LENGTH && !text.startsWith("-")

/** Up to `limit` distinct entity names from the primary-key position `pivot`, wrapping to the start when short. */
export async function sampleSearchQueries(db: Database, pivot: string, limit: number): Promise<string[]> {
	const read = async (from: string | null): Promise<string[]> => {
		const over = limit * 4
		const result = await db.execute(
			from === null
				? sql`SELECT v.text FROM public.values v WHERE v.property_id = ${NAME_PROPERTY_ID}::uuid AND v.text IS NOT NULL
					ORDER BY v.id LIMIT ${over}`
				: sql`SELECT v.text FROM public.values v WHERE v.property_id = ${NAME_PROPERTY_ID}::uuid AND v.text IS NOT NULL
					AND v.id >= ${from} ORDER BY v.id LIMIT ${over}`,
		)
		const rows = ((result as {rows?: unknown}).rows as {text: unknown}[] | undefined) ?? []
		return rows.flatMap((r) => (typeof r.text === "string" ? [r.text.trim()] : []))
	}
	const seen = new Map<string, string>()
	for (const from of [pivot, null])
		for (const text of await read(from)) {
			if (seen.size >= limit) break
			if (usable(text) && !seen.has(text.toLowerCase())) seen.set(text.toLowerCase(), text)
		}
	return [...seen.values()].slice(0, limit)
}

const post = (query: string, variables: Record<string, unknown>) =>
	new Request("http://search-relation-shadow.invalid/graphql", {
		method: "POST",
		headers: {"content-type": "application/json", "x-gateway-probe": "1"},
		body: JSON.stringify({query, variables}),
	})

export type SearchRelationShadow = IntervalSampler

export function createSearchRelationShadow(deps: {
	config: SearchRelationShadowConfig
	db: Database
	/** The gateway handler with its shadow sample rate forced to 1. */
	send: (request: Request) => Promise<Response> | Response
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	random?: () => string
	sampleQueries?: typeof sampleSearchQueries
	sampleIds?: typeof sampleRelationIds
}): SearchRelationShadow {
	const {config, db, send, log} = deps
	const pivotOf = deps.random ?? (() => crypto.randomUUID())
	const sampleQueries = deps.sampleQueries ?? sampleSearchQueries
	const sampleIds = deps.sampleIds ?? sampleRelationIds

	const ask = async (request: Request) => {
		try {
			// Drain the body: the shadow compares a clone, and an unread response would hold its stream.
			await (await send(request)).text()
		} catch (error) {
			log.warn("search relation shadow request failed", {error: String(error)})
		}
	}

	async function pass(): Promise<void> {
		let queries: string[] = []
		let ids: string[] = []
		try {
			queries = await sampleQueries(db, pivotOf(), config.sample)
		} catch (error) {
			log.warn("search relation shadow: the Postgres name sample failed", {error: String(error)})
		}
		try {
			ids = await sampleIds(db, pivotOf(), config.sample)
		} catch (error) {
			log.warn("search relation shadow: the Postgres relation sample failed", {error: String(error)})
		}
		for (const q of queries) {
			await ask(post(SEARCH_QUERY, {q}))
			await ask(post(SEARCH_CONNECTION_QUERY, {q}))
		}
		for (const id of ids) await ask(post(RELATION_QUERY, {id}))
	}

	return createIntervalSampler({
		enabled: config.enabled,
		intervalMs: config.intervalMs,
		pass,
		log,
		label: "search relation shadow",
	})
}
