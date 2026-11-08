import {beforeEach, describe, expect, it, vi} from "vitest"
import type {Database} from "../../forYou/queries"
import {GeoDbError} from "../client"
import type {DerivedScanReader, DerivedScoresReader, ScoreRow} from "../derived"
import {
	__resetBestOrderShadowMetricsForTests,
	createBestOrderShadow,
	readBestOrderShadowConfig,
	renderBestOrderShadowMetrics,
	samePage,
} from "../entitiesBestOrderShadow"

const A = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
const B = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
const S = "55555555-5555-4555-8555-555555555555"
const hex = (u: string) => u.replace(/-/g, "")
const log = {warn: vi.fn()}
const count = (read: string, result: string) =>
	Number(new RegExp(`read="${read}",result="${result}"\\} (\\d+)`).exec(renderBestOrderShadowMetrics())?.[1])

const row = (ranking: number): ScoreRow => ({
	engaged: true,
	trendingSource: 0,
	counts: [0, 0, 0, 0, 0],
	quality: 0,
	intrinsic: 0,
	participation: 0,
	comment: 0,
	typeWeight: 1,
	watch: 0,
	ageCredit: 0,
	trending: 0,
	ranking,
	topicHex: null,
})

type Pages = {best: string[]; global: string[]; local: string[]}
/** A db answering by what the query touches. `sampleIds` and the local-score space sample read plain id/space rows. */
function fakeDb(pages: Pages): Database {
	const card = (id: string) => ({
		id,
		created_epoch: "1700000000",
		property_count: "1",
		relation_count: "0",
		type_weight_max: null,
	})
	const config = {
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
		execute: vi.fn(async (q: {queryChunks?: unknown[]}) => {
			const text = JSON.stringify(q.queryChunks ?? [])
			if (text.includes("entities_in_best_order")) return {rows: pages.best.map((id) => ({id}))}
			if (text.includes("INNER JOIN global_scores")) return {rows: pages.global.map((id) => ({id}))}
			if (text.includes("INNER JOIN local_scores")) return {rows: pages.local.map((id) => ({id}))}
			if (text.includes("FROM local_scores")) return {rows: [{entity_id: A, space_id: S, score: "1"}]}
			if (text.includes("entity_ranking_config")) return {rows: [config]}
			if (text.includes("property_count")) return {rows: [card(A), card(B)]}
			return {rows: [{entity_id: A}, {entity_id: B}]}
		}),
	} as unknown as Database
}
const reader = (over: Partial<DerivedScanReader & DerivedScoresReader> = {}) =>
	({
		get: vi.fn(),
		close: vi.fn(),
		scores: vi.fn(async () => [row(1), row(2)]),
		scan: vi.fn(async (collection: number) =>
			collection === 2
				? {
						rows: [
							{keyHex: hex(B), score: 2, valueHex: ""},
							{keyHex: hex(A), score: 1, valueHex: ""},
						],
						more: false,
					}
				: {
						rows: [
							{keyHex: hex(A) + hex(S), score: 3, valueHex: ""},
							{keyHex: hex(B) + hex(S), score: 3, valueHex: ""},
						],
						more: false,
					},
		),
		...over,
	}) as unknown as DerivedScanReader & DerivedScoresReader
const config = {enabled: true, intervalMs: 1000, sample: 5}
const agreeing: Pages = {best: [hex(B), hex(A)], global: [hex(B), hex(A)], local: [hex(A), hex(B)]}

beforeEach(() => {
	__resetBestOrderShadowMetricsForTests()
	log.warn.mockClear()
})

describe("config, samePage and metrics", () => {
	it("is off unless the flag says on, and clamps the sample to 64", () => {
		expect(readBestOrderShadowConfig({}).enabled).toBe(false)
		expect(readBestOrderShadowConfig({GEODB_ENTITIES_BEST_ORDER_SHADOW: "0"}).enabled).toBe(false)
		expect(readBestOrderShadowConfig({GEODB_ENTITIES_BEST_ORDER_SHADOW: "true"}).enabled).toBe(true)
		expect(readBestOrderShadowConfig({GEODB_ENTITIES_BEST_ORDER_SHADOW_SAMPLE: "900"}).sample).toBe(64)
	})
	it("compares position by position", () => {
		expect(samePage(["a", "b"], ["a", "b"])).toBe(true)
		expect(samePage(["a", "b"], ["b", "a"])).toBe(false)
		expect(samePage(["a"], ["a", "b"])).toBe(false)
	})
	it("exports nothing until mounted, then every series at zero", () => {
		expect(renderBestOrderShadowMetrics()).toBe("")
		createBestOrderShadow({config, db: fakeDb(agreeing), reader: reader(), log})
		expect(count("best_order", "match")).toBe(0)
		expect(count("local_score", "failed")).toBe(0)
	})
})

describe("runOnce", () => {
	it("counts a match for each read when the orders agree", async () => {
		await createBestOrderShadow({config, db: fakeDb(agreeing), reader: reader(), log}).runOnce()
		expect(count("best_order", "match")).toBe(1)
		expect(count("global_score", "match")).toBe(1)
		expect(count("local_score", "match")).toBe(1)
	})
	it("counts a mismatch when only the order differs, and logs counts only", async () => {
		const swapped = {...agreeing, best: [hex(A), hex(B)], local: [hex(B), hex(A)]}
		await createBestOrderShadow({config, db: fakeDb(swapped), reader: reader(), log}).runOnce()
		expect(count("best_order", "mismatch")).toBe(1)
		expect(count("local_score", "mismatch")).toBe(1)
		expect(count("global_score", "match")).toBe(1)
		expect(JSON.stringify(log.warn.mock.calls)).not.toContain(hex(A))
	})
	it("counts failed per read when geo-db fails", async () => {
		const failing = reader({
			scores: vi.fn(async () => {
				throw new GeoDbError("client_failed", "boom")
			}),
			scan: vi.fn(async () => {
				throw new GeoDbError("timeout", "slow")
			}),
		})
		await createBestOrderShadow({config, db: fakeDb(agreeing), reader: failing, log}).runOnce()
		expect(count("best_order", "failed")).toBe(1)
		expect(count("global_score", "failed")).toBe(1)
		expect(count("local_score", "failed")).toBe(1)
	})
	it("skips best_order on a node that cannot serve scores", async () => {
		await createBestOrderShadow({config, db: fakeDb(agreeing), reader: reader({scores: undefined}), log}).runOnce()
		expect(count("best_order", "match") + count("best_order", "failed") + count("best_order", "mismatch")).toBe(0)
		expect(count("global_score", "match")).toBe(1)
	})
	it("scans collection 3 under the sampled space and collection 2 under group 0", async () => {
		const r = reader()
		await createBestOrderShadow({config, db: fakeDb(agreeing), reader: r, log}).runOnce()
		expect(r.scan).toHaveBeenCalledWith(2, "0", 5)
		expect(r.scan).toHaveBeenCalledWith(3, hex(S), 5)
	})
})
