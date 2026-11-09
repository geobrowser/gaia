import {existsSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"
import {beforeEach, describe, expect, it, vi} from "vitest"
import type {Database} from "../../forYou/queries"
import {GeoDbError} from "../client"
import type {DerivedReader} from "../derived"
import {
	__resetFeedRunsShadowMetricsForTests,
	COMPLETED_STATUS,
	compareWatermarks,
	createFeedRunsShadow,
	feedRunKey,
	parseFeedRunValue,
	readFeedRunsShadowConfig,
	readFeedRunsWatermark,
	readPostgresWatermark,
	renderFeedRunsShadowMetrics,
} from "../feedRunsShadow"

const here = dirname(fileURLToPath(import.meta.url))
const metric = (result: string) => new RegExp(`result="${result}"\\} (\\d+)`).exec(renderFeedRunsShadowMetrics())?.[1]
const T = "2026-11-06T12:00:00.000000Z"
const value = (block: number | string, cursor: string) => Buffer.from(`${block}:${cursor}`)
const readerOf = (v: Buffer | null | Error): Pick<DerivedReader, "get"> => ({
	get: async () => {
		if (v instanceof Error) throw v
		return v ? {group: "0".repeat(32), score: 0, value: v} : null
	},
})

beforeEach(() => __resetFeedRunsShadowMetricsForTests())

describe("pins", () => {
	it("use the mirror's key and the status readSeen filters on", () => {
		const mirror = join(here, "../../../../ranking-indexer/src/feed_signal_runs_mirror.rs")
		if (existsSync(mirror)) expect(readFileSync(mirror, "utf8")).toContain(`KEY_PREFIX: &str = "feed_signal_run:"`)
		expect(feedRunKey("succeeded")).toBe("feed_signal_run:succeeded")
		const queries = join(here, "../../forYou/queries.ts")
		if (existsSync(queries)) expect(readFileSync(queries, "utf8")).toContain(`r.status = '${COMPLETED_STATUS}'`)
	})
})

describe("parseFeedRunValue", () => {
	it("reads the mirror's shape, microseconds included", () => {
		const p = parseFeedRunValue(`1793968496123456:${T}/g2`)
		expect(p?.dataThrough?.toISOString()).toBe("2026-11-06T12:00:00.000Z")
		expect(p?.finishedAt.getTime()).toBe(1793968496123)
	})
	it("reads a dash as no cutoff and a slash inside the generation", () => {
		expect(parseFeedRunValue("5:-/")?.dataThrough).toBeNull()
		expect(parseFeedRunValue(`5:${T}/a/b:c`)?.dataThrough).not.toBeNull()
	})
	it("refuses other shapes", () => {
		for (const bad of ["", "x", "5:nope", "a:-/", `5:${T}`, "5:2026-11-06 12:00:00/g"])
			expect(parseFeedRunValue(bad)).toBeNull()
	})
})

describe("readFeedRunsWatermark", () => {
	it("asks collection 10 for the succeeded key, as hex", async () => {
		const get = vi.fn(async () => null)
		await readFeedRunsWatermark({get})
		expect(get).toHaveBeenCalledWith(10, Buffer.from("feed_signal_run:succeeded").toString("hex"))
	})
	it("answers absent, a watermark, or malformed", async () => {
		expect(await readFeedRunsWatermark(readerOf(null))).toEqual({present: false})
		const got = await readFeedRunsWatermark(readerOf(value(1, `${T}/g`)))
		expect(got.present && got.watermark?.toISOString()).toBe("2026-11-06T12:00:00.000Z")
		await expect(readFeedRunsWatermark(readerOf(Buffer.from("junk")))).rejects.toThrow()
	})
})

describe("compareWatermarks", () => {
	const d = new Date(T)
	it("matches the same instant to the millisecond, or both empty", () => {
		expect(compareWatermarks(d, {present: true, watermark: new Date(d)})).toBe("match")
		expect(compareWatermarks(null, {present: false})).toBe("match")
		expect(compareWatermarks(null, {present: true, watermark: null})).toBe("match")
	})
	it("calls a different instant a mismatch", () => {
		expect(compareWatermarks(d, {present: true, watermark: new Date(d.getTime() + 1)})).toBe("mismatch")
		expect(compareWatermarks(d, {present: true, watermark: null})).toBe("mismatch")
		expect(compareWatermarks(null, {present: true, watermark: d})).toBe("mismatch")
	})
	it("keeps a missing geo-db row apart from a mismatch", () => {
		expect(compareWatermarks(d, {present: false})).toBe("absent")
	})
})

describe("readPostgresWatermark", () => {
	it("filters on the completed status and reads a Date or null", async () => {
		const execute = vi.fn(async () => ({rows: [{watermark: new Date(T)}]}))
		expect((await readPostgresWatermark({execute} as unknown as Database))?.toISOString()).toBe(
			"2026-11-06T12:00:00.000Z",
		)
		const none = {execute: async () => ({rows: [{watermark: null}]})} as unknown as Database
		expect(await readPostgresWatermark(none)).toBeNull()
	})
})

describe("createFeedRunsShadow", () => {
	const make = (pg: (Date | null) | Error, geo: Buffer | null | Error) => {
		const warn = vi.fn()
		const shadow = createFeedRunsShadow({
			config: {enabled: true, intervalMs: 1000},
			db: {} as Database,
			reader: readerOf(geo),
			readWatermark: async () => {
				if (pg instanceof Error) throw pg
				return pg
			},
			log: {warn},
		})
		return {shadow, warn}
	}
	it("exports nothing until mounted, then every series at zero", () => {
		expect(renderFeedRunsShadowMetrics()).toBe("")
		make(null, null)
		expect(renderFeedRunsShadowMetrics()).toContain('result="absent"} 0')
	})
	it("counts match, mismatch and absent", async () => {
		const d = new Date(T)
		await make(d, value(1, `${T}/g`)).shadow.runOnce()
		const mismatch = make(new Date(d.getTime() + 5000), value(1, `${T}/g`))
		await mismatch.shadow.runOnce()
		await make(d, null).shadow.runOnce()
		expect([metric("match"), metric("mismatch"), metric("absent")]).toEqual(["1", "1", "1"])
		expect(JSON.stringify(mismatch.warn.mock.calls)).not.toMatch(/2026/)
	})
	it("counts a malformed row, a geo-db failure and the Postgres failures apart", async () => {
		await make(new Date(T), Buffer.from("junk")).shadow.runOnce()
		await make(new Date(T), new GeoDbError("client_failed", "x")).shadow.runOnce()
		await make(new Error("pg down"), null).shadow.runOnce()
		await make(Object.assign(new Error("t"), {code: "57014"}), null).shadow.runOnce()
		expect([
			metric("malformed"),
			metric("failed"),
			metric("reference_failed"),
			metric("reference_timeout"),
		]).toEqual(["1", "1", "1", "1"])
		expect(metric("mismatch")).toBe("0")
	})
	it("reads its config, off by default, and starts no timer when off", () => {
		expect(readFeedRunsShadowConfig({})).toEqual({enabled: false, intervalMs: 300_000})
		expect(
			readFeedRunsShadowConfig({GEODB_FEED_RUNS_SHADOW: "1", GEODB_FEED_RUNS_SHADOW_INTERVAL_MS: "5000"}),
		).toEqual({
			enabled: true,
			intervalMs: 5000,
		})
		const shadow = createFeedRunsShadow({
			config: {enabled: false, intervalMs: 1000},
			db: {} as Database,
			reader: readerOf(null),
			log: {warn: vi.fn()},
		})
		expect(shadow.start()).toBeTypeOf("function")
	})
})
