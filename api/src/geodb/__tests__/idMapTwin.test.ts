import {afterEach, describe, expect, it, vi} from "vitest"
import {conformingId} from "../card"
import {GeoDbError} from "../client"
import type {GaiaIdsOf} from "../idMap"
import {
	__resetIdMapTwinMetricsForTests,
	compareLegacyRows,
	type IdMapTwinConfig,
	readIdMapTwinConfig,
	renderIdMapTwinMetrics,
	withIdMapTwin,
} from "../idMapTwin"

const LEGACY = "990a4147a92855e88e630e542faf1c72"
const LEGACY_GEO = conformingId("entity", LEGACY)
const LEGACY2 = "01a1a8c0000070008000000000000001"
const LEGACY2_GEO = conformingId("entity", LEGACY2)
const NATIVE = "6b9f649e38b24b9a8e3c1b6a0b3c3c3a"
const STRANGER = "0d4f1a2b3c4d4e5f8a6b7c8d9e0f1a2b"

afterEach(() => __resetIdMapTwinMetricsForTests())

const on: IdMapTwinConfig = {enabled: true, sample: 1, timeoutMs: 100, cooldownMs: 1000}
const row = (entity: string) => ({prop: "p", raw: "<bytes>", entity})
const sql =
	(pairs: [string, string][]): GaiaIdsOf =>
	async () =>
		new Map(pairs)
const settle = () => new Promise((r) => setTimeout(r, 0))
const metric = (name: string) => {
	const m = new RegExp(`^${name.replace(/[{}"]/g, "\\$&")} (\\S+)$`, "m").exec(renderIdMapTwinMetrics())
	return m ? Number(m[1]) : null
}
const result = (r: string) => metric(`gaia_api_geodb_id_map_twin_ids_total{result="${r}"}`)
const skipped = (r: string) => metric(`gaia_api_geodb_id_map_twin_skipped_total{reason="${r}"}`)

function twin(rows: ReturnType<typeof row>[] | Error, pairs: [string, string][], config = on, extra = {}) {
	const readLegacyIds = vi.fn(async () => {
		if (rows instanceof Error) throw rows
		return rows
	})
	const warn = vi.fn()
	const map = withIdMapTwin(sql(pairs), {client: {readLegacyIds}, config, log: {warn}, ...extra})
	return {map, readLegacyIds, warn}
}

describe("compareLegacyRows", () => {
	const pairs = [
		{geo: LEGACY_GEO, gaia: LEGACY},
		{geo: NATIVE, gaia: NATIVE},
	]
	it("a v4 id answers as itself and a mapped id as its keyed id", () => {
		expect(compareLegacyRows(pairs, [LEGACY_GEO, NATIVE])).toMatchObject({match: 2, mismatch: 0})
	})
	it("a non-v4 id with no row is unmapped, not a mismatch", () => {
		expect(compareLegacyRows(pairs, [NATIVE])).toMatchObject({match: 1, unmapped_in_geodb: 1, mismatch: 0})
	})
	it("a non-v4 id keyed differently is a mismatch", () => {
		expect(compareLegacyRows(pairs, [STRANGER, NATIVE])).toMatchObject({
			match: 1,
			mismatch: 1,
			unmapped_in_geodb: 0,
			mismatched: [LEGACY],
		})
	})
	it("a v4 id with no row is a mismatch", () => {
		expect(compareLegacyRows(pairs, [LEGACY_GEO])).toMatchObject({match: 1, mismatch: 1, mismatched: [NATIVE]})
	})
	it("two keyed ids for one original are a conflict", () => {
		expect(compareLegacyRows(pairs, [LEGACY_GEO, STRANGER, NATIVE])).toMatchObject({
			match: 2,
			mismatch: 0,
			conflict: 1,
		})
	})
})

