import {existsSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"

import {beforeEach, describe, expect, it, vi} from "vitest"

import type {GeoCardRow} from "../card"
import {GeoDbError} from "../client"
import {
	__resetEditMetaShadowMetricsForTests,
	EDIT_META_COLUMNS as C,
	compareEditMeta,
	createEditMetaReader,
	createEditMetaShadow,
	decodeEditMeta,
	EDIT_META_KIND,
	editMetaRecordId,
	readEditMetaShadowConfig,
	renderEditMetaShadowMetrics,
} from "../editMetaShadow"

const here = dirname(fileURLToPath(import.meta.url))
const E1 = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"
const E2 = "1a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"
const SPACE = "a".repeat(32)
const metric = (labels: string) => new RegExp(`${labels}\\} (\\d+)`).exec(renderEditMetaShadowMetrics())?.[1]
const settle = () => new Promise((r) => setTimeout(r, 0))

const rowsFor = (editId: string, block: string, name?: string, extra: GeoCardRow[] = []): GeoCardRow[] => [
	{prop: C.editId, raw: editId.replaceAll("-", ""), entity: editMetaRecordId(editId)},
	{prop: C.block, raw: block, entity: editMetaRecordId(editId)},
	{prop: C.sequence, raw: "3", entity: editMetaRecordId(editId)},
	...(name === undefined ? [] : [{prop: C.name, raw: name, entity: editMetaRecordId(editId)}]),
	...extra,
]

const config = {enabled: true, sampleEvery: 1, maxPerMinute: 1000, maxInFlight: 4}
const make = (readCards: ReturnType<typeof vi.fn>, over = {}) =>
	createEditMetaShadow({
		config: {...config, ...over},
		geoDb: {readCards},
		systemSpaceOf: async () => SPACE,
		log: {warn: vi.fn()},
	})

beforeEach(() => __resetEditMetaShadowMetricsForTests())

describe("pins", () => {
	it("match the feeder's recordKinds", () => {
		const file = join(here, "../../../../geodb-feeder/src/recordKinds.ts")
		if (!existsSync(file)) return
		const src = readFileSync(file, "utf8")
		expect(src.slice(src.indexOf('\teditMeta: {id: "'))).toContain(`id: "${EDIT_META_KIND}"`)
		const cols = src.slice(src.indexOf("\teditMeta: {\n"))
		for (const [prop, id] of Object.entries(C)) expect(cols).toContain(`${prop}: "${id}"`)
	})
	// Golden: the id the feeder's editMetaKeyed.ts derives for E1 (vitest, 2026-11-12).
	it("derives the same record id as the feeder", () => {
		expect(editMetaRecordId("0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d")).toBe("a0d20899949d4e67bdef59df4bbc4d67")
	})
})

describe("decodeEditMeta", () => {
	it("reads block, sequence and name", () => {
		expect(decodeEditMeta(rowsFor(E1, "123", "hello"))).toEqual({meta: {block: 123, sequence: 3, name: "hello"}})
	})
	it("has no name when the column is absent, and no record for no rows", () => {
		expect(decodeEditMeta(rowsFor(E1, "9"))).toEqual({meta: {block: 9, sequence: 3, name: null}})
		expect(decodeEditMeta([])).toBeNull()
	})
	it("is ambiguous on two values or a non-number block", () => {
		expect(decodeEditMeta(rowsFor(E1, "1", "a", [{prop: C.block, raw: "2"}]))).toEqual({ambiguous: true})
		expect(decodeEditMeta(rowsFor(E1, "x"))).toEqual({ambiguous: true})
	})
})

describe("reader", () => {
	it("reads one batch and keys results by edit id", async () => {
		const readCards = vi.fn(async () => [...rowsFor(E1, "10", "a"), ...rowsFor(E2, "11")])
		const got = await createEditMetaReader({geoDb: {readCards}, systemSpaceOf: async () => SPACE}).read([E1, E2])
		expect(readCards).toHaveBeenCalledTimes(1)
		expect(got.get(E1)).toEqual({meta: {block: 10, sequence: 3, name: "a"}})
		expect(got.get(E2)).toEqual({meta: {block: 11, sequence: 3, name: null}})
	})
})

describe("compareEditMeta", () => {
	const held = new Map([[E1, {meta: {block: 10, sequence: 3, name: "a"}}]])
	it("agrees on equal block and name", () => {
		expect(compareEditMeta([{editId: E1, blockNumber: "10", name: "a"}], held)).toEqual({
			fields: [],
			missing: false,
		})
	})
	it("names the differing fields", () => {
		expect(compareEditMeta([{editId: E1, blockNumber: "11", name: "b"}], held).fields).toEqual(["block", "name"])
	})
	it("flags a missing record, and skips name when not served", () => {
		expect(compareEditMeta([{editId: E1, blockNumber: "10"}, {editId: E2}], held)).toEqual({
			fields: [],
			missing: true,
		})
	})
})

describe("shadow", () => {
	const page = (blockNumber: string) => ({entityId: "b".repeat(32), versions: [{editId: E1, blockNumber, name: "a"}]})
	it("counts a match, a mismatch and a missing record", async () => {
		const shadow = make(vi.fn(async () => rowsFor(E1, "10", "a")))
		shadow.versions(page("10"))
		await settle()
		shadow.versions(page("11"))
		await settle()
		expect(metric('gaia_api_edit_meta_shadow_total\\{result="match"')).toBe("1")
		expect(metric('gaia_api_edit_meta_shadow_total\\{result="mismatch"')).toBe("1")
		expect(metric('mismatch_total\\{field="block"')).toBe("1")
		const empty = make(vi.fn(async () => []))
		empty.versions(page("10"))
		await settle()
		expect(metric('gaia_api_edit_meta_shadow_total\\{result="missing"')).toBe("1")
	})
	it("counts failed on a geo-db error and unsupported without a governance Space", async () => {
		make(vi.fn(async () => Promise.reject(new GeoDbError("timeout", "t")))).versions(page("10"))
		await settle()
		expect(metric('gaia_api_edit_meta_shadow_total\\{result="failed"')).toBe("1")
		createEditMetaShadow({
			config,
			geoDb: {readCards: vi.fn()},
			systemSpaceOf: async () => null,
			log: {warn: vi.fn()},
		}).versions(page("10"))
		await settle()
		expect(metric('gaia_api_edit_meta_shadow_total\\{result="unsupported"')).toBe("1")
	})
	it("samples and does nothing when disabled", async () => {
		const readCards = vi.fn(async () => rowsFor(E1, "10", "a"))
		const sampled = make(readCards, {sampleEvery: 2})
		sampled.versions(page("10"))
		sampled.versions(page("10"))
		await settle()
		expect(readCards).toHaveBeenCalledTimes(1)
		expect(metric('skipped_total\\{reason="sample"')).toBe("1")
		const off = vi.fn()
		make(off, {enabled: false}).versions(page("10"))
		await settle()
		expect(off).not.toHaveBeenCalled()
	})
	it("never throws into the response", () => {
		const shadow = make(vi.fn())
		expect(() => shadow.versions(undefined as never)).not.toThrow()
	})
})

describe("config and metrics", () => {
	it("is off by default and reads its env", () => {
		expect(readEditMetaShadowConfig({}).enabled).toBe(false)
		expect(
			readEditMetaShadowConfig({GEODB_EDIT_META_SHADOW: "on", GEODB_EDIT_META_SHADOW_SAMPLE_EVERY: "5"}),
		).toMatchObject({
			enabled: true,
			sampleEvery: 5,
		})
	})
	it("renders nothing until a shadow is made", () => {
		expect(renderEditMetaShadowMetrics()).toBe("")
		make(vi.fn())
		expect(renderEditMetaShadowMetrics()).toContain('gaia_api_edit_meta_shadow_total{result="match"} 0')
	})
})
