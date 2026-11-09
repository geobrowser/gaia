import {existsSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"

import {beforeEach, describe, expect, it, vi} from "vitest"

import type {Database} from "../../forYou/queries"
import type {GeoCardRow} from "../card"
import {GeoDbError} from "../client"
import {
	__resetEntityFeedConfigShadowMetricsForTests,
	AmbiguousConfigError,
	ENTITY_FEED_BLOCK_COLUMNS as BC,
	type ConfigRow,
	compareConfigRow,
	createEntityFeedConfigReaders,
	createEntityFeedConfigShadow,
	decodeEntityFeedBlock,
	decodeEntityTypeExclusion,
	decodeEntityTypeWeight,
	ENTITY_FEED_BLOCK_KIND,
	ENTITY_TYPE_EXCLUSION_KIND,
	ENTITY_TYPE_WEIGHT_KIND,
	entityFeedBlockRecordId,
	entityTypeExclusionRecordId,
	entityTypeWeightRecordId,
	readEntityFeedConfigShadowConfig,
	renderEntityFeedConfigShadowMetrics,
	sampleConfigRows,
	ENTITY_TYPE_WEIGHT_COLUMNS as WC,
	ENTITY_TYPE_EXCLUSION_COLUMNS as XC,
} from "../entityFeedConfigShadow"

const here = dirname(fileURLToPath(import.meta.url))
const T = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"
const SPACE = "a".repeat(32)
const metric = (labels: string) => new RegExp(`${labels}\\} (\\d+)`).exec(renderEntityFeedConfigShadowMetrics())?.[1]

beforeEach(() => __resetEntityFeedConfigShadowMetricsForTests())

describe("pins", () => {
	it("match the feeder's recordKinds (kinds and columns)", () => {
		const file = join(here, "../../../../geodb-feeder/src/recordKinds.ts")
		if (!existsSync(file)) return
		const src = readFileSync(file, "utf8")
		const kind = (name: string) => src.slice(src.indexOf(`\t${name}: {id: "`))
		expect(kind("entityTypeWeight")).toContain(`id: "${ENTITY_TYPE_WEIGHT_KIND}"`)
		expect(kind("entityTypeExclusion")).toContain(`id: "${ENTITY_TYPE_EXCLUSION_KIND}"`)
		expect(kind("entityFeedBlock")).toContain(`id: "${ENTITY_FEED_BLOCK_KIND}"`)
		const cols = (name: string) => src.slice(src.indexOf(`\t${name}: {\n`))
		for (const [name, c] of [
			["entityTypeWeight", WC],
			["entityTypeExclusion", XC],
			["entityFeedBlock", BC],
		] as const)
			for (const [prop, id] of Object.entries(c)) expect(cols(name)).toContain(`${prop}: "${id}"`)
	})
	// Golden: the ids the feeder's entityFeedConfigKeyed.ts derives for T (bun, 2026-11-08).
	it("derive the same record ids as the feeder", () => {
		expect(entityTypeWeightRecordId(T)).toBe("6b22fe2710394934b8bd4e7290afc653")
		expect(entityTypeExclusionRecordId(T)).toBe("9cc5fae79a714306a17fba9de544ac33")
		expect(entityFeedBlockRecordId(T)).toBe("47e861dcc4164f33913946321bc5d101")
		expect(entityTypeWeightRecordId(T.replaceAll("-", ""))).toBe(entityTypeWeightRecordId(T))
	})
})

describe("decoders", () => {
	const weightCard = (w = "1.5", note?: string): GeoCardRow[] => [
		{prop: WC.typeId, raw: T},
		{prop: WC.weight, raw: w},
		...(note === undefined ? [] : [{prop: WC.note, raw: note}]),
	]
	it("decode a weight record, numbers as geo-db prints them", () => {
		expect(decodeEntityTypeWeight(weightCard())).toEqual({weight: 1.5, note: null})
		expect(decodeEntityTypeWeight(weightCard("2.0", "boost"))).toEqual({weight: 2, note: "boost"})
		expect(decodeEntityTypeWeight([])).toBeNull()
	})
	it("refuse a weight record with no, a double or a non-numeric weight", () => {
		expect(() => decodeEntityTypeWeight(weightCard().slice(0, 1))).toThrow(AmbiguousConfigError)
		expect(() => decodeEntityTypeWeight([...weightCard(), {prop: WC.weight, raw: "3"}])).toThrow(
			AmbiguousConfigError,
		)
		expect(() => decodeEntityTypeWeight(weightCard("x"))).toThrow(AmbiguousConfigError)
	})
	it("decode exclusion and blocklist presence records with an optional note or reason", () => {
		expect(decodeEntityTypeExclusion([])).toBeNull()
		expect(decodeEntityTypeExclusion([{prop: XC.typeId, raw: T}])).toEqual({note: null})
		expect(
			decodeEntityTypeExclusion([
				{prop: XC.typeId, raw: T},
				{prop: XC.note, raw: "n"},
			]),
		).toEqual({note: "n"})
		expect(
			decodeEntityFeedBlock([
				{prop: BC.entityId, raw: T},
				{prop: BC.reason, raw: "spam"},
			]),
		).toEqual({reason: "spam"})
		expect(decodeEntityFeedBlock([])).toBeNull()
		expect(() =>
			decodeEntityFeedBlock([
				{prop: BC.reason, raw: "a"},
				{prop: BC.reason, raw: "b"},
			]),
		).toThrow(AmbiguousConfigError)
	})
})

describe("readers", () => {
	it("read the record in the config Space by its derived id", async () => {
		const readCard = vi.fn(async () => [{prop: XC.typeId, raw: T}] as GeoCardRow[])
		const readers = createEntityFeedConfigReaders({geoDb: {readCard}, systemSpaceOf: async () => SPACE})
		expect(await readers.typeExclusion(T)).toEqual({note: null})
		expect(readCard).toHaveBeenCalledWith({geoSpaceId: SPACE, entityId: entityTypeExclusionRecordId(T)})
		await readers.feedBlock(T)
		expect(readCard).toHaveBeenLastCalledWith({geoSpaceId: SPACE, entityId: entityFeedBlockRecordId(T)})
	})
	it("answer null for a record geo-db does not hold, and throw when the Space is not mapped", async () => {
		const readers = createEntityFeedConfigReaders({
			geoDb: {readCard: async () => []},
			systemSpaceOf: async () => SPACE,
		})
		expect(await readers.typeWeight(T)).toBeNull()
		const none = createEntityFeedConfigReaders({geoDb: {readCard: async () => []}, systemSpaceOf: async () => null})
		await expect(none.typeWeight(T)).rejects.toThrow()
	})
})

describe("compareConfigRow", () => {
	const w = (weight: string, note: string | null): ConfigRow => ({table: "entity_type_weights", key: T, weight, note})
	it("holds a weight within tolerance and the note exactly", () => {
		expect(compareConfigRow(w("1.50", null), {weight: 1.5, note: null})).toBe("match")
		expect(compareConfigRow(w("1.5", null), {weight: 1.6, note: null})).toBe("mismatch")
		expect(compareConfigRow(w("1.5", "a"), {weight: 1.5, note: null})).toBe("mismatch")
		expect(compareConfigRow(w("1.5", null), null)).toBe("missing")
	})
	it("holds exclusion notes and block reasons exactly", () => {
		expect(compareConfigRow({table: "entity_type_exclusions", key: T, note: "n"}, {note: "n"})).toBe("match")
		expect(compareConfigRow({table: "entity_type_exclusions", key: T, note: null}, {note: "n"})).toBe("mismatch")
		expect(compareConfigRow({table: "entity_feed_blocklist", key: T, reason: "r"}, {reason: "x"})).toBe("mismatch")
		expect(compareConfigRow({table: "entity_feed_blocklist", key: T, reason: null}, null)).toBe("missing")
	})
})

describe("config", () => {
	it("is off by default, one flag per table", () => {
		expect(Object.values(readEntityFeedConfigShadowConfig({}).enabled)).toEqual([false, false, false])
		const c = readEntityFeedConfigShadowConfig({
			GEODB_ENTITY_FEED_BLOCKLIST_SHADOW: "1",
			GEODB_ENTITY_TYPE_WEIGHTS_SHADOW: "0",
		})
		expect(c.enabled).toEqual({
			entity_type_weights: false,
			entity_type_exclusions: false,
			entity_feed_blocklist: true,
		})
		expect(c.sample).toBe(20)
	})
})

describe("sampleConfigRows", () => {
	it("maps rows per table and drops unusable ones", async () => {
		const db = {
			execute: async () => ({
				rows: [
					{key: T, weight: "1.5", note: null},
					{key: null, weight: "2"},
				],
			}),
		} as unknown as Database
		expect(await sampleConfigRows(db, "entity_type_weights", 5)).toEqual([
			{table: "entity_type_weights", key: T, weight: "1.5", note: null},
		])
	})
})

describe("shadow", () => {
	const allOn = {
		enabled: {entity_type_weights: true, entity_type_exclusions: true, entity_feed_blocklist: true},
		intervalMs: 60_000,
		sample: 5,
	}
	const rows: Record<string, ConfigRow[]> = {
		entity_type_weights: [{table: "entity_type_weights", key: T, weight: "1.5", note: null}],
		entity_type_exclusions: [{table: "entity_type_exclusions", key: T, note: null}],
		entity_feed_blocklist: [{table: "entity_feed_blocklist", key: T, reason: "r"}],
	}
	const make = (readers: Parameters<typeof createEntityFeedConfigShadow>[0]["readers"], config = allOn) => {
		const warn = vi.fn()
		const shadow = createEntityFeedConfigShadow({
			config,
			db: {} as Database,
			readers,
			log: {warn},
			sampleRows: async (_db, table) => rows[table] as ConfigRow[],
		})
		return {shadow, warn}
	}
	it("exports nothing until mounted, then only the enabled tables", async () => {
		expect(renderEntityFeedConfigShadowMetrics()).toBe("")
		make(
			{typeWeight: async () => null, typeExclusion: async () => null, feedBlock: async () => null},
			{...allOn, enabled: {...allOn.enabled, entity_type_exclusions: false, entity_feed_blocklist: false}},
		)
		expect(renderEntityFeedConfigShadowMetrics()).toContain('table="entity_type_weights",result="match"} 0')
		expect(renderEntityFeedConfigShadowMetrics()).not.toContain('table="entity_feed_blocklist"')
	})
	it("counts match, mismatch and missing per table, logging counts only", async () => {
		const {shadow, warn} = make({
			typeWeight: async () => ({weight: 1.5, note: null}),
			typeExclusion: async () => ({note: "different"}),
			feedBlock: async () => null,
		})
		await shadow.runOnce()
		expect(metric('table="entity_type_weights",result="match"')).toBe("1")
		expect(metric('table="entity_type_exclusions",result="mismatch"')).toBe("1")
		expect(metric('table="entity_feed_blocklist",result="missing"')).toBe("1")
		expect(JSON.stringify(warn.mock.calls)).not.toContain(T)
	})
	it("counts unsupported, ambiguous (mismatch) and failed reads without throwing", async () => {
		const {ConfigSpaceMissing} = await import("../entityFeedConfigShadow")
		const {shadow} = make({
			typeWeight: async () => {
				throw new ConfigSpaceMissing()
			},
			typeExclusion: async () => {
				throw new AmbiguousConfigError("x")
			},
			feedBlock: async () => {
				throw new GeoDbError("timeout", "down")
			},
		})
		await shadow.runOnce()
		expect(metric('table="entity_type_weights",result="unsupported"')).toBe("1")
		expect(metric('table="entity_type_exclusions",result="mismatch"')).toBe("1")
		expect(metric('table="entity_feed_blocklist",result="failed"')).toBe("1")
	})
	it("counts a Postgres sample failure as reference_timeout or reference_failed, not failed", async () => {
		const readers = {typeWeight: vi.fn(), typeExclusion: vi.fn(), feedBlock: vi.fn()}
		const errors = [
			Object.assign(new Error("canceling statement due to statement timeout"), {code: "57014"}),
			new Error("connection refused"),
			new Error("connection refused"),
		]
		let n = 0
		const shadow = createEntityFeedConfigShadow({
			config: allOn,
			db: {} as Database,
			readers,
			log: {warn: vi.fn()},
			sampleRows: async () => {
				throw errors[n++]
			},
		})
		await shadow.runOnce()
		expect(metric('table="entity_type_weights",result="reference_timeout"')).toBe("1")
		expect(metric('table="entity_type_exclusions",result="reference_failed"')).toBe("1")
		expect(metric('table="entity_feed_blocklist",result="reference_failed"')).toBe("1")
		expect(metric('table="entity_type_weights",result="failed"')).toBe("0")
		expect(readers.typeWeight).not.toHaveBeenCalled()
	})
	it("does nothing when every flag is off", async () => {
		const typeWeight = vi.fn()
		const off = {
			...allOn,
			enabled: {entity_type_weights: false, entity_type_exclusions: false, entity_feed_blocklist: false},
		}
		const {shadow} = make({typeWeight, typeExclusion: vi.fn(), feedBlock: vi.fn()}, off)
		await shadow.runOnce()
		expect(typeWeight).not.toHaveBeenCalled()
		expect(shadow.start()).toBeTypeOf("function")
		expect(renderEntityFeedConfigShadowMetrics()).toBe("")
	})
})
