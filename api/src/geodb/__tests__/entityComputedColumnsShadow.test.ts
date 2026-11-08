import {beforeEach, describe, expect, it, vi} from "vitest"
import type {Database} from "../../forYou/queries"
import type {ScoreRow} from "../derived"
import {
	__resetEntityColumnsShadowMetricsForTests,
	compareColumns,
	createEntityColumnsShadow,
	readEntityColumnsShadowConfig,
	renderEntityColumnsShadowMetrics,
} from "../entityComputedColumnsShadow"
import {type SCORE_COLUMNS, scoreColumns} from "../rankedReads/entityColumns"

const A = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
const hexA = A.replace(/-/g, "")
const log = {warn: vi.fn()}
const total = (result: string, column = "ranking_score") =>
	Number(new RegExp(`column="${column}",result="${result}"\\} (\\d+)`).exec(renderEntityColumnsShadowMetrics())?.[1])

const served = (over: Partial<ScoreRow> = {}): ScoreRow => ({
	engaged: true,
	trendingSource: 2,
	counts: [3, 1, 4, 2, 5],
	quality: 0.5,
	intrinsic: 0.25,
	participation: 1,
	comment: 0.5,
	typeWeight: 1,
	watch: 0,
	ageCredit: 0.1,
	trending: 0.2,
	ranking: 42.5,
	topicHex: "0123456789abcdef0123456789abcdef",
	...over,
})
/** The Postgres text the SQL functions would print for `row`. */
const pgText = (row: ScoreRow) =>
	Object.fromEntries(Object.entries(scoreColumns(row)).map(([k, v]) => [k, v === null ? null : String(v)])) as Record<
		(typeof SCORE_COLUMNS)[number],
		string | null
	>

/** A db whose execute() answers by the order the shadow issues reads: sample, columns, cards, config (the last three in parallel). */
function fakeDb(pgRow: Record<string, unknown>): Database {
	const cardRow = {
		id: A,
		created_epoch: "1700000000",
		property_count: "3",
		relation_count: "1",
		type_weight_max: null,
	}
	const configRow = {
		tau: "1",
		z: "1",
		pp: "1",
		pn: "1",
		floor: "0",
		cap: "1",
		pt: "1",
		rt: "1",
		pw: "1",
		pc: "1",
		cw: "1",
		cc: "1",
	}
	return {
		execute: vi.fn(async (query: unknown) => {
			const text = JSON.stringify(query)
			if (text.includes("entities_")) return {rows: [{id: A, ...pgRow}]}
			if (text.includes("entity_ranking_config")) return {rows: [configRow]}
			if (text.includes("property_count")) return {rows: [cardRow]}
			return {rows: [{entity_id: A}]}
		}),
	} as unknown as Database
}

beforeEach(() => {
	__resetEntityColumnsShadowMetricsForTests()
	log.warn.mockClear()
})

describe("config and metrics", () => {
	it("is off unless the flag says on, and clamps the sample to geo-db's 64", () => {
		expect(readEntityColumnsShadowConfig({}).enabled).toBe(false)
		expect(readEntityColumnsShadowConfig({GEODB_ENTITY_COLUMNS_SHADOW: "0"}).enabled).toBe(false)
		expect(readEntityColumnsShadowConfig({GEODB_ENTITY_COLUMNS_SHADOW: "on"}).enabled).toBe(true)
		expect(readEntityColumnsShadowConfig({GEODB_ENTITY_COLUMNS_SHADOW_SAMPLE: "500"}).sample).toBe(64)
	})
	it("exports nothing until mounted, then every series at zero", () => {
		expect(renderEntityColumnsShadowMetrics()).toBe("")
		createEntityColumnsShadow({
			config: {enabled: true, intervalMs: 1000, sample: 5},
			db: fakeDb({}),
			reader: {},
			log,
		})
		expect(total("match")).toBe(0)
		expect(total("failed", "upvotes")).toBe(0)
	})
})

describe("compareColumns", () => {
	it("matches the columns the SQL would print", () => {
		const verdicts = compareColumns(pgText(served()), served())
		expect(Object.values(verdicts).every((v) => v === "match")).toBe(true)
	})
	it("names the column that differs", () => {
		const verdicts = compareColumns({...pgText(served()), upvotes: "9"}, served())
		expect(verdicts.upvotes).toBe("mismatch")
		expect(verdicts.downvotes).toBe("match")
	})
	it("holds a topic id to the dashed uuid and NULL to NULL", () => {
		const row = served({trendingSource: 0})
		expect(compareColumns(pgText(row), row).trending_topic_id).toBe("match")
		expect(compareColumns({...pgText(row), trending_topic_id: "x"}, row).trending_topic_id).toBe("mismatch")
	})
})

describe("runOnce", () => {
	const config = {enabled: true, intervalMs: 1000, sample: 5}
	it("counts a match per column", async () => {
		const row = served()
		const shadow = createEntityColumnsShadow({
			config,
			db: fakeDb(pgText(row)),
			reader: {scores: vi.fn(async () => [row])},
			log,
			random: () => A,
		})
		await shadow.runOnce()
		expect(total("match")).toBe(1)
		expect(total("match", "trending_topic_id")).toBe(1)
		expect(total("mismatch")).toBe(0)
	})
	it("counts a mismatch and logs counts only", async () => {
		const row = served()
		const shadow = createEntityColumnsShadow({
			config,
			db: fakeDb({...pgText(row), ranking_score: "1"}),
			reader: {scores: vi.fn(async () => [row])},
			log,
			random: () => A,
		})
		await shadow.runOnce()
		expect(total("mismatch")).toBe(1)
		expect(total("match", "upvotes")).toBe(1)
		expect(log.warn).toHaveBeenCalledWith("entity columns shadow disagreement", {})
		expect(JSON.stringify(log.warn.mock.calls)).not.toContain(hexA)
	})
	it("counts failed on every column when the reader throws", async () => {
		const shadow = createEntityColumnsShadow({
			config,
			db: fakeDb(pgText(served())),
			reader: {
				scores: vi.fn(async () => {
					throw new Error("boom")
				}),
			},
			log,
			random: () => A,
		})
		await shadow.runOnce()
		expect(total("failed")).toBe(1)
		expect(total("failed", "upvotes")).toBe(1)
	})
	it("does nothing when the reader cannot serve scores", async () => {
		const shadow = createEntityColumnsShadow({config, db: fakeDb({}), reader: {}, log, random: () => A})
		await shadow.runOnce()
		expect(total("match")).toBe(0)
	})
})
