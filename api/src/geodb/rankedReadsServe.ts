/**
 * The ranked reads' serve switch (GEODB_RANKED_READS_SERVE, off by default; plan 22 section 25.2, SERVE-PATH-RANKED-READS-TYPE-COUNTS).
 *
 * The group B ranked reads have twins (`rankedReads/*`) that the shadows hold to Postgres but never let answer. This is the switch
 * that lets one answer: `GEODB_RANKED_READS_SERVE` is a comma list in the form of GRAPHQL_GATEWAY_FIELDS, `typeCounts`,
 * `bestOrder=on`, `feeds=off`. The entries are `typeCounts`, `bestOrder`, `entityColumns`, `trending` and `feeds`; an entry without
 * `=on` counts as on, `=off` (or anything but `on`) as off, an unknown name is ignored. Unset or empty lists nothing, and a read
 * that is not listed never reaches geo-db.
 *
 * A listed read asks geo-db first (`run`) and answers from Postgres instead when:
 *  - `lag`: geo-db is more than GEODB_RANKED_READS_SERVE_MAX_LAG_BLOCKS chain blocks behind Postgres (default 20);
 *  - `lag_unknown`: that lag cannot be told (no applied-block row, or the Postgres head or the row could not be read), so the
 *    guard fails closed;
 *  - `timeout`: no answer within GEODB_RANKED_READS_SERVE_TIMEOUT_MS (default 250);
 *  - `error` / `overloaded` / `client_failed` / `bad_request`: the geo-db read failed (the GeoDbError reason, else `error`);
 *  - a decline: the twin cannot answer this call exactly (`not_served`, `too_many_spaces`, `unmapped_space`, `serve_revision`,
 *    `resource_exhausted`, `no_source`).
 * A Postgres failure on the fallback is the caller's own: it propagates untouched.
 *
 * `gaia_api_ranked_read_serve_total{read,source}`: which store answered. `gaia_api_ranked_read_serve_fallback_total{read,reason}`:
 * why Postgres answered a listed read. Both are empty until a read is listed.
 *
 * Lag. geo-db's applied position is the feeder's `cursors` row `geodb-feeder-applied` (collection 10, value `<block>:<json>`,
 * `geodb-feeder/src/appliedBlock.ts`, written only under GEODB_APPLIED_BLOCK, which is off today), against `max(block_number)` of
 * `public.edit_versions`, the head search-indexer's `kg_applied_block` reads. Both are cached for GEODB_RANKED_READS_SERVE_CACHE_MS
 * (default 5000) so a request costs no read in the common case.
 *
 * What each entry's twin can serve exactly, and what it cannot, is in the PR; in short, `typeCounts` is held back by the collection 1
 * type_counts repair, `bestOrder` and `entityColumns` differ from Postgres for an entity with no `entity_ranking_scores` row, and
 * `trending` and `feeds` run over rows only Postgres supplies today, so without a geo-db row source they always decline `no_source`.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import type {GeoDbError} from "./client"
import {
	countsRefusal,
	type DerivedCountsReader,
	type DerivedReader,
	type DerivedScoresReader,
	FormGatedError,
	MAX_SCORE_CARDS,
	type ScoreCard,
	type ScoreConfig,
} from "./derived"
import {entitiesInBestOrder} from "./rankedReads/bestOrder"
import {SCORE_COLUMNS, type ScoreColumns, scoreColumns} from "./rankedReads/entityColumns"
import {type FeedData, Feeds} from "./rankedReads/feeds"
import {type Graph, Trending} from "./rankedReads/trending"
import {
	isFallback,
	planTypeCounts,
	REVISION_TOPICS,
	type TypeCountsCall,
	typeCountsFrom,
} from "./rankedReads/typeCounts"
import {readCards, readScoreConfig} from "./rankingScoresShadow"

export const SERVE_READS = ["typeCounts", "bestOrder", "entityColumns", "trending", "feeds"] as const
export type ServeRead = (typeof SERVE_READS)[number]

export const SOURCES = ["geodb", "postgres"] as const
export type Source = (typeof SOURCES)[number]

/** Why Postgres answered a listed read. A closed set, so it is safe as a label. */
export const FALLBACK_REASONS = [
	"lag",
	"lag_unknown",
	"timeout",
	"error",
	"overloaded",
	"client_failed",
	"bad_request",
	"not_served",
	"too_many_spaces",
	"unmapped_space",
	"serve_revision",
	"resource_exhausted",
	"no_source",
] as const
export type FallbackReason = (typeof FALLBACK_REASONS)[number]

