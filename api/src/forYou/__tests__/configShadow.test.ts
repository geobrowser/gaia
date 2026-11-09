import {existsSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"

import {beforeEach, describe, expect, it, vi} from "vitest"

import type {GeoCardRow} from "../../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../../geodb/client"
import {
	__resetForYouConfigMetricsForTests,
	FOR_YOU_CONFIG_COLUMNS as COL,
	createForYouConfigShadow,
	decodeForYouConfig,
	FOR_YOU_CONFIG_KIND,
	forYouConfigRecordId,
	readForYouConfigShadowConfig,
	renderForYouConfigMetrics,
} from "../configShadow"
import type {ForYouConfig} from "../rank"

const here = dirname(fileURLToPath(import.meta.url))
const served: ForYouConfig = {maxBoostDays: 6, interestHalfSaturation: 2, explorationShare: 0.1, maxTopics: 50}
const card = (o: Partial<Record<keyof typeof COL, string>> = {}): GeoCardRow[] =>
	Object.entries({
		maxBoostDays: "6",
		interestHalfSaturation: "2.0",
		explorationShare: "0.1",
		maxTopics: "50",
		...o,
	}).map(([k, raw]) => ({prop: COL[k as keyof typeof COL], raw}))
const metric = (name: string) => new RegExp(`${name}\\} (\\d+)`).exec(renderForYouConfigMetrics())?.[1]

const timeout = Object.assign(new Error("canceling statement due to statement timeout"), {code: "57014"})

function make(
	rows: () => Promise<GeoCardRow[]>,
	over: {
		space?: string | null
		enabled?: boolean
		settings?: () => Promise<{config: ForYouConfig; revision: number; tauSeconds: number}>
	} = {},
) {
	const warn = vi.fn()
	const geoDb: GeoDbCardClient = {readCard: vi.fn(rows)}
	const readSettings = vi.fn(over.settings ?? (async () => ({config: served, revision: 1, tauSeconds: 1})))
	const shadow = createForYouConfigShadow({
		config: {enabled: over.enabled ?? true, intervalMs: 1000},
		db: {execute: vi.fn()},
		geoDb,
		systemSpaceOf: async () => (over.space === undefined ? "a".repeat(32) : over.space),
		log: {warn},
		readSettings,
	})
	return {shadow, geoDb, warn, readSettings}
}

beforeEach(() => __resetForYouConfigMetricsForTests())

describe("pins", () => {
	it("match the feeder's recordKinds", () => {
		const file = join(here, "../../../../geodb-feeder/src/recordKinds.ts")
		if (!existsSync(file)) return
		const text = readFileSync(file, "utf8")
		expect(text).toContain(FOR_YOU_CONFIG_KIND)
		for (const id of Object.values(COL)) expect(text).toContain(id)
	})
	it("key the record by the single row", () => {
		expect(forYouConfigRecordId()).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
	})
})

describe("decodeForYouConfig", () => {
	it("reads the four values, numbers as geo-db prints them", () => {
		expect(decodeForYouConfig(card())).toEqual(served)
	})
	it("is null for a record that does not exist, and refuses a missing or double column", () => {
		expect(decodeForYouConfig([])).toBeNull()
		expect(() => decodeForYouConfig(card().slice(1))).toThrow()
		expect(() => decodeForYouConfig([...card(), {prop: COL.maxTopics, raw: "51"}])).toThrow()
		expect(() => decodeForYouConfig(card({maxTopics: "x"}))).toThrow()
	})
})

describe("shadow", () => {
	it("is silent and empty until mounted, and off starts no timer", async () => {
		expect(renderForYouConfigMetrics()).toBe("")
		const spy = vi.spyOn(globalThis, "setInterval")
		const {shadow, geoDb} = make(async () => card(), {enabled: false})
		shadow.start()()
		expect(spy).not.toHaveBeenCalled()
		expect(geoDb.readCard).not.toHaveBeenCalled()
		expect(metric('result="match"')).toBe("0")
		spy.mockRestore()
	})
	it("counts a match and a mismatch, from a Postgres read and the record", async () => {
		const a = make(async () => card())
		await a.shadow.runOnce()
		expect(metric('result="match"')).toBe("1")
		expect(a.readSettings).toHaveBeenCalledTimes(1)
		expect(a.geoDb.readCard).toHaveBeenCalledWith({geoSpaceId: "a".repeat(32), entityId: forYouConfigRecordId()})
		const b = make(async () => card({maxTopics: "80"}))
		await b.shadow.runOnce()
		expect(metric('result="mismatch"')).toBe("1")
		expect(b.warn).toHaveBeenCalled()
	})
	it("counts unsupported for a missing Space, a missing record and an ambiguous one", async () => {
		await make(async () => card(), {space: null}).shadow.runOnce()
		await make(async () => []).shadow.runOnce()
		await make(async () => card().slice(1)).shadow.runOnce()
		expect(metric('result="unsupported"')).toBe("3")
		for (const r of ["system_space_missing", "record_missing", "ambiguous_record"])
			expect(metric(`reason="${r}"`)).toBe("1")
	})
	it("counts a geo-db failure as failed, a Postgres one as reference_timeout or reference_failed", async () => {
		await make(async () => {
			throw new GeoDbError("timeout", "down")
		}).shadow.runOnce()
		expect(metric('result="failed"')).toBe("1")
		const slow = make(async () => card(), {
			settings: async () => {
				throw timeout
			},
		})
		await slow.shadow.runOnce()
		expect(metric('result="reference_timeout"')).toBe("1")
		expect(slow.geoDb.readCard).not.toHaveBeenCalled()
		await make(async () => card(), {
			settings: async () => {
				throw new Error("connection refused")
			},
		}).shadow.runOnce()
		expect(metric('result="reference_failed"')).toBe("1")
		expect(metric('result="failed"')).toBe("1")
	})
	it("runs on the timer without a request, never overlaps itself, and stops", async () => {
		vi.useFakeTimers()
		try {
			let release: (rows: GeoCardRow[]) => void = () => {}
			const a = make(
				() =>
					new Promise<GeoCardRow[]>((r) => {
						release = r
					}),
			)
			const stop = a.shadow.start()
			await vi.advanceTimersByTimeAsync(1000)
			expect(a.geoDb.readCard).toHaveBeenCalledTimes(1)
			await vi.advanceTimersByTimeAsync(2000)
			expect(a.geoDb.readCard).toHaveBeenCalledTimes(1)
			expect(metric('reason="busy"')).toBe("2")
			release(card())
			await vi.advanceTimersByTimeAsync(0)
			expect(metric('result="match"')).toBe("1")
			stop()
			await vi.advanceTimersByTimeAsync(5000)
			expect(a.geoDb.readCard).toHaveBeenCalledTimes(1)
		} finally {
			vi.useRealTimers()
		}
	})
	it("reads its flag off by default, and a five-minute interval", () => {
		expect(readForYouConfigShadowConfig({}).enabled).toBe(false)
		expect(readForYouConfigShadowConfig({GEODB_FOR_YOU_CONFIG_SHADOW: "1"}).enabled).toBe(true)
		expect(readForYouConfigShadowConfig({}).intervalMs).toBe(300_000)
		expect(readForYouConfigShadowConfig({GEODB_FOR_YOU_CONFIG_SHADOW_INTERVAL_MS: "5000"}).intervalMs).toBe(5000)
		expect(readForYouConfigShadowConfig({GEODB_FOR_YOU_CONFIG_SHADOW_INTERVAL_MS: "x"}).intervalMs).toBe(300_000)
	})
})
