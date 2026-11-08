/**
 * The topic page type counts' shadow (GEODB_TYPE_COUNTS_SHADOW, off by default; GAP2-TOPIC-TYPE-COUNTS-TWIN).
 *
 * `public.topic_feed_type_counts` counts the feed entities of a topic per type for the composition strip. geo-db answers the same
 * question from derived collection 1 with DerivedScan's count mode (wire frame revision 13, one topic, no Space filter; the plan and
 * its limits are in `rankedReads/typeCounts.ts`). Each pass takes a few topics from `entity_topic_ranking`, asks Postgres and
 * geo-db the same question and holds the counts equal. Reads stay on Postgres; nothing here touches a request.
 *
 * Both SQL shapes are exercised: `match_all` with no debate swap, and a union walk with `debate_tagged_claims` (Claim asked, Debate
 * walked). The types asked are the topic's own types plus Claim and a type the topic does not hold.
 *
 * Needs the serve AND the geodb-client binary at frame revision 13: an older serve refuses the mode, and an older client has no
 * `counts` line, either way the read shows as `failed`. A refusal kills the held pipe worker, which restarts on the next read.
 *
 * `gaia_api_type_counts_shadow_total{result}`:
 *  - match: same types, same counts;
 *  - mismatch: they differ. A write between the two reads is a benign mismatch: read the rate, not one count;
 *  - not_served: the call is outside count mode (the blocklist is longer than 256);
 *  - failed: a read failed, or the node is below frame revision 13.
 *
 * Counts only: no topic, type or entity is logged or labelled.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {GeoDbError} from "./client"
import type {DerivedCountsReader} from "./derived"
import {CLAIM_TYPE} from "./rankedReads/feeds"
import {planTypeCounts, type TypeCountsCall, typeCountsFrom} from "./rankedReads/typeCounts"

export const RESULTS = ["match", "mismatch", "not_served", "failed"] as const
export type Result = (typeof RESULTS)[number]

/** Derived collection 1: the topic feed (`entity_topic_ranking`), keyed by topic. */
export const COLLECTION = 1

export type TypeCountsShadowConfig = {enabled: boolean; intervalMs: number; topics: number}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readTypeCountsShadowConfig(env: Record<string, string | undefined>): TypeCountsShadowConfig {
	const flag = env.GEODB_TYPE_COUNTS_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		intervalMs: wholeNumber(env.GEODB_TYPE_COUNTS_SHADOW_INTERVAL_MS, 300_000, 86_400_000),
		topics: wholeNumber(env.GEODB_TYPE_COUNTS_SHADOW_TOPICS, 5, 50),
	}
}

/** A type no topic holds, so the answer's "zero count is omitted" case is in every pass. */
const ABSENT_TYPE = "00000000-0000-4000-8000-000000000001"

/** Where the shadow gets its topics and the Postgres answers. The default reads Postgres; tests inject. */
export type TypeCountsSource = {
	/** Up to `limit` topic ids that hold rows, from a random position. */
	sampleTopics(limit: number): Promise<string[]>
	/** The distinct types a topic holds (a few). */
	typesOf(topic: string): Promise<string[]>
	/** The blocklisted entity ids (uuid text). */
	blocklist(): Promise<string[]>
	/** `topic_feed_type_counts` for the call: type id to entity count. */
	pgCounts(call: TypeCountsCall): Promise<Map<string, number>>
}

let active = false
const results = new Map<Result, number>()
const bump = (r: Result): void => void results.set(r, (results.get(r) ?? 0) + 1)

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderTypeCountsShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_type_counts_shadow_total Sampled topic_feed_type_counts calls against geo-db's DerivedScan count mode (collection 1), by result: match, mismatch (a write between the reads is benign), not_served (outside count mode) or failed (a read failed, or the node is below frame revision 13).",
		"# TYPE gaia_api_type_counts_shadow_total counter",
	]
	for (const r of RESULTS) lines.push(`gaia_api_type_counts_shadow_total{result="${r}"} ${results.get(r) ?? 0}`)
	return lines.join("\n")
}

export function __resetTypeCountsShadowMetricsForTests(): void {
	active = false
	results.clear()
}

const sameCounts = (a: Map<string, number>, b: Map<string, number>) =>
	a.size === b.size && [...a].every(([type, n]) => b.get(type) === n)

export type TypeCountsShadow = {
	/** One sampling pass. Resolves when done; never throws. */
	runOnce(): Promise<void>
	/** Start the periodic pass; returns the stop function. The timer does not keep the process alive. */
	start(): () => void
}

