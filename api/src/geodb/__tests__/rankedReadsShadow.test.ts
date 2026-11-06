import {readFileSync} from "node:fs"
import {join} from "node:path"
import {beforeEach, describe, expect, it} from "vitest"
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
})
