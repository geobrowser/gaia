/**
 * The gateway probe (GATEWAY_PROBE, off by default).
 *
 * `metas` and `relation` are gateway fields in mode "shadow", but the web app asks for them rarely (a page's freshness
 * sentinel, a single relation by id) and the shadow compares only a sampled fraction (GRAPHQL_GATEWAY_SHADOW_SAMPLE,
 * 1%) of what it sees, so their counters stay near zero. This asks for them itself: every `intervalMs`, in the
 * background, it sends one `metas` operation and up to `relations` `relation(id:)` operations (ids sampled from
 * Postgres from a random primary-key position, wrapping to the start) through a second gateway built from the same
 * shadows, with the sample rate forced to 1. Each answer from PostGraphile is then compared by the existing shadow code
 * and counted in `gaia_api_gateway_shadow_total{field}` like any request's. Nothing a client sees changes, and the
 * probe's own errors are swallowed.
 */
import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"

export type GatewayProbeConfig = {enabled: boolean; intervalMs: number; relations: number}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readGatewayProbeConfig(env: Record<string, string | undefined>): GatewayProbeConfig {
	const flag = env.GATEWAY_PROBE?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		intervalMs: wholeNumber(env.GATEWAY_PROBE_INTERVAL_MS, 60_000, 86_400_000),
		relations: wholeNumber(env.GATEWAY_PROBE_RELATIONS, 3, 20),
	}
}

export const METAS_QUERY = "query ProbeMetas { metas { nodes { id cursor blockNumber } } }"
export const RELATION_QUERY =
	"query ProbeRelation($id: UUID!) { relation(id: $id) { id spaceId fromEntityId typeId toEntityId entityId } }"

/** Up to `limit` relation ids from the primary-key position `pivot` (a UUID), wrapping to the start when short. */
export async function sampleRelationIds(
	db: NodePgDatabase<Record<string, unknown>>,
	pivot: string,
	limit: number,
): Promise<string[]> {
	const read = async (from: string | null, n: number): Promise<string[]> => {
		const result = await db.execute(
			from === null
				? sql`SELECT id::text FROM relations ORDER BY id LIMIT ${n}`
				: sql`SELECT id::text FROM relations WHERE id >= ${from}::uuid ORDER BY id LIMIT ${n}`,
		)
		return (((result as {rows?: unknown}).rows as {id: unknown}[] | undefined) ?? []).flatMap((r) =>
			typeof r.id === "string" ? [r.id] : [],
		)
	}
	const first = await read(pivot, limit)
	if (first.length >= limit) return first
	const seen = new Set(first)
	return first.concat((await read(null, limit)).filter((id) => !seen.has(id)).slice(0, limit - first.length))
}

const post = (query: string, variables?: Record<string, unknown>) =>
	new Request("http://gateway-probe.invalid/graphql", {
		method: "POST",
		headers: {"content-type": "application/json", "x-gateway-probe": "1"},
		body: JSON.stringify({query, ...(variables ? {variables} : {})}),
	})

export type GatewayProbe = {
	runOnce(): Promise<void>
	start(): () => void
}

export function createGatewayProbe(deps: {
	config: GatewayProbeConfig
	db: NodePgDatabase<Record<string, unknown>>
	/** The gateway handler with its shadow sample rate forced to 1. */
	send: (request: Request) => Promise<Response> | Response
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	random?: () => string
}): GatewayProbe {
	const {config, db, send, log} = deps
	const pivotOf = deps.random ?? (() => crypto.randomUUID())
	let running = false

	const ask = async (request: Request) => {
		try {
			// Drain the body: the shadow compares a clone, and an unread response would hold its stream.
			await (await send(request)).text()
		} catch (error) {
			log.warn("gateway probe request failed", {error: String(error)})
		}
	}

	async function runOnce(): Promise<void> {
		if (running) return
		running = true
		try {
			await ask(post(METAS_QUERY))
			let ids: string[] = []
			try {
				ids = await sampleRelationIds(db, pivotOf(), config.relations)
			} catch (error) {
				log.warn("gateway probe sample failed", {error: String(error)})
			}
			for (const id of ids) await ask(post(RELATION_QUERY, {id}))
		} finally {
			running = false
		}
	}

	return {
		runOnce,
		start() {
			if (!config.enabled) return () => {}
			const timer = setInterval(() => void runOnce().catch(() => {}), config.intervalMs)
			timer.unref?.()
			return () => clearInterval(timer)
		},
	}
}
