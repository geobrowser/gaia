import {beforeEach, describe, expect, it, vi} from "vitest"
import type {Database} from "../../forYou/queries"
import {GeoDbError} from "../client"
import type {DerivedReader, DerivedRow} from "../derived"
import {
	__resetScoresShadowMetricsForTests,
	compareScore,
	createScoresShadow,
	readScoresShadowConfig,
	renderScoresShadowMetrics,
	toSampledRow,
} from "../scoresShadow"

const E = "11111111-1111-4111-8111-111111111111"
const S = "22222222-2222-4222-8222-222222222222"
const EH = E.replace(/-/g, "")
const SH = S.replace(/-/g, "")
const Z = "0".repeat(32)
const held = (group: string, score: number, value = Buffer.alloc(0)): DerivedRow => ({group, score, value})
const log = {warn: vi.fn()}
const count = (table: string, result: string) =>
	Number(new RegExp(`table="${table}",result="${result}"\\} (\\d+)`).exec(renderScoresShadowMetrics())?.[1])

beforeEach(() => {
	__resetScoresShadowMetricsForTests()
	log.warn.mockClear()
})

describe("config", () => {
	it("is off unless the flag says on, and clamps tunables", () => {
		expect(readScoresShadowConfig({}).enabled).toBe(false)
		expect(readScoresShadowConfig({GEODB_SCORES_SHADOW: "0"}).enabled).toBe(false)
		expect(readScoresShadowConfig({GEODB_SCORES_SHADOW: "on"}).enabled).toBe(true)
		const c = readScoresShadowConfig({GEODB_SCORES_SHADOW_SAMPLE: "9999", GEODB_SCORES_SHADOW_INTERVAL_MS: "x"})
		expect(c.sample).toBe(200)
		expect(c.intervalMs).toBe(60_000)
	})
})

describe("layouts and comparison", () => {
	it("maps each table to its geo-db key and group", () => {
		expect(toSampledRow("global_scores", {entity_id: E, score: "0.5"})).toEqual({
			keyHex: EH,
			groupHex: Z,
			score: 0.5,
		})
		expect(toSampledRow("space_scores", {space_id: S, score: "2"})).toEqual({keyHex: SH, groupHex: Z, score: 2})
		expect(toSampledRow("local_scores", {entity_id: E, space_id: S, score: "1.25"})).toEqual({
			keyHex: EH + SH,
			groupHex: SH,
			score: 1.25,
		})
	})
	it("skips non-UUID ids and non-finite scores", () => {
		expect(toSampledRow("global_scores", {entity_id: "nope", score: "1"})).toBeNull()
		expect(toSampledRow("global_scores", {entity_id: E, score: "NaN"})).toBeNull()
		expect(toSampledRow("local_scores", {entity_id: E, space_id: "x", score: "1"})).toBeNull()
	})
	it("compares scores with a tolerance, group and empty value", () => {
		const s = {keyHex: EH, groupHex: Z, score: 0.3}
		expect(compareScore(s, held(Z, 0.3 + 1e-12))).toBe("match")
		expect(compareScore(s, held(Z, 0.31))).toBe("mismatch")
		expect(compareScore(s, null)).toBe("unsupported")
		expect(compareScore(s, held(SH, 0.3))).toBe("unsupported")
		expect(compareScore(s, held(Z, 0.3, Buffer.from([1])))).toBe("unsupported")
	})
})

function fakeDb(rows: Record<string, Record<string, unknown>[]>): Database {
	return {
		execute: vi.fn(async (q: {queryChunks?: unknown[]}) => {
			const text = JSON.stringify(q.queryChunks ?? [])
			const table = text.includes("local_scores") ? "local" : text.includes("space_scores") ? "space" : "global"
			return {rows: rows[table] ?? []}
		}),
	} as unknown as Database
}

describe("the shadow", () => {
	const config = {enabled: true, intervalMs: 1000, sample: 5}
	it("exports nothing until mounted, then every series at zero", () => {
		expect(renderScoresShadowMetrics()).toBe("")
		const reader = {get: vi.fn(), close: vi.fn()} as unknown as DerivedReader
		createScoresShadow({config, db: fakeDb({}), reader, log})
		expect(count("global_scores", "match")).toBe(0)
		expect(count("space_scores", "failed")).toBe(0)
	})
	it("counts match, mismatch, unsupported and failed per table", async () => {
		const get = vi.fn(async (collection: number, key: string) => {
			if (collection === 2) return held(Z, 1)
			if (collection === 3) return held(SH, 9)
			if (key === SH) return null
			throw new GeoDbError("client_failed", "boom")
		})
		const reader = {get, close: vi.fn()} as unknown as DerivedReader
		const db = fakeDb({
			global: [{entity_id: E, score: "1"}],
			local: [{entity_id: E, space_id: S, score: "1"}],
			space: [{space_id: S, score: "1"}],
		})
		const shadow = createScoresShadow({config, db, reader, log})
		await shadow.runOnce()
		expect(count("global_scores", "match")).toBe(1)
		expect(count("local_scores", "mismatch")).toBe(1)
		expect(count("space_scores", "unsupported")).toBe(1)
		expect(get).toHaveBeenCalledWith(2, EH)
		expect(get).toHaveBeenCalledWith(3, EH + SH)
		expect(log.warn).toHaveBeenCalledWith("scores shadow mismatch", {table: "local_scores"})
		get.mockRejectedValue(new GeoDbError("client_failed", "boom"))
		await shadow.runOnce()
		expect(count("global_scores", "failed")).toBe(1)
	})
	it("counts a failed Postgres sample", async () => {
		const reader = {get: vi.fn(), close: vi.fn()} as unknown as DerivedReader
		const db = {execute: vi.fn().mockRejectedValue(new Error("pg down"))} as unknown as Database
		await createScoresShadow({config, db, reader, log}).runOnce()
		expect(count("global_scores", "failed")).toBe(1)
		expect(reader.get).not.toHaveBeenCalled()
	})
	it("does not start a timer when disabled", () => {
		const reader = {get: vi.fn(), close: vi.fn()} as unknown as DerivedReader
		const setI = vi.spyOn(globalThis, "setInterval")
		createScoresShadow({config: {...config, enabled: false}, db: fakeDb({}), reader, log}).start()
		expect(setI).not.toHaveBeenCalled()
		setI.mockRestore()
	})
})