export const DEFAULT_TIMEOUT_MS = 250
export const DEFAULT_MAX_LAG_BLOCKS = 20
export const DEFAULT_CACHE_MS = 5000

export type RankedReadsServeConfig = {
	reads: ReadonlySet<ServeRead>
	timeoutMs: number
	maxLagBlocks: number
	cacheMs: number
}

const whole = (raw: string | undefined, fallback: number, min: number, max: number) => {
	if (raw === undefined || raw.trim() === "") return fallback
	const n = Number(raw)
	return Number.isInteger(n) && n >= min && n <= max ? n : fallback
}

/** Parses `read,read=on,read=off`. Pure. */
export function parseServeReads(raw: string | undefined): Set<ServeRead> {
	const reads = new Set<ServeRead>()
	for (const entry of (raw ?? "").split(",")) {
		const [name, mode] = entry.split("=")
		const read = SERVE_READS.find((r) => r === name?.trim())
		if (read === undefined) continue
		const value = mode?.trim().toLowerCase()
		if (mode === undefined || value === "on" || value === "1" || value === "true") reads.add(read)
	}
	return reads
}

export function readRankedReadsServeConfig(env: Record<string, string | undefined>): RankedReadsServeConfig {
	return {
		reads: parseServeReads(env.GEODB_RANKED_READS_SERVE),
		timeoutMs: whole(env.GEODB_RANKED_READS_SERVE_TIMEOUT_MS, DEFAULT_TIMEOUT_MS, 1, 5000),
		maxLagBlocks: whole(env.GEODB_RANKED_READS_SERVE_MAX_LAG_BLOCKS, DEFAULT_MAX_LAG_BLOCKS, 0, 1_000_000),
		cacheMs: whole(env.GEODB_RANKED_READS_SERVE_CACHE_MS, DEFAULT_CACHE_MS, 0, 600_000),
	}
}

/** A call the twin cannot answer exactly: Postgres answers it, and the reason is counted. */
export class ServeDecline extends Error {
	constructor(readonly reason: FallbackReason) {
		super(`ranked read declined: ${reason}`)
	}
}

const isReason = (r: unknown): r is FallbackReason => (FALLBACK_REASONS as readonly unknown[]).includes(r)

/** The fallback reason a failed geo-db read counts under. Pure. */
export function reasonOf(error: unknown): FallbackReason {
	if (error instanceof ServeDecline) return error.reason
	const reason = (error as Partial<GeoDbError> | null)?.reason
	return isReason(reason) ? reason : "error"
}

// ---- metrics ---------------------------------------------------------------------------------------------------------------

let active = false
const served = new Map<string, number>()
const fallbacks = new Map<string, number>()
const bump = (map: Map<string, number>, key: string) => map.set(key, (map.get(key) ?? 0) + 1)

/** Empty until a read is listed, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderRankedReadsServeMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_ranked_read_serve_total Listed ranked reads (GEODB_RANKED_READS_SERVE) by the store that answered.",
		"# TYPE gaia_api_ranked_read_serve_total counter",
	]
	for (const read of SERVE_READS)
		for (const source of SOURCES)
			lines.push(
				`gaia_api_ranked_read_serve_total{read="${read}",source="${source}"} ${served.get(`${read}|${source}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_ranked_read_serve_fallback_total Why Postgres answered a listed ranked read instead of geo-db, by read and reason.",
		"# TYPE gaia_api_ranked_read_serve_fallback_total counter",
	)
	for (const read of SERVE_READS)
		for (const reason of FALLBACK_REASONS)
			lines.push(
				`gaia_api_ranked_read_serve_fallback_total{read="${read}",reason="${reason}"} ${fallbacks.get(`${read}|${reason}`) ?? 0}`,
			)
	return lines.join("\n")
}

export function __resetRankedReadsServeMetricsForTests(): void {
	active = false
	served.clear()
	fallbacks.clear()
}

// ---- the lag guard ---------------------------------------------------------------------------------------------------------

/** How many chain blocks geo-db is behind Postgres, or null when that cannot be told. Never throws. */
export type GeoDbFreshness = {lagBlocks(): Promise<number | null>}

