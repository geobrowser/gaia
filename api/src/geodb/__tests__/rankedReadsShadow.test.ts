import {readFileSync} from "node:fs"
import {join} from "node:path"
import {beforeEach, describe, expect, it} from "vitest"
import type {FeedsSample, FeedsShadowSource} from "../rankedFeedsShadow"
import {type FeedData, Feeds} from "../rankedReads/feeds"
import {nextPrimerClaim, type PrimerData, primerCandidates} from "../rankedReads/primer"
import {type Graph, Trending} from "../rankedReads/trending"
import {
	__resetRankedReadsShadowMetricsForTests,
	createRankedReadsShadow,
	type PgDebateRow,
	type RankedReadsSource,
	readRankedReadsShadowConfig,
	renderRankedReadsShadowMetrics,
	sameRows,
} from "../rankedReadsShadow"

const golden = JSON.parse(readFileSync(join(__dirname, "../rankedReads/__tests__/fixtures/golden.json"), "utf8")) as {
	today: string
	seeds: {tables: Record<string, unknown[]>}[]
}
const graph = golden.seeds[0]!.tables as unknown as Graph
const config = {enabled: true, intervalMs: 1000, spaces: 3}
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

		it("exports zero series for the feed reads when no feeds source is configured", async () => {
			await createRankedReadsShadow({config, source: source(), log}).runOnce()
			for (const read of FEED_READS) expect(series(read, "match"), read).toBe(0)
		})
	})
})
