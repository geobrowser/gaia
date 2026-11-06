import {beforeEach, describe, expect, it, vi} from "vitest"
import type {Database} from "../../forYou/queries"
import {GeoDbError} from "../client"
import type {DerivedReader, DerivedRow, DerivedScanReader} from "../derived"
import {
	__resetRankingScoresShadowMetricsForTests,
	compareRow,
	createRankingScoresShadow,
	overlayCounts,
	readRankingScoresShadowConfig,
	renderRankingScoresShadowMetrics,
	toPgRow,
} from "../rankingScoresShadow"

const A = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
const B = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
const C = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
const hex = (u: string) => u.replace(/-/g, "")
const Z = "0".repeat(32)
const log = {warn: vi.fn()}
const count = (result: string) =>
	Number(new RegExp(`result="${result}"\\} (\\d+)`).exec(renderRankingScoresShadowMetrics())?.[1])

/** A 62-byte overlay value: version 1, source 0, the five counts, three f64 terms, an empty topic. */
function overlay(counts: number[], version = 1, length = 62): Buffer {
	const v = Buffer.alloc(length)
	v[0] = version
	counts.forEach((n, i) => v.writeUInt32LE(n, 2 + 4 * i))
	return v
}
const held = (score: number, value: Buffer): DerivedRow => ({group: Z, score, value})
const pgRow = (over: Record<string, unknown> = {}) => ({
	ranking_score: "50.5",
	positive: "3",
	negative: "1",
	stance_positive: "0",
	stance_negative: "0",
	commenter_count: "2",
	participation_score: "0",
	comment_score: "0",
	topic_score: "0",
	watch_score: "0",
	age_credit: "0",
	trending_score: "0",
	trending_source: null,
	...over,
})
const idleRow = pgRow({positive: "0", negative: "0", commenter_count: "0"})

beforeEach(() => {
	__resetRankingScoresShadowMetricsForTests()
	log.warn.mockClear()
})

describe("config", () => {
	it("is off unless the flag says on, and clamps tunables", () => {
		expect(readRankingScoresShadowConfig({}).enabled).toBe(false)
		expect(readRankingScoresShadowConfig({GEODB_RANKING_SCORES_SHADOW: "0"}).enabled).toBe(false)
		expect(readRankingScoresShadowConfig({GEODB_RANKING_SCORES_SHADOW: "on"}).enabled).toBe(true)
		const c = readRankingScoresShadowConfig({
			GEODB_RANKING_SCORES_SHADOW_SAMPLE: "9999",
			GEODB_RANKING_SCORES_SHADOW_INTERVAL_MS: "x",
		})
		expect(c.sample).toBe(200)
		expect(c.intervalMs).toBe(60_000)
	})
})

describe("the engaged predicate and the overlay layout", () => {
	it("is engaged when any engagement term is nonzero, un-engaged when all are zero", () => {
		expect(toPgRow(pgRow())?.engaged).toBe(true)
		expect(toPgRow(idleRow)?.engaged).toBe(false)
		for (const term of [
			"participation_score",
			"comment_score",
			"topic_score",
			"watch_score",
			"age_credit",
			"trending_score",
		])
			expect(toPgRow({...idleRow, [term]: "0.1"})?.engaged).toBe(true)
		expect(toPgRow({...idleRow, trending_source: "topic"})?.engaged).toBe(true)
		expect(toPgRow({...idleRow, ranking_score: "NaN"})).toBeNull()
	})
	it("reads the five counts of a 62-byte version-1 value only", () => {
		expect(overlayCounts(overlay([1, 2, 3, 4, 5]))).toEqual([1, 2, 3, 4, 5])
		expect(overlayCounts(overlay([1, 2, 3, 4, 5], 2))).toBeNull()
		expect(overlayCounts(overlay([1, 2, 3, 4, 5], 1, 48))).toBeNull()
		const bad = overlay([0, 0, 0, 0, 0])
		bad[1] = 3
		expect(overlayCounts(bad)).toBeNull()
	})
})

