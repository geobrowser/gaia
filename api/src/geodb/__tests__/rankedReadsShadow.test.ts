import {readFileSync} from "node:fs"
import {join} from "node:path"
import type {SQL} from "drizzle-orm"
import {PgDialect} from "drizzle-orm/pg-core"
import {beforeEach, describe, expect, it} from "vitest"
import type {FeedsSample, FeedsShadowSource} from "../rankedFeedsShadow"
import {type FeedData, Feeds} from "../rankedReads/feeds"
import {nextPrimerClaim, type PrimerData, primerCandidates} from "../rankedReads/primer"
import {type Graph, Trending} from "../rankedReads/trending"
import {
	__resetRankedReadsShadowMetricsForTests,
	createBudgetedPostgresRankedReadsSource,
	createRankedReadsShadow,
	type PgDebateRow,
	type RankedReadsSource,
	readRankedReadsShadowConfig,
	renderRankedReadsShadowMetrics,
	sameRows,
	type TransactionalDatabase,
	timedSource,
} from "../rankedReadsShadow"

const golden = JSON.parse(readFileSync(join(__dirname, "../rankedReads/__tests__/fixtures/golden.json"), "utf8")) as {
	today: string
	seeds: {tables: Record<string, unknown[]>}[]
}
const graph = golden.seeds[0]!.tables as unknown as Graph
const config = {enabled: true, intervalMs: 1000, spaces: 3, statementTimeoutMs: 1000}
const warnings: string[] = []
const log = {warn: (m: string) => void warnings.push(m)}

/** A source whose "Postgres" answers are the twin's own, optionally tampered with. */
function source(tamper?: (rows: PgDebateRow[]) => PgDebateRow[]): RankedReadsSource {
	const t = new Trending(graph, golden.today)
	const spaces = graph.spaces.map((s) => s.id)
	return {
		today: async () => golden.today,
		sampleSpaces: async () => spaces.slice(0, 3),
		graph: async () => graph,
		pgCurrentDebateAgendas: async (earlier) => {
			const rows = t.currentDebateAgendas(null, earlier).map((r) => ({...r, listed_at: Date.parse(r.listed_at)}))
			return tamper ? tamper(rows) : rows
		},
		pgTopicSuggestions: async (s) => t.topicSuggestionsForSpaces(s),
	}
}

const series = (read: string, result: string) =>
	Number(new RegExp(`read="${read}",result="${result}"\\} (\\d+)`).exec(renderRankedReadsShadowMetrics())?.[1])