/** The `cursors` collection and the feeder's applied-block key (geodb-feeder/src/appliedBlock.ts). */
export const CURSORS_COLLECTION = 10
export const APPLIED_BLOCK_KEY = "geodb-feeder-applied"
const APPLIED_KEY_HEX = Buffer.from(APPLIED_BLOCK_KEY, "utf8").toString("hex")

/** The block of an applied-block row's value (`<decimal block>:<json>`), or null when malformed. Pure. */
export function appliedBlockOf(value: string): number | null {
	const colon = value.indexOf(":")
	const block = colon < 0 ? value : value.slice(0, colon)
	return /^\d{1,15}$/.test(block) ? Number(block) : null
}

/**
 * Lag from the feeder's applied-block row against Postgres's head, cached for `cacheMs`. A read that fails, a missing row or a
 * missing head is `null` (and cached too, so a broken source is not hammered). A geo-db ahead of the head reads as lag 0.
 */
export function createAppliedBlockFreshness(deps: {
	reader: Pick<DerivedReader, "get">
	/** `max(block_number)` of `public.edit_versions`, null when the table is empty. */
	pgHead: () => Promise<number | null>
	cacheMs: number
	now?: () => number
}): GeoDbFreshness {
	const now = deps.now ?? Date.now
	let at = Number.NEGATIVE_INFINITY
	let cached: number | null = null
	let pending: Promise<number | null> | null = null
	const measure = async (): Promise<number | null> => {
		try {
			const [row, head] = await Promise.all([deps.reader.get(CURSORS_COLLECTION, APPLIED_KEY_HEX), deps.pgHead()])
			if (row === null || head === null) return null
			const applied = appliedBlockOf(row.value.toString("utf8"))
			return applied === null ? null : Math.max(0, head - applied)
		} catch {
			return null
		}
	}
	return {
		async lagBlocks() {
			if (now() - at < deps.cacheMs) return cached
			pending ??= measure().then((lag) => {
				cached = lag
				at = now()
				pending = null
				return lag
			})
			return pending
		},
	}
}

/** `max(block_number)` of `public.edit_versions`: the head search-indexer's kg_applied_block reads. */
export async function readPgHeadBlock(db: Database): Promise<number | null> {
	const result = (await db.execute(sql`SELECT max(block_number)::text AS head FROM public.edit_versions`)) as {
		rows?: {head: string | null}[]
	}
	const head = result.rows?.[0]?.head
	return head === null || head === undefined ? null : Number(head)
}

// ---- the switch ------------------------------------------------------------------------------------------------------------

export type RankedReadsServe = {
	listed(read: ServeRead): boolean
	/**
	 * Answers `read` from `geoDb` when it is listed, geo-db is fresh and it answers in time, and from `postgres` otherwise. `geoDb`
	 * may throw a `ServeDecline` or any error; its `signal` aborts when the budget runs out.
	 */
	run<T>(read: ServeRead, geoDb: (signal: AbortSignal) => Promise<T>, postgres: () => Promise<T>): Promise<T>
}