describe("compareRow", () => {
	const pg = toPgRow(pgRow())!
	const idle = toPgRow(idleRow)!
	it("matches an engaged row with the same score and counts, within tolerance", () => {
		expect(compareRow(pg, held(50.5 + 1e-12, overlay([3, 1, 0, 0, 2])))).toBe("match")
	})
	it("mismatches a different score or count", () => {
		expect(compareRow(pg, held(50.6, overlay([3, 1, 0, 0, 2])))).toBe("mismatch")
		expect(compareRow(pg, held(50.5, overlay([3, 1, 0, 0, 3])))).toBe("mismatch")
	})
	it("says absent_ok for an un-engaged row with no overlay and missing for an engaged one", () => {
		expect(compareRow(idle, null)).toBe("absent_ok")
		expect(compareRow(null, null)).toBe("absent_ok")
		expect(compareRow(pg, null)).toBe("missing")
	})
	it("says stale for an overlay row of an un-engaged or absent Postgres row", () => {
		expect(compareRow(idle, held(1, overlay([0, 0, 0, 0, 0])))).toBe("stale")
		expect(compareRow(null, held(1, overlay([1, 0, 0, 0, 0])))).toBe("stale")
	})
	it("says unsupported for an overlay row of another layout, before anything else", () => {
		expect(compareRow(pg, held(50.5, overlay([3, 1, 0, 0, 2], 1, 48)))).toBe("unsupported")
		expect(compareRow(null, held(50.5, Buffer.alloc(0)))).toBe("unsupported")
	})
})

function fakeDb(byId: Record<string, Record<string, unknown>>, sample: string[]): Database {
	return {
		execute: vi.fn(async (q: {queryChunks?: unknown[]}) => {
			const text = JSON.stringify(q.queryChunks ?? [])
			if (text.includes("WHERE entity_id =")) {
				const id = Object.keys(byId).find((k) => text.includes(k) || JSON.stringify(q).includes(k))
				return {rows: id ? [byId[id]] : []}
			}
			return {rows: sample.map((entity_id) => ({entity_id}))}
		}),
	} as unknown as Database
}

describe("the shadow", () => {
	const config = {enabled: true, intervalMs: 1000, sample: 5}
	it("exports nothing until mounted, then every series at zero", () => {
		expect(renderRankingScoresShadowMetrics()).toBe("")
		const reader = {get: vi.fn(), close: vi.fn()} as unknown as DerivedReader
		createRankingScoresShadow({config, db: fakeDb({}, []), reader, log})
		for (const r of ["match", "mismatch", "absent_ok", "missing", "stale", "unsupported", "failed"])
			expect(count(r)).toBe(0)
	})
	it("walks the overlay into Postgres and Postgres into the overlay, counting each result", async () => {
		const reader = {
			get: vi.fn(async (_c: number, key: string) => {
				if (key === hex(C)) return null
				throw new GeoDbError("client_failed", "boom")
			}),
			scan: vi.fn(async () => ({
				rows: [
					{score: 50.5, keyHex: hex(A), valueHex: overlay([3, 1, 0, 0, 2]).toString("hex")},
					{score: 9, keyHex: hex(B), valueHex: overlay([0, 0, 0, 0, 0]).toString("hex")},
				],
				more: false,
			})),
			close: vi.fn(),
		} as unknown as DerivedScanReader
		const db = fakeDb({[A]: pgRow(), [B]: idleRow, [C]: idleRow}, [C, hex(A)])
		await createRankingScoresShadow({config, db, reader, log}).runOnce()
		expect(reader.scan).toHaveBeenCalledWith(21, "0", 5)
		expect(count("match")).toBe(1)
		expect(count("stale")).toBe(1)
		expect(count("absent_ok")).toBe(1)
		expect(count("failed")).toBe(1)
		// Counts only: the warning names no entity.
		expect(JSON.stringify(log.warn.mock.calls)).not.toContain("aaaa")
		expect(JSON.stringify(log.warn.mock.calls)).not.toContain("bbbb")
	})
	it("counts a failed scan and a failed Postgres sample", async () => {
		const reader = {
			get: vi.fn(),
			scan: vi.fn().mockRejectedValue(new GeoDbError("client_failed", "boom")),
			close: vi.fn(),
		} as unknown as DerivedScanReader
		const db = {execute: vi.fn().mockRejectedValue(new Error("pg down"))} as unknown as Database
		await createRankingScoresShadow({config, db, reader, log}).runOnce()
		expect(count("failed")).toBe(2)
		expect(reader.get).not.toHaveBeenCalled()
	})
	it("does not start a timer when disabled", () => {
		const reader = {get: vi.fn(), close: vi.fn()} as unknown as DerivedReader
		const setI = vi.spyOn(globalThis, "setInterval")
		createRankingScoresShadow({config: {...config, enabled: false}, db: fakeDb({}, []), reader, log}).start()
		expect(setI).not.toHaveBeenCalled()
		setI.mockRestore()
	})
})
