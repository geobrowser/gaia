/**
 * The governance shadow's sampler (GEODB_GOVERNANCE_SHADOW_SAMPLER, off by default).
 *
 * The governance shadow runs only when a request serves a proposal (`GET /proposals/:id`, the list), and the web app
 * reads proposals through another path, so on a quiet api it compares next to nothing. This drives it from a timer:
 * every `intervalMs`, in the background, it picks up to `sample` proposals of Postgres from a random primary-key
 * position (wrapping to the start), loads each as the detail route does (`getProposalWithVotes`) and hands it to the
 * shadow's `detail`, which compares and counts it in `gaia_api_governance_shadow_total{surface="detail"}`. It never
 * touches a request and swallows every error. Needs GEODB_GOVERNANCE_SHADOW on: the shadow it feeds is that one.
 */

import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import {getProposalWithVotes} from "../queries"
import {getCurrentTimeSeconds} from "../status"
import type {GovernanceShadow} from "./shadow"

export type GovernanceSamplerConfig = {enabled: boolean; intervalMs: number; sample: number}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readGovernanceSamplerConfig(env: Record<string, string | undefined>): GovernanceSamplerConfig {
	const flag = env.GEODB_GOVERNANCE_SHADOW_SAMPLER?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		intervalMs: wholeNumber(env.GEODB_GOVERNANCE_SHADOW_SAMPLER_INTERVAL_MS, 60_000, 86_400_000),
		sample: wholeNumber(env.GEODB_GOVERNANCE_SHADOW_SAMPLER_SAMPLE, 5, 50),
	}
}

/** Up to `limit` proposal ids from the primary-key position `pivot` (a UUID), wrapping to the start when short. */
export async function sampleProposalIds(
	db: NodePgDatabase<Record<string, unknown>>,
	pivot: string,
	limit: number,
): Promise<string[]> {
	const read = async (from: string | null, n: number): Promise<string[]> => {
		const result = await db.execute(
			from === null
				? sql`SELECT id::text FROM proposals ORDER BY id LIMIT ${n}`
				: sql`SELECT id::text FROM proposals WHERE id >= ${from}::uuid ORDER BY id LIMIT ${n}`,
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

export type GovernanceSampler = {
	runOnce(): Promise<void>
	start(): () => void
}

export function createGovernanceSampler(deps: {
	config: GovernanceSamplerConfig
	db: NodePgDatabase<Record<string, unknown>>
	shadow: GovernanceShadow
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	random?: () => string
}): GovernanceSampler {
	const {config, db, shadow, log} = deps
	const pivotOf = deps.random ?? (() => crypto.randomUUID())
	let running = false

	async function runOnce(): Promise<void> {
		if (running) return
		running = true
		try {
			const ids = await sampleProposalIds(db, pivotOf(), config.sample)
			for (const id of ids) {
				const proposal = await Effect.runPromise(getProposalWithVotes(db, id))
				if (proposal) shadow.detail(proposal, getCurrentTimeSeconds())
			}
		} catch (error) {
			log.warn("governance sampler failed", {error: String(error)})
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
