import {existsSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {forYouConfigRecordId} from "../../forYou/configShadow"
import {entityRankingConfigRecordId} from "../../forYou/entityRankingConfigShadow"
import type {Database} from "../../forYou/queries"
import type {GeoCardRow} from "../card"
import {GeoDbError} from "../client"
import {
	__resetConfigRevisionShadowMetricsForTests,
	ConfigRecordMissing,
	compareMovement,
	createConfigRevisionReader,
	createConfigRevisionShadow,
	digestConfigRecords,
	INTEREST_CONFIG_KIND,
	INTEREST_SIGNAL_KINDS,
	INTEREST_SIGNAL_WEIGHT_KIND,
	interestConfigRecordId,
	interestSignalWeightRecordId,
	readConfigRevisionShadowConfig,
	renderConfigRevisionShadowMetrics,
} from "../configRevisionShadow"
import {ConfigSpaceMissing} from "../entityFeedConfigShadow"

const here = dirname(fileURLToPath(import.meta.url))
const SPACE = "a".repeat(32)
const metric = (result: string) =>
	new RegExp(`result="${result}"\\} (\\d+)`).exec(renderConfigRevisionShadowMetrics())?.[1]
const row = (prop: string, raw: string): GeoCardRow => ({prop, raw})

beforeEach(() => __resetConfigRevisionShadowMetricsForTests())

describe("pins", () => {
	it("match the feeder's recordKinds", () => {
		const file = join(here, "../../../../geodb-feeder/src/recordKinds.ts")
		if (!existsSync(file)) return
		const src = readFileSync(file, "utf8")
		expect(src).toContain(`interestConfig: {id: "${INTEREST_CONFIG_KIND}"`)
		expect(src).toContain(`interestSignalWeight: {id: "${INTEREST_SIGNAL_WEIGHT_KIND}"`)
	})
	it("name the kinds the schema seeds", () => {
		const file = join(here, "../../../drizzle/0100_user_topic_interest.sql")
		if (!existsSync(file)) return
		const sqlText = readFileSync(file, "utf8")
		for (const kind of INTEREST_SIGNAL_KINDS) expect(sqlText).toContain(`('${kind}'`)
	})
	it("derive distinct record ids", () => {
		const ids = new Set([interestConfigRecordId(), ...INTEREST_SIGNAL_KINDS.map(interestSignalWeightRecordId)])
		expect(ids.size).toBe(1 + INTEREST_SIGNAL_KINDS.length)
		for (const id of ids) expect(id).toMatch(/^[0-9a-f]{32}$/)
	})
})

describe("digestConfigRecords", () => {
	const a = {id: "r1", rows: [row("p1", "1"), row("p2", "2")]}
	const b = {id: "r2", rows: [row("p1", "3")]}
	it("ignores read order", () => {
		expect(digestConfigRecords([a, b])).toEqual(
			digestConfigRecords([{...b}, {id: "r1", rows: [...a.rows].reverse()}]),
		)
	})
	it("changes when a value changes, and returns when it is reverted", () => {
		const before = digestConfigRecords([a, b])
		const edited = digestConfigRecords([a, {id: "r2", rows: [row("p1", "4")]}])
		expect(edited.digest).not.toBe(before.digest)
		expect(digestConfigRecords([a, {id: "r2", rows: [row("p1", "3")]}])).toEqual(before)
	})
	it("tells which record holds a statement, and a missing record from an empty one only by its absence of rows", () => {
		const moved = digestConfigRecords([
			{id: "r1", rows: [row("p1", "1")]},
			{id: "r2", rows: [row("p2", "2"), row("p1", "3")]},
		])
		expect(moved.digest).not.toBe(digestConfigRecords([a, b]).digest)
		expect(digestConfigRecords([a, b]).digest).not.toBe(digestConfigRecords([a]).digest)
	})
	it("gives a revision that is a safe integer", () => {
		const {revision} = digestConfigRecords([a, b])
		expect(Number.isSafeInteger(revision)).toBe(true)
		expect(revision).toBeGreaterThanOrEqual(0)
	})
})

describe("compareMovement", () => {
	it("labels all four combinations", () => {
		expect(compareMovement(false, false)).toBe("match_unchanged")
		expect(compareMovement(true, true)).toBe("match_changed")
		expect(compareMovement(true, false)).toBe("revision_only")
		expect(compareMovement(false, true)).toBe("digest_only")
	})
})

describe("config", () => {
	it("is off by default, and reads the flag and interval", () => {
		expect(readConfigRevisionShadowConfig({})).toEqual({enabled: false, intervalMs: 30_000})
		expect(readConfigRevisionShadowConfig({GEODB_CONFIG_REVISION_SHADOW: "0"}).enabled).toBe(false)
		expect(
			readConfigRevisionShadowConfig({
				GEODB_CONFIG_REVISION_SHADOW: "on",
				GEODB_CONFIG_REVISION_SHADOW_INTERVAL_MS: "5000",
			}),
		).toEqual({enabled: true, intervalMs: 5000})
		expect(readConfigRevisionShadowConfig({GEODB_CONFIG_REVISION_SHADOW_INTERVAL_MS: "junk"}).intervalMs).toBe(
			30_000,
		)
	})
})

describe("createConfigRevisionReader", () => {
	const store = new Map<string, GeoCardRow[]>()
	const asked: string[] = []
	const readCard = async ({entityId}: {geoSpaceId: string; entityId: string}) => {
		asked.push(entityId)
		return store.get(entityId) ?? []
	}
	const reader = createConfigRevisionReader({geoDb: {readCard}, systemSpaceOf: async () => SPACE})
	const seed = (value: string) => {
		store.clear()
		const ids = [
			forYouConfigRecordId(),
			entityRankingConfigRecordId(),
			interestConfigRecordId(),
			...INTEREST_SIGNAL_KINDS.map(interestSignalWeightRecordId),
		]
		for (const id of ids) store.set(id, [row("p", value)])
	}
	it("reads three singleton records and one per kind, and refuses when a singleton is absent", async () => {
		store.clear()
		asked.length = 0
		await expect(reader.read()).rejects.toBeInstanceOf(ConfigRecordMissing)
		expect(asked).toHaveLength(3 + INTEREST_SIGNAL_KINDS.length)
		expect(new Set(asked).size).toBe(3 + INTEREST_SIGNAL_KINDS.length)
	})
	it("changes with a statement on any record and not otherwise", async () => {
		seed("1")
		const one = await reader.read()
		expect(await reader.read()).toEqual(one)
		seed("2")
		expect((await reader.read()).digest).not.toBe(one.digest)
		seed("1")
		expect(await reader.read()).toEqual(one)
		// a signal kind alone
		const kindId = interestSignalWeightRecordId("vote")
		store.set(kindId, [row("p", "9")])
		expect((await reader.read()).digest).not.toBe(one.digest)
	})
	it("reports a missing config Space", async () => {
		const r = createConfigRevisionReader({geoDb: {readCard}, systemSpaceOf: async () => null})
		await expect(r.read()).rejects.toBeInstanceOf(ConfigSpaceMissing)
	})
})

describe("createConfigRevisionShadow", () => {
	const make = (digests: (string | Error)[], counters: (number | Error)[]) => {
		const warn = vi.fn()
		let i = 0
		let j = 0
		const shadow = createConfigRevisionShadow({
			config: {enabled: true, intervalMs: 1000},
			db: {} as Database,
			reader: {
				read: async () => {
					const d = digests[i++]
					if (d instanceof Error) throw d
					return {digest: d as string, revision: 0}
				},
			},
			readRevision: async () => {
				const c = counters[j++]
				if (c instanceof Error) throw c
				return c as number
			},
			log: {warn},
		})
		return {shadow, warn}
	}
	it("exports nothing until mounted, then every series at zero", () => {
		expect(renderConfigRevisionShadowMetrics()).toBe("")
		make([], [])
		expect(renderConfigRevisionShadowMetrics()).toContain('result="digest_only"} 0')
	})
	it("baselines, then counts whether both moved together", async () => {
		const {shadow, warn} = make(["a", "a", "b", "b", "b", "c"], [1, 1, 2, 2, 3, 3])
		for (let n = 0; n < 6; n++) await shadow.runOnce()
		expect(metric("baseline")).toBe("1")
		expect(metric("match_unchanged")).toBe("2")
		expect(metric("match_changed")).toBe("1")
		expect(metric("revision_only")).toBe("1")
		expect(metric("digest_only")).toBe("1")
		// counts only: the two disagreements are logged without a key or a value
		expect(warn).toHaveBeenCalledTimes(2)
		for (const call of warn.mock.calls) expect(JSON.stringify(call[1])).not.toMatch(/[0-9a-f]{32}/)
	})
	it("starts over after a failed or unsupported pass", async () => {
		const {shadow, warn} = make(
			["a", new GeoDbError("client_failed", "x"), new ConfigSpaceMissing(), "b", "c"],
			[1, 2, 3, 3, 3],
		)
		for (let n = 0; n < 5; n++) await shadow.runOnce()
		expect(metric("baseline")).toBe("2")
		expect(metric("failed")).toBe("1")
		expect(metric("unsupported")).toBe("1")
		expect(metric("match_changed")).toBe("0")
		expect(metric("digest_only")).toBe("1")
		expect(warn).toHaveBeenCalledTimes(1)
	})
	it("counts a Postgres failure and logs it", async () => {
		const {shadow, warn} = make(["a"], [new Error("pg down")])
		await shadow.runOnce()
		expect(metric("reference_failed")).toBe("1")
		expect(metric("failed")).toBe("0")
		expect(warn).toHaveBeenCalledTimes(1)
	})
	it("counts a Postgres statement timeout as reference_timeout", async () => {
		const {shadow} = make(
			["a"],
			[Object.assign(new Error("canceling statement due to statement timeout"), {code: "57014"})],
		)
		await shadow.runOnce()
		expect(metric("reference_timeout")).toBe("1")
		expect(metric("failed")).toBe("0")
	})
	it("does not start a timer when the flag is off", () => {
		const shadow = createConfigRevisionShadow({
			config: {enabled: false, intervalMs: 1000},
			db: {} as Database,
			reader: {read: async () => ({digest: "a", revision: 0})},
			log: {warn: vi.fn()},
		})
		expect(shadow.start()).toBeTypeOf("function")
	})
})
