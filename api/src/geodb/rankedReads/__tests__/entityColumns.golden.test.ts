import {readFileSync} from "node:fs"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import type {ScoreRow} from "../../derived"
import {
	ALL_COLUMNS,
	type ColumnValue,
	dashed,
	LISTING_COLUMNS,
	listingColumns,
	SCORE_COLUMNS,
	sameColumn,
	scoreColumns,
} from "../entityColumns"
import {type Graph, Trending} from "../trending"

interface PgScore {
	entity_id: string
	quality_score: number
	intrinsic_score: number
	ranking_score: number
	positive: number
	negative: number
	participation_score: number
	stance_positive: number
	stance_negative: number
	comment_score: number
	commenter_count: number
	age_credit: number
	trending_score: number
	trending_source: string | null
	trending_topic_id: string | null
}
interface Golden {
	today: string
	columns: string[]
	seeds: {
		seed: number
		tables: Record<string, unknown[]> & {scores: PgScore[]; results: Record<string, ColumnValue>[]}
	}[]
}

const golden = JSON.parse(readFileSync(join(__dirname, "fixtures", "golden-entity-columns.json"), "utf8")) as Golden

/** The score row geo-db serves for an entity whose Postgres row is `r`: the shape the twin reads. */
function served(r: PgScore): ScoreRow {
	return {
		engaged: true,
		trendingSource: r.trending_source === "listing" ? 1 : r.trending_source === "topic" ? 2 : 0,
		counts: [r.positive, r.negative, r.stance_positive, r.stance_negative, r.commenter_count],
		quality: r.quality_score,
		intrinsic: r.intrinsic_score,
		participation: r.participation_score,
		comment: r.comment_score,
		typeWeight: 1,
		watch: 0,
		ageCredit: r.age_credit,
		trending: r.trending_score,
		ranking: r.ranking_score,
		topicHex: r.trending_topic_id === null ? null : r.trending_topic_id.replace(/-/g, ""),
	}
}

describe("Entity computed-column twin against Postgres golden output", () => {
	it("covers all 17 functions, with null and non-null outputs for each", () => {
		expect(golden.columns.slice().sort()).toEqual([...ALL_COLUMNS])
		for (const c of ALL_COLUMNS) {
			const values = golden.seeds.flatMap((s) => s.tables.results.map((r) => r[c]))
			expect(
				values.some((v) => v === null),
				`${c} null`,
			).toBe(true)
			expect(
				values.some((v) => v !== null),
				`${c} non-null`,
			).toBe(true)
		}
	})

	for (const s of golden.seeds) {
		it(`seed ${s.seed}: every column of every entity matches`, () => {
			const trending = new Trending(s.tables as unknown as Graph, golden.today)
			const scores = new Map(s.tables.scores.map((r) => [r.entity_id, r]))
			let compared = 0
			for (const want of s.tables.results) {
				const id = want.id as string
				const row = scores.get(id)
				const got = {
					...scoreColumns(row === undefined ? null : served(row)),
					...listingColumns(trending, id),
				}
				for (const c of [...SCORE_COLUMNS, ...LISTING_COLUMNS]) {
					const w = want[c] ?? null
					expect(
						sameColumn(c, w === null ? null : String(w), got[c]),
						`${c} of ${id}: want ${w} got ${got[c]}`,
					).toBe(true)
					compared++
				}
			}
			expect(compared).toBe(s.tables.results.length * 17)
		})
	}
})

describe("scoreColumns", () => {
	const row = (over: Partial<ScoreRow> = {}): ScoreRow => ({
		engaged: true,
		trendingSource: 0,
		counts: [5, 2, 7, 3, 4],
		quality: 0.5,
		intrinsic: 0.25,
		participation: 1.5,
		comment: 0.75,
		typeWeight: 1,
		watch: 0,
		ageCredit: 0.1,
		trending: 0.2,
		ranking: -12.5,
		topicHex: "0123456789abcdef0123456789abcdef",
		...over,
	})
	it("returns NULL for every column when there is no row", () => {
		expect(Object.values(scoreColumns(null)).every((v) => v === null)).toBe(true)
	})
	it("names the stored columns", () => {
		const c = scoreColumns(row())
		expect([c.upvotes, c.downvotes, c.agrees, c.disagrees, c.commenters]).toEqual([5, 2, 7, 3, 4])
		expect(c.trending_age_credit).toBe(0.1)
		expect(c.ranking_score).toBe(-12.5)
	})
	it("keeps the topic id only for a topic boost", () => {
		expect(scoreColumns(row({trendingSource: 0})).trending_topic_id).toBeNull()
		expect(scoreColumns(row({trendingSource: 1})).trending_topic_id).toBeNull()
		expect(scoreColumns(row({trendingSource: 1})).trending_source).toBe("listing")
		expect(scoreColumns(row({trendingSource: 2})).trending_topic_id).toBe("01234567-89ab-cdef-0123-456789abcdef")
		expect(scoreColumns(row({trendingSource: 2})).trending_source).toBe("topic")
		expect(scoreColumns(row({trendingSource: 0})).trending_source).toBeNull()
	})
	it("dashes a uuid", () => {
		expect(dashed("0123456789ABCDEF0123456789abcdef")).toBe("01234567-89ab-cdef-0123-456789abcdef")
	})
})

describe("sameColumn", () => {
	it("holds numbers to a relative tolerance and text exactly", () => {
		expect(sameColumn("ranking_score", "12.3456789012", 12.3456789012)).toBe(true)
		expect(sameColumn("ranking_score", "12.3", 12.4)).toBe(false)
		expect(sameColumn("trending_source", "topic", "topic")).toBe(true)
		expect(sameColumn("trending_source", "topic", "listing")).toBe(false)
		expect(sameColumn("upvotes", null, null)).toBe(true)
		expect(sameColumn("upvotes", null, 0)).toBe(false)
		expect(sameColumn("upvotes", "0", null)).toBe(false)
	})
})