describe("withIdMapTwin", () => {
	it("is inert when the flag is off: the same function, no read, no metrics", async () => {
		const inner = sql([[NATIVE, NATIVE]])
		const readLegacyIds = vi.fn()
		const map = withIdMapTwin(inner, {
			client: {readLegacyIds},
			config: readIdMapTwinConfig({}),
			log: {warn: vi.fn()},
		})
		expect(map).toBe(inner)
		await map([NATIVE])
		expect(readLegacyIds).not.toHaveBeenCalled()
		expect(renderIdMapTwinMetrics()).toBe("")
	})

	it("reads the flag and its bounds from the environment", () => {
		expect(readIdMapTwinConfig({GEODB_ID_MAP_TWIN: "on"})).toEqual({
			enabled: true,
			sample: 0.05,
			timeoutMs: 2000,
			cooldownMs: 30_000,
		})
		expect(
			readIdMapTwinConfig({
				GEODB_ID_MAP_TWIN: "1",
				GEODB_ID_MAP_TWIN_SAMPLE: "0",
				GEODB_ID_MAP_TWIN_TIMEOUT_MS: "50",
				GEODB_ID_MAP_TWIN_COOLDOWN_MS: "9",
			}),
		).toEqual({enabled: true, sample: 0, timeoutMs: 50, cooldownMs: 9})
		expect(readIdMapTwinConfig({GEODB_ID_MAP_TWIN: "off"}).enabled).toBe(false)
	})

	it("stays off when the client cannot read legacy ids", async () => {
		const inner = sql([])
		const warn = vi.fn()
		expect(withIdMapTwin(inner, {client: {}, config: on, log: {warn}})).toBe(inner)
		expect(warn).toHaveBeenCalled()
	})

	it("counts a v4 passthrough and a mapped id as matches, and returns Postgres's answer untouched", async () => {
		const {map, readLegacyIds, warn} = twin(
			[row(NATIVE), row(LEGACY_GEO)],
			[
				[NATIVE, NATIVE],
				[LEGACY_GEO, LEGACY],
			],
		)
		const got = await map([NATIVE, LEGACY_GEO])
		expect(Object.fromEntries(got)).toEqual({[NATIVE]: NATIVE, [LEGACY_GEO]: LEGACY})
		await settle()
		expect(readLegacyIds).toHaveBeenCalledWith({originalIds: [NATIVE, LEGACY], timeoutMs: 100})
		expect(result("match")).toBe(2)
		expect(result("mismatch")).toBe(0)
		expect(warn).not.toHaveBeenCalled()
	})

	it("counts an id with no map statement as unmapped_in_geodb, apart from a mismatch", async () => {
		const {map, warn} = twin([], [[LEGACY_GEO, LEGACY]])
		await map([LEGACY_GEO])
		await settle()
		expect(result("unmapped_in_geodb")).toBe(1)
		expect(result("mismatch")).toBe(0)
		expect(warn).not.toHaveBeenCalled()
	})

	it("counts a different keyed id as a mismatch and logs it", async () => {
		const {map, warn} = twin([row(STRANGER)], [[LEGACY_GEO, LEGACY]])
		await map([LEGACY_GEO])
		await settle()
		expect(result("mismatch")).toBe(1)
		expect(warn).toHaveBeenCalledWith(
			expect.any(String),
			expect.objectContaining({event: "geodb_id_map_twin_mismatch", mismatch: 1}),
		)
	})

	it("counts two keyed ids for one original as a conflict", async () => {
		const {map} = twin([row(LEGACY_GEO), row(STRANGER)], [[LEGACY_GEO, LEGACY]])
		await map([LEGACY_GEO])
		await settle()
		expect(result("match")).toBe(1)
		expect(result("conflict")).toBe(1)
	})

	it("leaves ids Postgres missed alone, and asks nothing when it found none", async () => {
		const {map, readLegacyIds} = twin([], [])
		await map([STRANGER])
		await settle()
		expect(readLegacyIds).not.toHaveBeenCalled()
	})

	it("asks about 32 ids at most, each original once", async () => {
		const many: [string, string][] = Array.from({length: 40}, (_, i) => {
			const id = i.toString(16).padStart(32, "4")
			return [id, id]
		})
		const {map, readLegacyIds} = twin([], many)
		await map(many.map(([id]) => id))
		await settle()
		const asked = (readLegacyIds.mock.calls[0] as unknown as [{originalIds: string[]}])[0].originalIds
		expect(asked).toHaveLength(32)
		expect(new Set(asked).size).toBe(32)
	})

	it("counts a failed read, then cools down and skips until the cool-down ends", async () => {
		let clock = 1000
		const {map, readLegacyIds, warn} = twin(new GeoDbError("timeout", "exceeded"), [[NATIVE, NATIVE]], on, {
			now: () => clock,
		})
		await map([NATIVE])
		await settle()
		expect(result("failed")).toBe(1)
		expect(warn).toHaveBeenCalledWith(expect.any(String), expect.objectContaining({reason: "timeout"}))
		await map([NATIVE])
		await settle()
		expect(readLegacyIds).toHaveBeenCalledTimes(1)
		expect(skipped("cooldown")).toBe(1)
		clock += 1000
		await map([NATIVE])
		await settle()
		expect(readLegacyIds).toHaveBeenCalledTimes(2)
	})

	it("never delays or fails the request: a read that never ends is not awaited", async () => {
		const readLegacyIds = vi.fn(() => new Promise<never>(() => {}))
		const map = withIdMapTwin(sql([[NATIVE, NATIVE]]), {client: {readLegacyIds}, config: on, log: {warn: vi.fn()}})
		await expect(map([NATIVE])).resolves.toEqual(new Map([[NATIVE, NATIVE]]))
		// The one read is still in flight, so the next call is skipped, not queued.
		await map([NATIVE])
		expect(readLegacyIds).toHaveBeenCalledTimes(1)
		expect(skipped("busy")).toBe(1)
	})

	it("passes inner's failure through and reads nothing", async () => {
		const readLegacyIds = vi.fn()
		const map = withIdMapTwin(async () => Promise.reject(new Error("pg down")), {
			client: {readLegacyIds},
			config: on,
			log: {warn: vi.fn()},
		})
		await expect(map([NATIVE])).rejects.toThrow("pg down")
		expect(readLegacyIds).not.toHaveBeenCalled()
	})

	it("samples: a call outside the fraction is counted and not read", async () => {
		const {map, readLegacyIds} = twin([], [[NATIVE, NATIVE]], {...on, sample: 0.25}, {random: () => 0.9})
		await map([NATIVE])
		await settle()
		expect(readLegacyIds).not.toHaveBeenCalled()
		expect(skipped("sampled_out")).toBe(1)
	})

	it("exports reads and their outcomes", async () => {
		const {map} = twin([row(LEGACY2_GEO)], [[LEGACY2_GEO, LEGACY2]])
		await map([LEGACY2_GEO])
		await settle()
		expect(metric('gaia_api_geodb_id_map_twin_reads_total{outcome="ok"}')).toBe(1)
		expect(metric('gaia_api_geodb_id_map_twin_reads_total{outcome="error"}')).toBe(0)
	})
})
