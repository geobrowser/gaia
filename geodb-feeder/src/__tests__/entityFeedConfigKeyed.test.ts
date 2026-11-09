import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {loadConfig} from "../config.js"
import {gateEntityTypeWeights} from "../entityFeedConfigGate.js"
import {
	applyEntityFeedBlock,
	applyEntityTypeExclusion,
	applyEntityTypeWeight,
	backfillEntityFeedConfig,
	entityFeedBlockFromRow,
	entityFeedBlockRecordId,
	entityFeedConfigKeyedEnabled,
	entityTypeExclusionFromRow,
	entityTypeExclusionRecordId,
	entityTypeWeightFromRow,
	entityTypeWeightRecordId,
	entityTypeWeightIntents,
} from "../entityFeedConfigKeyed.js"
import {makePacer} from "../keyedBackfillJob.js"
import {countEntityFeedConfigRows, type ReadOnlyPg} from "../pgSource.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {fixture, ModelStore} from "./keyedHarness.js"

const T1 = "550e8400-e29b-41d4-a716-446655440000"
const T2 = "550e8400-e29b-41d4-a716-446655440001"
const WC = RECORD_COLUMNS.entityTypeWeight
const f = () => fixture({model: new ModelStore(), offsetsDir: (d) => `${d}/x`})

describe("entity_type_weights record", () => {
	it("is one config record per type, dashed or not the same record, with distinct kinds per table", () => {
		expect(RECORD_KINDS.entityTypeWeight.family).toBe("config")
		expect(entityTypeWeightRecordId(T1)).toBe(entityTypeWeightRecordId(T1.replaceAll("-", "")))
		expect(entityTypeWeightRecordId(T1)).not.toBe(entityTypeWeightRecordId(T2))
		expect(entityTypeWeightRecordId(T1)).not.toBe(entityTypeExclusionRecordId(T1))
		expect(entityTypeExclusionRecordId(T1)).not.toBe(entityFeedBlockRecordId(T1))
	})
	it("writes key and weight; a change closes the old value and A -> B -> A ends on A", async () => {
		const model = new ModelStore()
		const d = fixture({model, offsetsDir: (x) => `${x}/x`})
		const w = (weight: string, note: string | null = null) => entityTypeWeightFromRow({type_id: T1, weight, note})
		expect(await applyEntityTypeWeight(d.deps, w("2.5", "news"))).toBe("applied")
		const rec = entityTypeWeightRecordId(T1)
		expect(model.cell(rec, WC.typeId)).toEqual([`.{ .text = "${T1}" }`])
		expect(model.cell(rec, WC.weight)).toEqual(['.{ .decimal = "25,-1" }'])
		expect(model.cell(rec, WC.note)).toEqual(['.{ .text = "news" }'])
		await applyEntityTypeWeight(d.deps, w("0.5"))
		expect(model.cell(rec, WC.weight)).toEqual(['.{ .decimal = "5,-1" }'])
		await applyEntityTypeWeight(d.deps, w("2.5", "news"))
		expect(model.cell(rec, WC.weight)).toEqual(['.{ .decimal = "25,-1" }'])
	})
	it("sends the same intents for the same row", () => {
		const a = entityTypeWeightFromRow({type_id: T1, weight: "1.0", note: null})
		expect(entityTypeWeightIntents(a)).toEqual(entityTypeWeightIntents({...a}))
		expect(entityTypeWeightIntents({...a, weight: 2})).not.toEqual(entityTypeWeightIntents(a))
	})
	it("refuses what the table's range check would have", () => {
		for (const weight of ["0.1", "10.5", "0", "", "x"]) expect(() => entityTypeWeightFromRow({type_id: T1, weight, note: null})).toThrow()
		expect(() => entityTypeWeightFromRow({type_id: "nope", weight: "1", note: null})).toThrow()
		expect(entityTypeWeightFromRow({type_id: T1, weight: "10", note: null}).weight).toBe(10)
	})
})

describe("presence records", () => {
	it("write the key and the optional text", async () => {
		const model = new ModelStore()
		const d = fixture({model, offsetsDir: (x) => `${x}/x`})
		await applyEntityTypeExclusion(d.deps, entityTypeExclusionFromRow({type_id: T1, note: null}))
		const xr = entityTypeExclusionRecordId(T1)
		expect(model.cell(xr, RECORD_COLUMNS.entityTypeExclusion.typeId)).toEqual([`.{ .text = "${T1}" }`])
		expect(model.cell(xr, RECORD_COLUMNS.entityTypeExclusion.note)).toEqual([])
		await applyEntityFeedBlock(d.deps, entityFeedBlockFromRow({entity_id: T2, reason: "spam"}))
		const br = entityFeedBlockRecordId(T2)
		expect(model.cell(br, RECORD_COLUMNS.entityFeedBlock.reason)).toEqual(['.{ .text = "spam" }'])
	})
})

