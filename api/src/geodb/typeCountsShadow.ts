/**
 * The topic page type counts' shadow (GEODB_TYPE_COUNTS_SHADOW, off by default; GAP2-TOPIC-TYPE-COUNTS-TWIN).
 *
 * `public.topic_feed_type_counts` counts the feed entities of a topic per type for the composition strip. geo-db answers the same
 * question from derived collection 1 with DerivedScan's count mode (wire frame revision 13 one topic, 14 several topics, 15 the Space
 * filter as a graph block; the plan and its limits are in `rankedReads/typeCounts.ts`). Each pass takes a few topics from
 * `entity_topic_ranking`, asks Postgres and geo-db the same question and holds the counts equal. Reads stay on Postgres; nothing
 * here touches a request.
 *
 * Both SQL shapes are exercised: `match_all` with no debate swap, and a union walk with `debate_tagged_claims` (Claim asked, Debate
 * walked). The types asked are the topic's own types plus Claim and a type the topic does not hold. Each is asked bare, with a Space
 * filter (a few Spaces holding the topic's names, mapped to geo-db's ids by `geoSpaceOf`), and, once a pass holds two topics, over both.
 *
 * Needs the serve AND the geodb-client binary at the frame revision of the form asked (13, 14, 15). A serve below it answers
 * `InvalidWirePayload` (and an older client a malformed line): that is a `fallback`, not a failure and never a mismatch, and the
 * form is not asked again for `FORM_BACKOFF_MS`, because a refusal kills the held pipe worker. The live serve is revision 13 today,
 * so with this flag on the graph and several-topic forms only probe. A refusal kills the held pipe worker, which restarts on the next read.
 *
 * `gaia_api_type_counts_shadow_total{result}`:
 *  - match: same types, same counts;
 *  - mismatch: they differ. A write between the two reads is a benign mismatch: read the rate, not one count;
 *  - not_served: the call is outside count mode (the blocklist is longer than 256, no Space of the call has a geo-db twin to send);
 *  - fallback: geo-db declines and Postgres stays the read: more than 64 Spaces, `ResourceExhausted` (the graph's read budget), or a
 *    serve below the form's frame revision (`gaia_api_type_counts_shadow_fallback_total{reason}`);
 *  - failed: a read failed.
 *
 * `gaia_api_type_counts_shadow_failed_total{reason}` splits `failed` by the GeoDbError reason (or `postgres` / `other`), so a failure
 * is told apart without a log line. A geo-db read that times out or loses its worker is tried once more (the pipe restarts on the next
 * read) before it counts as failed: `gaia_api_type_counts_shadow_retries_total{result}` says how often that second try answered.
 * `gaia_api_type_counts_shadow_first_try_failed_total{reason}` counts why the FIRST try failed even when the retry answered, so an
 * idle-pipe failure is told apart from a slow read.
 *
 * Counts only: no topic, type or entity is logged or labelled. A disagreement log names each differing type by a fixed label (`claim`,
 * `debate`, `other`, `absent`) with the Postgres and geo-db counts, never by id.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {GeoDbError} from "./client"
import {countsRefusal, type DerivedCountsReader} from "./derived"
import {CLAIM_TYPE, DEBATE_WALK_TYPE} from "./rankedReads/feeds"
import {
	isFallback,
	NAME_PROPERTY,
	planTypeCounts,
	REVISION_GRAPH,
	REVISION_TOPICS,
	type TypeCountsCall,
	type TypeCountsPlan,
	typeCountsFrom,
} from "./rankedReads/typeCounts"

export const RESULTS = ["match", "mismatch", "not_served", "fallback", "failed"] as const
export type Result = (typeof RESULTS)[number]

export const FAIL_REASONS = [
	"timeout",
	"client_failed",
	"bad_request",
	"overloaded",
	"unmapped_space",
	"postgres",
	"other",
] as const
export type FailReason = (typeof FAIL_REASONS)[number]

/** Derived collection 1: the topic feed (`entity_topic_ranking`), keyed by topic. */
export const COLLECTION = 1

/** How long a form a serve refused for its revision is left alone before one probe asks it again. */
export const FORM_BACKOFF_MS = 600_000

export const FALLBACK_REASONS = ["too_many_spaces", "resource_exhausted", "serve_revision", "backoff"] as const
export type FallbackReason = (typeof FALLBACK_REASONS)[number]

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
	/** Up to `limit` Spaces (gaia uuid text) that hold a Name value of an entity the topic holds. */
	sampleSpaces(topic: string, limit: number): Promise<string[]>
	/** `topic_feed_type_counts` for the call: type id to entity count. */
	pgCounts(call: TypeCountsCall): Promise<Map<string, number>>
}