export function createTypeCountsShadow(deps: {
	config: TypeCountsShadowConfig
	source: TypeCountsSource
	reader: DerivedCountsReader
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
}): TypeCountsShadow {
	const {config, source, reader, log} = deps
	active = true
	let running = false
	let logged = 0

	async function check(call: TypeCountsCall, blocked: string[]): Promise<void> {
		try {
			const plan = planTypeCounts(call, blocked.length)
			if (plan === null) return bump("not_served")
			const pg = await source.pgCounts(call)
			const served = await reader.typeCounts(
				COLLECTION,
				plan.group,
				plan.walked,
				blocked.map((id) => id.replace(/-/g, "").toLowerCase()),
			)
			if (sameCounts(pg, typeCountsFrom(plan, served.counts))) return bump("match")
			bump("mismatch")
			if (logged < 20) {
				logged++
				log.warn("type counts shadow disagreement", {matchAll: call.matchAll, debate: call.debateTaggedClaims})
			}
		} catch (error) {
			bump("failed")
			if (!(error instanceof GeoDbError)) log.warn("type counts shadow failed", {error: String(error)})
		}
	}

	async function runOnce(): Promise<void> {
		if (running) return
		running = true
		try {
			let topics: string[]
			let blocked: string[]
			try {
				topics = await source.sampleTopics(config.topics)
				blocked = await source.blocklist()
			} catch (error) {
				bump("failed")
				log.warn("type counts shadow failed", {error: String(error)})
				return
			}
			for (const topic of topics) {
				let held: string[]
				try {
					held = await source.typesOf(topic)
				} catch (error) {
					bump("failed")
					log.warn("type counts shadow failed", {error: String(error)})
					continue
				}
				const types = [...new Set([...held, CLAIM_TYPE, ABSENT_TYPE])]
				await check({topics: [topic], types, spaces: null, matchAll: true, debateTaggedClaims: false}, blocked)
				await check({topics: [topic], types, spaces: null, matchAll: false, debateTaggedClaims: true}, blocked)
			}
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

function rowsOf(result: unknown): Record<string, unknown>[] {
	return ((result as {rows?: Record<string, unknown>[]}).rows ?? []) as Record<string, unknown>[]
}

const uuidArray = (ids: string[]) =>
	sql`ARRAY[${sql.join(
		ids.map((id) => sql`${id}::uuid`),
		sql`, `,
	)}]::uuid[]`

/** The default source: Postgres. */
export function createPostgresTypeCountsSource(db: Database): TypeCountsSource {
	const topicsFrom = async (from: string | null, n: number) =>
		rowsOf(
			await db.execute(
				from === null
					? sql`SELECT DISTINCT topic_id::text AS id FROM (SELECT topic_id FROM public.entity_topic_ranking ORDER BY topic_id LIMIT ${n}) t`
					: sql`SELECT DISTINCT topic_id::text AS id FROM (SELECT topic_id FROM public.entity_topic_ranking WHERE topic_id >= ${from}::uuid ORDER BY topic_id LIMIT ${n}) t`,
			),
		).map((r) => String(r.id))
	return {
		async sampleTopics(limit) {
			// Per-topic row count skews small, so ask for rows and keep the distinct topics: a big topic fills the page alone.
			const first = await topicsFrom(crypto.randomUUID(), limit)
			return first.length > 0 ? first.slice(0, limit) : (await topicsFrom(null, limit)).slice(0, limit)
		},
		async typesOf(topic) {
			return rowsOf(
				await db.execute(
					sql`SELECT DISTINCT type_id::text AS id FROM public.entity_topic_ranking WHERE topic_id = ${topic}::uuid LIMIT 8`,
				),
			).map((r) => String(r.id))
		},
		async blocklist() {
			return rowsOf(
				await db.execute(sql`SELECT entity_id::text AS id FROM public.entity_feed_blocklist LIMIT 300`),
			).map((r) => String(r.id))
		},
		async pgCounts(call) {
			const rows = rowsOf(
				await db.execute(
					sql`SELECT type_id::text AS type_id, entity_count::text AS entity_count FROM public.topic_feed_type_counts(${uuidArray(call.topics)}, ${uuidArray(call.types)}, NULL, ${call.matchAll}, ${call.debateTaggedClaims})`,
				),
			)
			return new Map(rows.map((r) => [String(r.type_id), Number(r.entity_count)]))
		},
	}
}
