import {readFileSync} from "node:fs"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {checkOrderedByScore, entitiesInBestOrder, entitiesOrderedByScore, InvalidParameter, pageOf} from "../bestOrder"

interface Seed {
	seed: number
	tables: {
		entities: {id: string}[]
		ranking: {entity_id: string; ranking_score: string | number}[]
		global: {entity_id: string; score: string | number}[]
		local: {entity_id: string; space_id: string; score: string | number}[]
	}
	best: {ids: string[]; order?: string[]; error?: string}[]
	ordered: {type: "local" | "global"; space: string | null; direction: "ASC" | "DESC"; order: string[]}[]
	errors: {type: string | null; message: string}[]
}
const golden = JSON.parse(readFileSync(join(__dirname, "fixtures", "golden-best-order.json"), "utf8")) as {
	seeds: Seed[]
}
const hex = (ids: string[]) => ids.map((i) => i.replace(/-/g, ""))

describe("ordered entity reads against Postgres golden output", () => {
	it("exercises ties, unscored entities, missing ids, duplicates and the cap", () => {
		let scoredTie = false
		let unscored = 0
		let missing = 0
		let dup = 0
		let capError = 0
		for (const s of golden.seeds) {
			const ranking = s.tables.ranking.map((r) => Number(r.ranking_score))
			scoredTie ||= new Set(ranking).size < ranking.length
			const have = new Set(s.tables.entities.map((e) => e.id))
			for (const c of s.best) {
				if (c.error) capError++
				missing += c.ids.some((i) => !have.has(i)) ? 1 : 0
				dup += new Set(c.ids).size < c.ids.length ? 1 : 0
				unscored += c.order && c.order.length > 0 && c.order.length < s.tables.ranking.length ? 1 : 0
			}
		}
		expect(scoredTie).toBe(true)
		expect(unscored).toBeGreaterThan(0)
		expect(missing).toBeGreaterThan(0)
		expect(dup).toBeGreaterThan(0)
		expect(capError).toBe(golden.seeds.length)
	})

	for (const s of golden.seeds) {
		const existing = new Set(s.tables.entities.map((e) => e.id))
		const ranking = new Map(s.tables.ranking.map((r) => [r.entity_id, Number(r.ranking_score)]))

		it(`seed ${s.seed}: entities_in_best_order keeps the order of every call`, () => {
			for (const c of s.best) {
				if (c.error) {
					expect(() => entitiesInBestOrder(c.ids, existing, ranking)).toThrow(InvalidParameter)
					continue
				}
				expect(entitiesInBestOrder(c.ids, existing, ranking), JSON.stringify(c.ids)).toEqual(
					hex(c.order as string[]),
				)
			}
		})

		it(`seed ${s.seed}: entities_ordered_by_score keeps the order of every call`, () => {
			for (const c of s.ordered) {
				const rows =
					c.type === "global"
						? s.tables.global.map((r) => ({id: r.entity_id, score: Number(r.score)}))
						: s.tables.local
								.filter((r) => r.space_id === c.space)
								.map((r) => ({id: r.entity_id, score: Number(r.score)}))
				expect(entitiesOrderedByScore(rows, existing, c.direction), `${c.type} ${c.direction}`).toEqual(
					hex(c.order),
				)
				expect(c.order.length).toBe(rows.length)
			}
		})

		it(`seed ${s.seed}: the argument errors match`, () => {
			for (const e of s.errors) {
				expect(() => checkOrderedByScore(e.type, null)).toThrow(e.message)
			}
		})
	}
})

describe("entitiesInBestOrder", () => {
	const A = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
	const B = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
	const C = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
	const D = "dddddddd-dddd-4ddd-8ddd-dddddddddddd"
	const exists = new Set([A, B, C, D])
	it("breaks scored ties by id descending and unscored by the caller's order", () => {
		const ranking = new Map([
			[A, 1],
			[B, 1],
		])
		expect(entitiesInBestOrder([A, B, D, C], exists, ranking)).toEqual(hex([B, A, D, C]))
		expect(entitiesInBestOrder([A, C, D], exists, new Map())).toEqual(hex([A, C, D]))
	})
	it("returns a duplicate once at its first position and omits a missing id", () => {
		expect(entitiesInBestOrder([C, D, C, "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"], exists, new Map())).toEqual(
			hex([C, D]),
		)
	})
})

describe("entitiesOrderedByScore and pageOf", () => {
	it("breaks ties by id ascending in both directions and drops entities with no row", () => {
		const rows = [
			{id: "02", score: 1},
			{id: "01", score: 1},
			{id: "03", score: 2},
		]
		expect(entitiesOrderedByScore(rows, null, "DESC")).toEqual(["03", "01", "02"])
		expect(entitiesOrderedByScore(rows, null, "ASC")).toEqual(["01", "02", "03"])
		expect(entitiesOrderedByScore(rows, new Set(["01", "03"]), "DESC")).toEqual(["03", "01"])
	})
	it("pages", () => {
		expect(pageOf([1, 2, 3, 4], 1, 2)).toEqual([2, 3])
		expect(pageOf([1, 2], 5, 2)).toEqual([])
	})
})