export function createRankedReadsServe(deps: {
	config: RankedReadsServeConfig
	freshness?: GeoDbFreshness
	log?: {warn: (message: string, data?: Record<string, unknown>) => void}
}): RankedReadsServe {
	const {config} = deps
	if (config.reads.size > 0) active = true
	let warned = 0

	const fallBack = <T>(read: ServeRead, reason: FallbackReason, postgres: () => Promise<T>, error?: unknown) => {
		bump(fallbacks, `${read}|${reason}`)
		bump(served, `${read}|postgres`)
		if (reason === "error" && warned < 20 && deps.log) {
			warned++
			deps.log.warn("ranked read serve failed", {read, error: String(error)})
		}
		return postgres()
	}

	return {
		listed: (read) => config.reads.has(read),
		async run(read, geoDb, postgres) {
			if (!config.reads.has(read)) return postgres()
			let lag: number | null = null
			try {
				lag = (await deps.freshness?.lagBlocks()) ?? null
			} catch {
				lag = null
			}
			if (lag === null) return fallBack(read, "lag_unknown", postgres)
			if (lag > config.maxLagBlocks) return fallBack(read, "lag", postgres)
			const controller = new AbortController()
			let timer: ReturnType<typeof setTimeout> | undefined
			const timedOut = Symbol("timeout")
			const timeout = new Promise<typeof timedOut>((resolve) => {
				timer = setTimeout(() => resolve(timedOut), config.timeoutMs)
			})
			try {
				const outcome = await Promise.race([geoDb(controller.signal), timeout])
				if (outcome === timedOut) {
					controller.abort()
					return fallBack(read, "timeout", postgres)
				}
				bump(served, `${read}|geodb`)
				return outcome
			} catch (error) {
				return fallBack(read, reasonOf(error), postgres, error)
			} finally {
				clearTimeout(timer)
			}
		},
	}
}

// ---- the per-entry geo-db answers, in the shape of the Postgres read ----------------------------------------------------

const hex = (id: string) => id.replace(/-/g, "").toLowerCase()

export type TypeCountsPorts = {
	reader: DerivedCountsReader
	/** A gaia Space id (32 hex) as geo-db's id, null when it has no twin. Absent: a Space filter is declined. */
	geoSpaceOf?: (gaiaSpaceHex: string) => Promise<string | null>
	/** `entity_feed_blocklist` entity ids. */
	blocklist: () => Promise<string[]>
}

/** `public.topic_feed_type_counts`: type id (uuid text) to count, zero counts omitted. Declines what count mode cannot serve. */
export async function serveTypeCounts(ports: TypeCountsPorts, call: TypeCountsCall): Promise<Map<string, number>> {
	let geoSpaces: string[] | null = null
	if (call.spaces !== null) {
		if (!ports.geoSpaceOf) throw new ServeDecline("not_served")
		if (call.spaces.length > 64) throw new ServeDecline("too_many_spaces")
		geoSpaces = []
		for (const space of call.spaces) {
			const geo = await ports.geoSpaceOf(hex(space))
			if (geo === null) throw new ServeDecline("unmapped_space")
			geoSpaces.push(geo)
		}
	}
	const blocked = await ports.blocklist()
	const plan = planTypeCounts(call, blocked.length, geoSpaces)
	if (plan === null) throw new ServeDecline("not_served")
	if (isFallback(plan)) throw new ServeDecline(plan.fallback)
	try {
		const counts = await ports.reader.typeCounts(
			1,
			plan.group,
			plan.walked,
			blocked.map(hex),
			plan.form === 13 ? undefined : {others: plan.others, matchAll: plan.matchAll, graph: plan.graph},
		)
		return typeCountsFrom(plan, counts.counts)
	} catch (error) {
		if (error instanceof FormGatedError) throw new ServeDecline("serve_revision")
		const refusal = countsRefusal(error)
		if (refusal === "exhausted") throw new ServeDecline("resource_exhausted")
		if (refusal === "revision" && plan.form >= REVISION_TOPICS) throw new ServeDecline("serve_revision")
		throw error
	}
}

export type ScoresPorts = {
	reader: DerivedScoresReader
	/** The ranking cards of the entities that exist (id as 32 hex), as the shadows read them. */
	cards: (ids: string[]) => Promise<Map<string, ScoreCard>>
	config: () => Promise<ScoreConfig | null>
	/**
	 * The ids (32 hex) that have an `entity_ranking_scores` row. `entities_in_best_order` puts an entity without one after every
	 * scored entity, in the caller's order, which geo-db cannot tell (it serves a default row), so `serveBestOrder` ranks only these.
	 * Absent: every entity with a card is ranked (the shadow's reading, not exact for an unscored entity).
	 */
	scored?: (ids: string[]) => Promise<Set<string>>
}