let active = false
const results = new Map<Result, number>()
const bump = (r: Result): void => void results.set(r, (results.get(r) ?? 0) + 1)
const failReasons = new Map<FailReason, number>()
const fallbacks = new Map<FallbackReason, number>()
const retries = {answered: 0, failed: 0}
const firstTryFailures = new Map<FailReason, number>()

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderTypeCountsShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_type_counts_shadow_total Sampled topic_feed_type_counts calls against geo-db's DerivedScan count mode (collection 1), by result: match, mismatch (a write between the reads is benign), not_served (outside count mode), fallback (geo-db declines and Postgres stays the read: too many Spaces, ResourceExhausted, or a serve below the form's frame revision) or failed (a read failed).",
		"# TYPE gaia_api_type_counts_shadow_total counter",
	]
	for (const r of RESULTS) lines.push(`gaia_api_type_counts_shadow_total{result="${r}"} ${results.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_type_counts_shadow_fallback_total Type-counts shadow calls that fell back to Postgres, by reason: too_many_spaces (past 64), resource_exhausted (the serve's read budget), serve_revision (the serve or client is below the form's frame revision 14 or 15) or backoff (the form was refused for its revision recently and is not asked yet).",
		"# TYPE gaia_api_type_counts_shadow_fallback_total counter",
	)
	for (const r of FALLBACK_REASONS)
		lines.push(`gaia_api_type_counts_shadow_fallback_total{reason="${r}"} ${fallbacks.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_type_counts_shadow_failed_total Failed type-counts shadow calls by reason: a GeoDbError reason, postgres (the SQL side threw) or other.",
		"# TYPE gaia_api_type_counts_shadow_failed_total counter",
	)
	for (const r of FAIL_REASONS)
		lines.push(`gaia_api_type_counts_shadow_failed_total{reason="${r}"} ${failReasons.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_type_counts_shadow_retries_total The one retry of a geo-db read that timed out or lost its worker, by whether the second try answered.",
		"# TYPE gaia_api_type_counts_shadow_retries_total counter",
		`gaia_api_type_counts_shadow_retries_total{result="answered"} ${retries.answered}`,
		`gaia_api_type_counts_shadow_retries_total{result="failed"} ${retries.failed}`,
	)
	lines.push(
		"# HELP gaia_api_type_counts_shadow_first_try_failed_total Why the FIRST try of a geo-db read failed, whether or not the retry then answered (failed_total only sees the last try).",
		"# TYPE gaia_api_type_counts_shadow_first_try_failed_total counter",
	)
	for (const r of FAIL_REASONS)
		if (r !== "postgres")
			lines.push(
				`gaia_api_type_counts_shadow_first_try_failed_total{reason="${r}"} ${firstTryFailures.get(r) ?? 0}`,
			)
	return lines.join("\n")
}

export function __resetTypeCountsShadowMetricsForTests(): void {
	active = false
	results.clear()
	failReasons.clear()
	fallbacks.clear()
	retries.answered = 0
	retries.failed = 0
	firstTryFailures.clear()
}

const sameCounts = (a: Map<string, number>, b: Map<string, number>) =>
	a.size === b.size && [...a].every(([type, n]) => b.get(type) === n)

/** A fixed label for a type, so a log line says which kind differs without naming an entity. */
function typeLabel(type: string, held: Set<string>): string {
	if (type === CLAIM_TYPE) return "claim"
	if (type === DEBATE_WALK_TYPE) return "debate"
	if (type === ABSENT_TYPE) return "absent"
	return held.has(type) ? "other" : "unknown"
}

export type TypeDiff = {type: string; postgres: number; geodb: number}

/** Every requested type whose two counts differ, by label (types sharing a label are summed); zero is a count like any other. */
export function diffTypeCounts(pg: Map<string, number>, geo: Map<string, number>, held: Set<string>): TypeDiff[] {
	const out = new Map<string, TypeDiff>()
	for (const type of new Set([...pg.keys(), ...geo.keys()])) {
		const a = pg.get(type) ?? 0
		const b = geo.get(type) ?? 0
		if (a === b) continue
		const label = typeLabel(type, held)
		const seen = out.get(label) ?? {type: label, postgres: 0, geodb: 0}
		seen.postgres += a
		seen.geodb += b
		out.set(label, seen)
	}
	return [...out.values()]
}

/** At most `max` events in any `windowMs`: a log line that cannot flood. */
function limiter(max: number, windowMs: number, now: () => number): () => boolean {
	const at: number[] = []
	return () => {
		const t = now()
		while (at.length > 0 && t - (at[0] as number) >= windowMs) at.shift()
		if (at.length >= max) return false
		at.push(t)
		return true
	}
}

const reasonOf = (error: unknown): FailReason => (error instanceof GeoDbError ? error.reason : "other")

const RETRYABLE = new Set<string>(["timeout", "client_failed"])

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
	/** gaia Space id (compact) to geo-db's, as the card reads map it (`createSpaceMap`). Without it no Space filter is asked. */
	geoSpaceOf?: (gaiaSpaceId: string) => Promise<string | null>
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): TypeCountsShadow {
	const {config, source, reader, log} = deps
	active = true
	let running = false
	const mayLogDisagreement = limiter(10, 600_000, deps.now ?? Date.now)
	const mayLogFailure = limiter(5, 600_000, deps.now ?? Date.now)
	let suppressed = 0
	const clock = deps.now ?? Date.now
	/** Geo-db reads made so far in this pass, when the last one ended, and the gap before the latest read: where a failure falls. */
	let position = 0
	let lastReadEnd: number | null = null
	let idleBefore: number | null = null

	/** When each form may be asked again after a serve refused it for its revision (a revision 13 serve refuses 14 and 15). */
	const refusedUntil = {topics: 0, graph: 0}
	const formBlocked = (form: number): boolean =>
		(form >= REVISION_GRAPH && clock() < refusedUntil.graph) ||
		(form >= REVISION_TOPICS && clock() < refusedUntil.topics)

	const fell = (reason: FallbackReason): void => {
		bump("fallback")
		fallbacks.set(reason, (fallbacks.get(reason) ?? 0) + 1)
	}

	const failed = (reason: FailReason, error: unknown): void => {
		bump("failed")
		failReasons.set(reason, (failReasons.get(reason) ?? 0) + 1)
		if (!mayLogFailure()) {
			suppressed++
			return
		}
		// A GeoDbError message is the client's own words (a refusal, a malformed row, a worker's stderr tail), not a request.
		log.warn("type counts shadow failed", {
			reason,
			message: (error instanceof Error ? error.message : String(error)).slice(0, 200),
			suppressed,
			position,
			idleMs: idleBefore,
		})
		suppressed = 0
	}

	/** One geo-db read; a timeout or lost worker is tried once more, as the pipe restarts on the next read. */
	async function read(plan: TypeCountsPlan, blocked: string[]) {
		try {
			return await readOnce(plan, blocked)
		} finally {
			lastReadEnd = clock()
		}
	}

	async function readOnce(plan: TypeCountsPlan, blocked: string[]) {
		const ask = () =>
			reader.typeCounts(
				COLLECTION,
				plan.group,
				plan.walked,
				blocked.map((id) => id.replace(/-/g, "").toLowerCase()),
				plan.form === 13 ? undefined : {others: plan.others, matchAll: plan.matchAll, graph: plan.graph},
			)
		position++
		idleBefore = lastReadEnd === null ? null : clock() - lastReadEnd
		try {
			return await ask()
		} catch (error) {
			// A refusal is the serve's answer, not a flake: do not ask it twice.
			if (countsRefusal(error) !== null) throw error
			const why = reasonOf(error)
			firstTryFailures.set(why, (firstTryFailures.get(why) ?? 0) + 1)
			if (!(error instanceof GeoDbError) || !RETRYABLE.has(error.reason)) throw error
			try {
				const second = await ask()
				retries.answered++
				return second
			} catch (again) {
				retries.failed++
				throw again
			}
		}
	}

	async function check(call: TypeCountsCall, blocked: string[], held: Set<string>): Promise<void> {
		let pg: Map<string, number>
		try {
			let geoSpaces: string[] | null = null
			if (call.spaces !== null) {
				if (!deps.geoSpaceOf) return bump("not_served")
				if (call.spaces.length > 64) return fell("too_many_spaces")
				geoSpaces = []
				for (const space of call.spaces) {
					const geo = await deps.geoSpaceOf(space.replace(/-/g, "").toLowerCase())
					if (geo === null)
						return failed("unmapped_space", new Error("a Space of the call has no geo-db twin"))
					geoSpaces.push(geo)
				}
			}
			const plan = planTypeCounts(call, blocked.length, geoSpaces)
			if (plan === null) return bump("not_served")
			if (isFallback(plan)) return fell(plan.fallback)
			if (formBlocked(plan.form)) return fell("backoff")
			try {
				pg = await source.pgCounts(call)
			} catch (error) {
				return failed("postgres", error)
			}
			let served: Awaited<ReturnType<typeof read>>
			try {
				served = await read(plan, blocked)
			} catch (error) {
				const refusal = countsRefusal(error)
				if (refusal === "exhausted") return fell("resource_exhausted")
				if (refusal === "revision" && plan.form >= REVISION_TOPICS) {
					if (plan.form >= REVISION_GRAPH) refusedUntil.graph = clock() + FORM_BACKOFF_MS
					else refusedUntil.topics = clock() + FORM_BACKOFF_MS
					return fell("serve_revision")
				}
				return failed(reasonOf(error), error)
			}
			const geo = typeCountsFrom(plan, served.counts)
			if (sameCounts(pg, geo)) return bump("match")
			bump("mismatch")
			if (mayLogDisagreement()) {
				const diffs = diffTypeCounts(pg, geo, held)
				log.warn("type counts shadow disagreement", {
					matchAll: call.matchAll,
					debate: call.debateTaggedClaims,
					topics: call.topics.length,
					spaces: call.spaces?.length ?? 0,
					types: pg.size,
					walked: served.walked,
					diffs,
				})
			}
		} catch (error) {
			failed(reasonOf(error), error)
		}
	}

	async function runOnce(): Promise<void> {
		if (running) return
		running = true
		position = 0
		try {
			let topics: string[]
			let blocked: string[]
			try {
				topics = await source.sampleTopics(config.topics)
				blocked = await source.blocklist()
			} catch (error) {
				failed("postgres", error)
				return
			}
			const pairs: [boolean, boolean][] = [
				[true, false],
				[false, true],
			]
			type Held = {topic: string; types: string[]; held: Set<string>}
			let first: Held | null = null
			let second: Held | null = null
			for (const topic of topics) {
				let held: string[]
				let spaces: string[] = []
				try {
					held = await source.typesOf(topic)
					if (deps.geoSpaceOf) spaces = await source.sampleSpaces(topic, 3)
				} catch (error) {
					failed("postgres", error)
					continue
				}
				const types = [...new Set([...held, CLAIM_TYPE, ABSENT_TYPE])]
				const heldSet = new Set(held)
				const here = {topic, types, held: heldSet}
				if (first === null) first = here
				else second ??= here
				for (const [matchAll, debateTaggedClaims] of pairs) {
					await check({topics: [topic], types, spaces: null, matchAll, debateTaggedClaims}, blocked, heldSet)
					if (spaces.length > 0)
						await check({topics: [topic], types, spaces, matchAll, debateTaggedClaims}, blocked, heldSet)
				}
			}
			// Several topics: the rev 14 form, pooled and `match_all`, over the first topic's types (a pair that holds nothing in common
			// is a zero answer on both sides, which is also a result worth holding).
			if (first !== null && second !== null) {
				const both = [first.topic, second.topic]
				for (const matchAll of [true, false])
					await check(
						{topics: both, types: first.types, spaces: null, matchAll, debateTaggedClaims: false},
						blocked,
						first.held,
					)
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
		async sampleSpaces(topic, limit) {
			// A few entities the topic holds, then the Spaces their Name values sit in: the ones the Space filter is about.
			return rowsOf(
				await db.execute(
					sql`SELECT DISTINCT v.space_id::text AS id FROM public.values v WHERE v.property_id = ${NAME_PROPERTY}::uuid AND v.entity_id IN (SELECT entity_id FROM public.entity_topic_ranking WHERE topic_id = ${topic}::uuid LIMIT 20) LIMIT ${limit}`,
				),
			).map((r) => String(r.id))
		},
		async pgCounts(call) {
			const spaces = call.spaces === null ? sql`NULL::uuid[]` : uuidArray(call.spaces)
			const rows = rowsOf(
				await db.execute(
					sql`SELECT type_id::text AS type_id, entity_count::text AS entity_count FROM public.topic_feed_type_counts(${uuidArray(call.topics)}, ${uuidArray(call.types)}, ${spaces}, ${call.matchAll}, ${call.debateTaggedClaims})`,
				),
			)
			return new Map(rows.map((r) => [String(r.type_id), Number(r.entity_count)]))
		},
	}
}
