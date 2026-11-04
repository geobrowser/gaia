import {readFileSync} from "node:fs"
import path from "node:path"
import {afterEach, describe, expect, it, vi} from "vitest"
import {conformingId} from "../card"
import {__resetIdMapMetricsForTests, createGaiaIdMap, type IdMapRow, renderIdMapMetrics} from "../idMap"

const LEGACY = "990a4147a92855e88e630e542faf1c72"
const NATIVE = "6b9f649e38b24b9a8e3c1b6a0b3c3c3a"
const UNKNOWN = "0d4f1a2b3c4d4e5f8a6b7c8d9e0f1a2b"

afterEach(() => __resetIdMapMetricsForTests())

describe("geodb.conform (migration 0116) and conformingId agree", () => {
	// The same pairs are asserted against geodb.conform in drizzle/tests/0116_geodb_id_map.sql.
	const pairs: [string, string][] = [
		["990a4147-a928-55e8-8e63-0e542faf1c72", "a523681b-ad07-4d0d-9b43-e0f6d117406a"],
		["01a1a8c0-0000-7000-8000-000000000001", "facd0857-ad85-49a6-968b-410df35ba70a"],
		["ffffffff-ffff-ffff-ffff-ffffffffffff", "d4f3d63b-ac72-4230-8c2b-906b54a98da2"],
		["6b9f649e-38b2-4b9a-8e3c-1b6a0b3c3c3a", "6b9f649e-38b2-4b9a-8e3c-1b6a0b3c3c3a"],
	]

	it.each(pairs)("conformingId('entity', %s)", (gaia, geo) => {
		expect(conformingId("entity", gaia)).toBe(geo.replaceAll("-", ""))
	})

	it("the SQL suite asserts the same pairs", () => {
		const suite = readFileSync(path.resolve(__dirname, "../../../drizzle/tests/0116_geodb_id_map.sql"), "utf8")
		for (const [gaia, geo] of pairs) expect(suite).toContain(`geodb.conform('entity', '${gaia}') = '${geo}'`)
	})
})

describe("createGaiaIdMap", () => {
	const rows: IdMapRow[] = [
		{geo: conformingId("entity", LEGACY), gaia: LEGACY, legacy: true},
		{geo: NATIVE, gaia: NATIVE, legacy: false},
	]
	const query = () => vi.fn(async (ids: readonly string[]) => rows.filter((r) => ids.includes(r.geo)))

	it("maps legacy and native ids, leaves a miss out, and asks once for duplicates", async () => {
		const q = query()
		const map = createGaiaIdMap(q)
		const got = await map([conformingId("entity", LEGACY), NATIVE, UNKNOWN, NATIVE])
		expect(Object.fromEntries(got)).toEqual({[conformingId("entity", LEGACY)]: LEGACY, [NATIVE]: NATIVE})
		expect(q).toHaveBeenCalledWith([conformingId("entity", LEGACY), NATIVE, UNKNOWN])
	})

	it("caches answers but asks for a miss again", async () => {
		const q = query()
		const map = createGaiaIdMap(q)
		await map([NATIVE, UNKNOWN])
		await map([NATIVE, UNKNOWN])
		expect(q).toHaveBeenLastCalledWith([UNKNOWN])
		await map([NATIVE])
		expect(q).toHaveBeenCalledTimes(2)
		expect(renderIdMapMetrics()).toContain('gaia_api_geodb_id_map_ids_total{result="cached"} 2')
		expect(renderIdMapMetrics()).toContain('gaia_api_geodb_id_map_ids_total{result="miss"} 2')
		expect(renderIdMapMetrics()).toContain('gaia_api_geodb_id_map_ids_total{result="native"} 1')
		expect(renderIdMapMetrics()).toContain('gaia_api_geodb_id_map_queries_total{outcome="ok"} 2')
	})

	it("bounds the cache", async () => {
		const q = query()
		const map = createGaiaIdMap(q, {cacheSize: 1})
		// The read answers the legacy id first, so the native one is what the cache keeps.
		await map([NATIVE, conformingId("entity", LEGACY)])
		await map([NATIVE])
		expect(q).toHaveBeenCalledTimes(1)
		await map([conformingId("entity", LEGACY)])
		expect(q).toHaveBeenCalledTimes(2)
	})

	it("turns a failed read into an id_map error and counts it", async () => {
		const map = createGaiaIdMap(async () => Promise.reject(new Error("connection refused")))
		await expect(map([NATIVE])).rejects.toMatchObject({reason: "id_map"})
		expect(renderIdMapMetrics()).toContain('gaia_api_geodb_id_map_queries_total{outcome="error"} 1')
	})

	it("exports nothing until a map exists", () => {
		expect(renderIdMapMetrics()).toBe("")
	})
})
