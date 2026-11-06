import {existsSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"

import {beforeEach, describe, expect, it, vi} from "vitest"

import type {GeoCardRow} from "../../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../../geodb/client"
import {
	__resetEntityRankingConfigMetricsForTests,
	ENTITY_RANKING_CONFIG_COLUMNS as COL,
	createEntityRankingConfigShadow,
	decodeTauSeconds,
	ENTITY_RANKING_CONFIG_KIND,
	entityRankingConfigRecordId,
	readEntityRankingConfigShadowConfig,
	renderEntityRankingConfigMetrics,
} from "../entityRankingConfigShadow"

const here = dirname(fileURLToPath(import.meta.url))
const served = 100000
const card = (tau = "100000"): GeoCardRow[] => [
	{prop: COL.tauSeconds, raw: tau},
	// Other columns on the record are ignored.
	{prop: "0".repeat(32), raw: "x"},
]
const metric = (name: string) => new RegExp(`${name}\\} (\\d+)`).exec(renderEntityRankingConfigMetrics())?.[1]
const settle = () => new Promise((r) => setTimeout(r, 0))

function make(
	rows: () => Promise<GeoCardRow[]>,
	over: {
		space?: string | null
		enabled?: boolean
		intervalMs?: number
		maxInFlight?: number
		now?: () => number
	} = {},
) {
	const warn = vi.fn()
	const geoDb: GeoDbCardClient = {readCard: vi.fn(rows)}
	const shadow = createEntityRankingConfigShadow({
		config: {enabled: over.enabled ?? true, maxInFlight: over.maxInFlight ?? 2, intervalMs: over.intervalMs ?? 0},
		geoDb,
		systemSpaceOf: async () => (over.space === undefined ? "a".repeat(32) : over.space),
		log: {warn},
		now: over.now,
	})
	return {shadow, geoDb, warn}
}

beforeEach(() => __resetEntityRankingConfigMetricsForTests())

describe("pins", () => {
	it("match the feeder's recordKinds (kind and the tau column)", () => {
		const file = join(here, "../../../../geodb-feeder/src/recordKinds.ts")
		if (!existsSync(file)) return
		const text = readFileSync(file, "utf8")
		expect(text).toContain(ENTITY_RANKING_CONFIG_KIND)
		// The tau column sits under `tauSeconds` in the entityRankingConfig block, not just anywhere in the file.
		const block = text.slice(text.indexOf("\tentityRankingConfig: {\n"))
		expect(block).toContain(`tauSeconds: "${COL.tauSeconds}"`)
	})
	it("key the record by the single row", () => {
		expect(entityRankingConfigRecordId()).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
	})
})

describe("decodeTauSeconds", () => {
	it("reads tau_seconds, numbers as geo-db prints them", () => {
		expect(decodeTauSeconds(card())).toBe(100000)
		expect(decodeTauSeconds(card("100000.0"))).toBe(100000)
	})
	it("is null for a record that does not exist, and refuses a missing, double or non-numeric column", () => {
		expect(decodeTauSeconds([])).toBeNull()
		expect(() => decodeTauSeconds(card().slice(1))).toThrow()
		expect(() => decodeTauSeconds([...card(), {prop: COL.tauSeconds, raw: "45000"}])).toThrow()
		expect(() => decodeTauSeconds(card("x"))).toThrow()
	})
})

describe("shadow", () => {
	it("is silent and empty until mounted, and off does nothing", async () => {
		expect(renderEntityRankingConfigMetrics()).toBe("")
		const {shadow, geoDb} = make(async () => card(), {enabled: false})
		shadow.compare(served)
		await settle()
		expect(geoDb.readCard).not.toHaveBeenCalled()
		expect(metric('result="match"')).toBe("0")
	})
	it("counts a match and a mismatch", async () => {
		const a = make(async () => card())
		a.shadow.compare(served)
		await settle()
		expect(metric('result="match"')).toBe("1")
		expect(a.geoDb.readCard).toHaveBeenCalledWith({
			geoSpaceId: "a".repeat(32),
			entityId: entityRankingConfigRecordId(),
		})
		const b = make(async () => card("45000"))
		b.shadow.compare(served)
		await settle()
		expect(metric('result="mismatch"')).toBe("1")
		expect(b.warn).toHaveBeenCalled()
	})
	it("counts unsupported for a missing Space, a missing record and an ambiguous one", async () => {
		make(async () => card(), {space: null}).shadow.compare(served)
		make(async () => []).shadow.compare(served)
		make(async () => card().slice(1)).shadow.compare(served)
		await settle()
		expect(metric('result="unsupported"')).toBe("3")
		for (const r of ["system_space_missing", "record_missing", "ambiguous_record"])
			expect(metric(`reason="${r}"`)).toBe("1")
	})
	it("counts a failed read without throwing", async () => {
		make(async () => {
			throw new GeoDbError("timeout", "down")
		}).shadow.compare(served)
		await settle()
		expect(metric('result="failed"')).toBe("1")
	})
	it("compares at most once an interval, and drops past maxInFlight", async () => {
		let t = 0
		const a = make(async () => card(), {intervalMs: 1000, now: () => t})
		a.shadow.compare(served)
		a.shadow.compare(served)
		await settle()
		expect(a.geoDb.readCard).toHaveBeenCalledTimes(1)
		t = 1000
		a.shadow.compare(served)
		await settle()
		expect(a.geoDb.readCard).toHaveBeenCalledTimes(2)
		const b = make(() => new Promise<GeoCardRow[]>(() => {}), {maxInFlight: 1})
		b.shadow.compare(served)
		await settle()
		b.shadow.compare(served)
		expect(metric('reason="busy"')).toBe("1")
	})
	it("reads its flag off by default", () => {
		expect(readEntityRankingConfigShadowConfig({}).enabled).toBe(false)
		expect(readEntityRankingConfigShadowConfig({GEODB_ENTITY_RANKING_CONFIG_SHADOW: "1"}).enabled).toBe(true)
		// The for-you flag does not turn it on.
		expect(readEntityRankingConfigShadowConfig({GEODB_FOR_YOU_CONFIG_SHADOW: "1"}).enabled).toBe(false)
	})
})