async function scoreRows(ports: ScoresPorts, ids: string[], only: ReadonlySet<string> | null = null) {
	const [cards, config] = await Promise.all([ports.cards(ids), ports.config()])
	if (config === null) throw new Error("entity_ranking_config has no usable row")
	const withCard = ids.filter((id) => cards.has(id))
	const asked = only === null ? withCard : withCard.filter((id) => only.has(id))
	const rows = new Map<string, Awaited<ReturnType<DerivedScoresReader["scores"]>>[number]>()
	for (let i = 0; i < asked.length; i += MAX_SCORE_CARDS) {
		const chunk = asked.slice(i, i + MAX_SCORE_CARDS)
		const served = await ports.reader.scores(
			config,
			chunk.map((id) => cards.get(id) as ScoreCard),
		)
		chunk.forEach((id, j) => {
			const row = served[j]
			if (row === undefined) throw Object.assign(new Error("short scores reply"), {reason: "client_failed"})
			rows.set(id, row)
		})
	}
	return {withCard, asked, rows}
}

/** `public.entities_in_best_order(ids)`: the ids in page order (dashless lowercase), from the scores geo-db serves. */
export async function serveBestOrder(ports: ScoresPorts, requested: string[]): Promise<string[]> {
	const ids = [...new Set(requested.map(hex))]
	const {withCard, asked, rows} = await scoreRows(ports, ids, ports.scored ? await ports.scored(ids) : null)
	const ranking = new Map(asked.map((id) => [id, (rows.get(id) as {ranking: number}).ranking] as const))
	return entitiesInBestOrder(requested, new Set(withCard), ranking)
}

/**
 * The Postgres side of `ScoresPorts`: cards and config as the shadows read them, and the ids that have an `entity_ranking_scores`
 * row. These are the small point reads geo-db's scores are held to; the scores themselves come from geo-db.
 */
export function createPostgresScoresPorts(db: Database, reader: DerivedScoresReader): ScoresPorts {
	return {
		reader,
		cards: (ids) => readCards(db, ids),
		config: () => readScoreConfig(db),
		async scored(ids) {
			if (ids.length === 0) return new Set()
			const list = sql.join(
				ids.map((id) => sql`${id}::uuid`),
				sql`, `,
			)
			const result = (await db.execute(
				sql`SELECT entity_id::text AS id FROM public.entity_ranking_scores WHERE entity_id IN (${list})`,
			)) as {rows?: {id: string}[]}
			return new Set((result.rows ?? []).map((row) => hex(String(row.id))))
		},
	}
}

/** The fourteen `entities_*` score columns of each id; an entity with no entities row has every column NULL. */
export async function serveEntityColumns(ports: ScoresPorts, requested: string[]): Promise<Map<string, ScoreColumns>> {
	const ids = [...new Set(requested.map(hex))]
	const {rows} = await scoreRows(ports, ids)
	return new Map(ids.map((id) => [id, scoreColumns(rows.get(id) ?? null)] as const))
}

/** The columns a served `ScoreColumns` carries, in the order the shadow compares them. */
export const SERVED_COLUMNS = SCORE_COLUMNS

/** Where the trending twin's rows come from. The only implementation today is Postgres (the shadow's); a geo-db graph reader is not built. */
export type TrendingSource = {today(): Promise<string>; graph(today: string, spaces: string[]): Promise<Graph>}

/** Runs `ask` on the trending twin over the source's rows. Declines `no_source` without one. */
export async function serveTrending<T>(
	source: TrendingSource | undefined,
	spaces: string[],
	ask: (twin: Trending) => T,
): Promise<T> {
	if (source === undefined) throw new ServeDecline("no_source")
	const today = await source.today()
	return ask(new Trending(await source.graph(today, spaces), today))
}

/** Where the feed twin's rows come from; see `TrendingSource`. */
export type FeedsSource = {feedData(): Promise<FeedData>}

/** Runs `ask` on the feed twin over the source's rows. Declines `no_source` without one. */
export async function serveFeeds<T>(source: FeedsSource | undefined, ask: (twin: Feeds) => T): Promise<T> {
	if (source === undefined) throw new ServeDecline("no_source")
	return ask(new Feeds(await source.feedData()))
}