describe("backfill", () => {
	it("writes every row of the three tables and dead-letters one the table could not hold", async () => {
		const d = f()
		const r = await backfillEntityFeedConfig(d.deps, {
			weights: [{type_id: T1, weight: "2", note: null}, {type_id: T2, weight: "99", note: null}],
			exclusions: [{type_id: T1, note: null}],
			blocks: [{entity_id: T1, reason: null}, {entity_id: "nope", reason: null}],
		})
		expect(r).toEqual({rows: 5, applied: 3, deadLetter: 2})
	})
})

describe("backfill pacing and dry run", () => {
	it("awaits the pace hook after each row it writes (a row that fails to decode writes nothing)", async () => {
		const d = f()
		let calls = 0
		d.deps.pace = async () => {
			calls++
		}
		await backfillEntityFeedConfig(d.deps, {
			weights: [{type_id: T1, weight: "2", note: null}],
			exclusions: [{type_id: T1, note: null}],
			blocks: [{entity_id: "nope", reason: null}],
		})
		expect(calls).toBe(2)
	})
	it("a pacer holds the rate (20 rows/s is 50 ms a row) and does not bank time after a stall", async () => {
		let t = 0
		const waits: number[] = []
		const pace = makePacer(20, () => t, async (ms) => {
			waits.push(ms)
			t += ms
		})
		if (!pace) throw new Error("no pacer")
		await pace()
		await pace()
		expect(waits).toEqual([50, 50])
		t += 1000 // a slow row
		await pace()
		await pace()
		expect(waits).toEqual([50, 50, 50])
		expect(makePacer(0)).toBeUndefined()
		expect(makePacer(Number.NaN)).toBeUndefined()
	})
	it("counts the three tables with plain reads, nothing else", async () => {
		const seen: string[] = []
		const db = {
			read: async (text: string) => {
				seen.push(text)
				return [{n: String(seen.length * 10)}]
			},
		} as unknown as ReadOnlyPg
		expect(await countEntityFeedConfigRows(db)).toEqual({entity_type_weights: 10, entity_type_exclusions: 20, entity_feed_blocklist: 30})
		expect(seen.every((q) => /^SELECT count\(\*\)::text AS n FROM entity_/.test(q))).toBe(true)
	})
})

describe("flag", () => {
	const base = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}
	it("is off by default and needs GEODB_RECORD_KEYS", () => {
		expect(loadConfig(base).entityFeedConfig).toEqual({enabled: false})
		expect(entityFeedConfigKeyedEnabled(loadConfig({...base, GEODB_ENTITY_FEED_CONFIG: "1"}))).toBe(false)
		expect(entityFeedConfigKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1"}))).toBe(false)
		expect(entityFeedConfigKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_ENTITY_FEED_CONFIG: "1"}))).toBe(true)
	})
})

describe("weights gate", () => {
	function dir(geoWeight: string, extra = false) {
		const d = mkdtempSync(join(tmpdir(), "etw-gate-"))
		const ids = extra ? [T1, T2] : [T1]
		const file = (n: string, v: (t: string) => string | null) =>
			writeFileSync(join(d, `etw-${n}.tsv`), ids.flatMap((t) => (v(t) === null ? [] : [`${entityTypeWeightRecordId(t)}\t${v(t)}\t1\n`])).join(""))
		file("type_id", (t) => t)
		file("weight", () => geoWeight)
		file("note", () => null)
		writeFileSync(join(d, "pg.tsv"), `${T1}\t2.50\t\\N\n`)
		return d
	}
	it("passes when Postgres and the records agree, numeric spelling aside, null note as absent", () => {
		const d = dir("2.5")
		const r = gateEntityTypeWeights(join(d, "pg.tsv"), d)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
	})
	it("reports a changed weight, and a removed row as geo-db-only", () => {
		const d = dir("3", true)
		const r = gateEntityTypeWeights(join(d, "pg.tsv"), d)
		expect(r.valueDiffs.map((x) => x.column)).toEqual(["weight"])
		expect(r.geoOnly).toEqual([entityTypeWeightRecordId(T2)])
	})
})