describe("ranked reads shadow", () => {
	beforeEach(() => {
		__resetRankedReadsShadowMetricsForTests()
		warnings.length = 0
	})

	it("is flag-off by default and exports nothing until mounted", () => {
		expect(readRankedReadsShadowConfig({}).enabled).toBe(false)
		expect(readRankedReadsShadowConfig({GEODB_RANKED_READS_SHADOW: "on"}).enabled).toBe(true)
		expect(renderRankedReadsShadowMetrics()).toBe("")
	})

	it("reads the statement budget, defaulting to 5 minutes and capped at 10", () => {
		expect(readRankedReadsShadowConfig({}).statementTimeoutMs).toBe(300_000)
		expect(
			readRankedReadsShadowConfig({GEODB_RANKED_READS_SHADOW_STATEMENT_TIMEOUT_MS: "45000"}).statementTimeoutMs,
		).toBe(45_000)
		expect(
			readRankedReadsShadowConfig({GEODB_RANKED_READS_SHADOW_STATEMENT_TIMEOUT_MS: "9999999"}).statementTimeoutMs,
		).toBe(600_000)
		expect(
			readRankedReadsShadowConfig({GEODB_RANKED_READS_SHADOW_STATEMENT_TIMEOUT_MS: "0"}).statementTimeoutMs,
		).toBe(300_000)
	})

	it("times each source call into the stage metric", async () => {
		await createRankedReadsShadow({config, source: source(), log}).runOnce()
		const text = renderRankedReadsShadowMetrics()
		for (const stage of ["today", "sampleSpaces", "graph", "pgCurrentDebateAgendas", "pgTopicSuggestions"])
			expect(text, stage).toMatch(
				new RegExp(`gaia_api_ranked_read_shadow_stage_seconds_count\\{stage="${stage}"\\} \\d+`),
			)
		expect(text).toMatch(/gaia_api_ranked_read_shadow_stage_seconds_count\{stage="pgCurrentDebateAgendas"\} 2/)
	})

	it("timedSource records elapsed seconds and tags a thrown error with its stage, once", async () => {
		createRankedReadsShadow({config, source: source(), log}) // mounts the metrics
		let clock = 0
		const now = () => clock
		const inner = timedSource(
			{
				slow: async () => {
					clock += 2500
					return 1
				},
				broken: async () => Promise.reject(new Error("canceling statement due to statement timeout")),
			},
			"feeds.",
			now,
		)
		const outer = timedSource({run: () => inner.broken()}, "", now)
		expect(await inner.slow()).toBe(1)
		await expect(outer.run()).rejects.toThrow("statement timeout")
		const text = renderRankedReadsShadowMetrics()
		expect(text).toContain('gaia_api_ranked_read_shadow_stage_seconds_sum{stage="feeds.slow"} 2.5')
		expect(text).toContain('gaia_api_ranked_read_shadow_stage_seconds_count{stage="feeds.broken"} 1')
	})

	it("runs each Postgres call in its own read-only transaction that sets the budget first", async () => {
		const dialect = new PgDialect()
		const transactions: {config: unknown; statements: string[]}[] = []
		const db = {
			execute: async () => {
				throw new Error("statement outside a transaction")
			},
			transaction: async (fn: (tx: unknown) => Promise<unknown>, config?: unknown) => {
				const statements: string[] = []
				transactions.push({config, statements})
				return fn({
					execute: async (q: SQL) => {
						statements.push(dialect.sqlToQuery(q).sql)
						return {rows: []}
					},
				})
			},
		} as unknown as TransactionalDatabase
		const pg = createBudgetedPostgresRankedReadsSource(db, 45_000)
		await pg.today()
		await pg.feeds!.sample([])
		expect(transactions).toHaveLength(2)
		for (const t of transactions) {
			expect(t.config).toEqual({accessMode: "read only"})
			expect(t.statements[0]).toBe("SET LOCAL statement_timeout = 45000")
			expect(t.statements.length).toBeGreaterThan(1)
		}
		expect(() => createBudgetedPostgresRankedReadsSource(db, 0)).toThrow()
	})

	it("counts a match for each read when twin and Postgres agree", async () => {
		await createRankedReadsShadow({config, source: source(), log}).runOnce()
		expect(series("current_debate_agendas_0", "match")).toBe(1)
		expect(series("current_debate_agendas_7", "match")).toBe(1)
		expect(series("topic_suggestions_for_spaces", "match")).toBe(1)
		expect(series("current_debate_agendas_0", "mismatch")).toBe(0)
	})

	it("counts a mismatch, without logging an id, when one row differs", async () => {
		await createRankedReadsShadow({
			config,
			source: source((rows) => rows.map((r, i) => (i === 0 ? {...r, item_position: 99} : r))),
			log,
		}).runOnce()
		expect(series("current_debate_agendas_0", "mismatch")).toBe(1)
		expect(warnings).toEqual(["ranked reads shadow disagreement", "ranked reads shadow disagreement"])
	})

	it("counts every read failed when the rows cannot be loaded", async () => {
		await createRankedReadsShadow({
			config,
			source: {...source(), graph: async () => Promise.reject(new Error("down"))},
			log,
		}).runOnce()
		expect(series("current_debate_agendas_0", "failed")).toBe(1)
		expect(series("topic_suggestions_for_spaces", "failed")).toBe(1)
	})

	it("sameRows requires equal length and order", () => {
		expect(sameRows([{a: 1}], [])).toBe(false)
		expect(sameRows([{a: 1, b: 2}], [{b: 2, a: 1}])).toBe(true)
		expect(sameRows([{a: 1}, {a: 2}], [{a: 2}, {a: 1}])).toBe(false)
	})

	describe("feed and primer reads", () => {
		const fg = JSON.parse(
			readFileSync(join(__dirname, "../rankedReads/__tests__/fixtures/golden-feeds.json"), "utf8"),
		) as {now: string; seeds: {tables: Record<string, any[]>}[]}
		const t = fg.seeds[0]!.tables
		const feedData = {
			entities: t.entities,
			relations: t.relations,
			names: t.names,
			scores: t.scores,
			blocklist: t.blocklist,
			exclusions: t.exclusions,
			typeRanking: t.typeRanking,
			topicRanking: t.topicRanking,
		} as FeedData
		const primerData = {
			spaces: t.spaces,
			votes: t.votes,
			stats: t.stats,
			anchorSets: t.anchorSets,
			anchorMembers: t.anchorMembers,
			entities: t.entities,
		} as PrimerData
		const sample: FeedsSample = {
			threshold: "5",
			types: [...new Set(t.typeRanking!.map((r) => r.type_id as string))].slice(0, 3),
			topics: [...new Set(t.topicRanking!.map((r) => r.topic_id as string))].slice(0, 2),
			spaces: t.spaces!.map((s) => s.id as string),
			userId: t.votes![0]!.user_id as string,
			now: new Date(fg.now),
		}
		const feeds = new Feeds(feedData)
		const pargs = {userId: sample.userId!, spaceIds: sample.spaces.slice(0, 2), answered: [], skipped: []}
		/** A feeds source whose "Postgres" answers are the twins' own, optionally tampered with. */
		const feedsSource = (tamper?: {ids?: (ids: string[]) => string[]}): FeedsShadowSource => {
			const tweak = (ids: string[]) => (tamper?.ids ? tamper.ids(ids) : ids)
			return {
				sample: async () => sample,
				feedData: async () => feedData,
				primerData: async () => primerData,
				pgFeed: async () => tweak(feeds.entitiesRankedForFeed(5, null, null, null, null).map((e) => e.id)),
				pgFeedByType: async () =>
					feeds.entitiesRankedForFeedByType(sample.types, 5, null, null, null, 100).map((e) => e.id),
				pgTopics: async () =>
					feeds
						.entitiesRankedForTopics({
							topics: sample.topics,
							weights: null,
							types: null,
							min: 5,
							after: null,
							before: null,
							spaces: null,
							maxPerTopic: 100,
							debateTaggedClaims: false,
							sortBy: "best",
							matchAll: false,
						})
						.map((e) => e.id),
				pgPrimer: async () => primerCandidates(primerData, pargs, sample.now),
				pgNextPrimer: async () => nextPrimerClaim(primerData, pargs, sample.now)?.id ?? null,
			}
		}
		const FEED_READS = [
			"entities_ranked_for_feed",
			"entities_ranked_for_feed_by_type",
			"entities_ranked_for_topics",
			"primer_candidates",
			"next_primer_claim",
		]

		it("samples something non-trivial", () => {
			expect(feeds.entitiesRankedForFeed(5, null, null, null, null).length).toBeGreaterThan(3)
			expect(primerCandidates(primerData, pargs, sample.now).length).toBeGreaterThan(0)
		})

		it("counts a match for each feed and primer read when twin and Postgres agree", async () => {
			await createRankedReadsShadow({config, source: {...source(), feeds: feedsSource()}, log}).runOnce()
			for (const read of FEED_READS) expect(series(read, "match"), read).toBe(1)
		})

		it("counts a mismatch for the read that differs and no other", async () => {
			await createRankedReadsShadow({
				config,
				source: {...source(), feeds: feedsSource({ids: (ids) => ids.slice(1)})},
				log,
			}).runOnce()
			expect(series("entities_ranked_for_feed", "mismatch")).toBe(1)
			expect(series("entities_ranked_for_feed_by_type", "match")).toBe(1)
		})

		it("counts the feed reads failed when their rows cannot be loaded", async () => {
			await createRankedReadsShadow({
				config,
				source: {
					...source(),
					feeds: {...feedsSource(), feedData: async () => Promise.reject(new Error("down"))},
				},
				log,
			}).runOnce()
			for (const read of FEED_READS) expect(series(read, "failed"), read).toBe(1)
		})

		it("names the stage that failed in the failure log", async () => {
			const fields: Record<string, unknown>[] = []
			await createRankedReadsShadow({
				config,
				source: {
					...source(),
					feeds: {...feedsSource(), feedData: async () => Promise.reject(new Error("statement timeout"))},
				},
				log: {warn: (_m: string, f?: Record<string, unknown>) => void fields.push(f ?? {})},
			}).runOnce()
			expect(fields.filter((f) => f.read === "entities_ranked_for_feed")).toEqual([
				{read: "entities_ranked_for_feed", stage: "feeds.feedData", error: "Error: statement timeout"},
			])
		})

		it("draws the random voter point in the process, not per row in Postgres", async () => {
			const dialect = new PgDialect()
			const statements: {sql: string; params: unknown[]}[] = []
			const tx = {
				execute: async (q: SQL) => {
					const {sql, params} = dialect.sqlToQuery(q)
					statements.push({sql, params})
					return {rows: sql.includes("OFFSET 199") ? [{threshold: "5"}] : []}
				},
			}
			const db = {
				execute: tx.execute,
				transaction: async (fn: (t: unknown) => Promise<unknown>) => fn(tx),
			} as unknown as TransactionalDatabase
			await createBudgetedPostgresRankedReadsSource(db, 1000).feeds!.sample([])
			const pick = statements.find((s) => s.sql.includes("user_votes") && s.sql.includes(">="))
			expect(pick, "user pick statement").toBeDefined()
			expect(pick!.sql).not.toContain("gen_random_uuid")
			expect(pick!.params).toHaveLength(1)
			expect(String(pick!.params[0])).toMatch(
				/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
			)
		})

		it("exports zero series for the feed reads when no feeds source is configured", async () => {
			await createRankedReadsShadow({config, source: source(), log}).runOnce()
			for (const read of FEED_READS) expect(series(read, "match"), read).toBe(0)
		})
	})
})
