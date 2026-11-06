import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {loadConfig} from "../config.js"
import {gateInterestConfig, gateInterestSignalWeights} from "../interestConfigGate.js"
import {
	applyInterestConfig,
	applyInterestSignalWeight,
	backfillInterest,
	backfillInterestConfig,
	backfillInterestSignalWeights,
	INTEREST_CONFIG_COLUMNS,
	type InterestConfigRow,
	type InterestSignalWeightRow,
	interestConfigFromRow,
	interestConfigIntents,
	interestConfigKeyedEnabled,
	interestConfigRecordId,
	interestSignalWeightFromRow,
	interestSignalWeightIntents,
	interestSignalWeightRecordId,
} from "../interestConfigKeyed.js"
import {intPart, recordEntityId, textPart} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {fixture, ModelStore} from "./keyedHarness.js"

const CFG = RECORD_COLUMNS.interestConfig
const W = RECORD_COLUMNS.interestSignalWeight

/** The migrations' defaults (0100): 30 days, 0.2, 10, 3, 20, '1 hour' as 3600 seconds. */
const PG: Record<string, string> = {
	half_life_days: "30",
	spread_fraction: "0.2",
	spread_max_neighbours: "10",
	cooccurrence_min_support: "3",
	cooccurrence_max_topics_per_entity: "20",
	sweep_lookback_seconds: "3600",
}
const row = (o: Record<string, string> = {}): InterestConfigRow => ({...PG, ...o})

/** The live weights (0100 seeds, 0106/0107 retunes). */
const WEIGHTS: InterestSignalWeightRow[] = [
	{kind: "vote", weight: "1", decays: true, note: "Any vote on an entity."},
	{kind: "follow", weight: "3", decays: false, note: "Follows the topic. \"Fixed\": does not decay."},
	{kind: "not_interested", weight: "-3", decays: true, note: null},
]

describe("interest_config record", () => {
	it("is one record in the config family, keyed by the table's single row", () => {
		expect(RECORD_KINDS.interestConfig.family).toBe("config")
		expect(interestConfigRecordId()).toBe(recordEntityId(RECORD_KINDS.interestConfig.id, [intPart(1)]))
	})

	it("mirrors every column once, each with its own pinned id; the three integers are numbers", () => {
		expect(INTEREST_CONFIG_COLUMNS).toHaveLength(6)
		expect(INTEREST_CONFIG_COLUMNS.map((c) => c.key).sort()).toEqual(Object.keys(CFG).filter((k) => k !== "id").sort())
		expect(new Set(Object.values(CFG)).size).toBe(7)
		expect(INTEREST_CONFIG_COLUMNS.filter((c) => c.int).map((c) => c.pg)).toEqual([
			"spread_max_neighbours",
			"cooccurrence_min_support",
			"cooccurrence_max_topics_per_entity",
		])
	})

	it("sends the same intents for the same values, and different ones for a changed value", () => {
		const base = interestConfigFromRow(row())
		expect(interestConfigIntents(base)).toEqual(interestConfigIntents(interestConfigFromRow(row())))
		expect(interestConfigIntents(interestConfigFromRow(row({half_life_days: "14"})))).not.toEqual(interestConfigIntents(base))
		expect(interestConfigIntents(interestConfigFromRow(row({spread_max_neighbours: "5"})))).not.toEqual(interestConfigIntents(base))
	})

	it("writes decimals as canonical decimals and integers as numbers, A -> B -> A ending on A", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: (d) => `${d}/x`})
		const meta = {partition: -1, offset: "backfill"}
		const a = interestConfigFromRow(row())
		expect(await applyInterestConfig(f.deps, a, meta)).toBe("applied")
		const rec = interestConfigRecordId()
		expect(model.cell(rec, CFG.id)).toEqual([".{ .number = 1 }"])
		expect(model.cell(rec, CFG.halfLifeDays)).toEqual(['.{ .decimal = "3,1" }'])
		expect(model.cell(rec, CFG.spreadFraction)).toEqual(['.{ .decimal = "2,-1" }'])
		expect(model.cell(rec, CFG.spreadMaxNeighbours)).toEqual([".{ .number = 10 }"])
		expect(model.cell(rec, CFG.sweepLookbackSeconds)).toEqual(['.{ .decimal = "36,2" }'])
		for (const {key} of INTEREST_CONFIG_COLUMNS) expect(model.cell(rec, CFG[key])).toHaveLength(1)
		await applyInterestConfig(f.deps, interestConfigFromRow(row({half_life_days: "14"})), meta)
		expect(model.cell(rec, CFG.halfLifeDays)).toEqual(['.{ .decimal = "14,0" }'])
		await applyInterestConfig(f.deps, a, meta)
		expect(model.cell(rec, CFG.halfLifeDays)).toEqual(['.{ .decimal = "3,1" }'])
	})
})

describe("interest_signal_weights records", () => {
	it("is one record per kind, keyed by the kind text", () => {
		expect(RECORD_KINDS.interestSignalWeight.family).toBe("config")
		expect(interestSignalWeightRecordId("vote")).toBe(recordEntityId(RECORD_KINDS.interestSignalWeight.id, [textPart("vote")]))
		expect(interestSignalWeightRecordId("vote")).not.toBe(interestSignalWeightRecordId("follow"))
		expect(new Set(Object.values(W)).size).toBe(4)
	})

	it("writes the key, weight, decays and a note, and no note statement for a null note", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: (d) => `${d}/x`})
		const meta = {partition: -1, offset: "backfill"}
		for (const w of WEIGHTS) expect(await applyInterestSignalWeight(f.deps, interestSignalWeightFromRow(w), meta)).toBe("applied")
		const follow = interestSignalWeightRecordId("follow")
		expect(model.cell(follow, W.kind)).toEqual(['.{ .text = "follow" }'])
		expect(model.cell(follow, W.weight)).toEqual(['.{ .decimal = "3,0" }'])
		expect(model.cell(follow, W.decays)).toEqual([".{ .boolean = false }"])
		expect(model.cell(follow, W.note)).toEqual(['.{ .text = "Follows the topic. \\"Fixed\\": does not decay." }'])
		const ni = interestSignalWeightRecordId("not_interested")
		expect(model.cell(ni, W.weight)).toEqual(['.{ .decimal = "-3,0" }'])
		expect(model.cell(ni, W.note)).toEqual([])
	})

	it("a retuned weight replaces the old one (A, B, A)", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: (d) => `${d}/x`})
		const meta = {partition: -1, offset: "backfill"}
		const vote = (weight: string) => interestSignalWeightFromRow({kind: "vote", weight, decays: true, note: null})
		const rec = interestSignalWeightRecordId("vote")
		await applyInterestSignalWeight(f.deps, vote("1"), meta)
		await applyInterestSignalWeight(f.deps, vote("2"), meta)
		expect(model.cell(rec, W.weight)).toEqual(['.{ .decimal = "2,0" }'])
		await applyInterestSignalWeight(f.deps, vote("1"), meta)
		expect(model.cell(rec, W.weight)).toEqual(['.{ .decimal = "1,0" }'])
	})

	it("sends the same intents for the same row and different ones when decays flips", () => {
		const w = interestSignalWeightFromRow(WEIGHTS[0] as InterestSignalWeightRow)
		expect(interestSignalWeightIntents(w)).toEqual(interestSignalWeightIntents({...w}))
		expect(interestSignalWeightIntents({...w, decays: false})).not.toEqual(interestSignalWeightIntents(w))
	})
})

describe("backfill", () => {
	it("writes the config row and every weight", async () => {
		const f = fixture({model: new ModelStore(), offsetsDir: (d) => `${d}/x`})
		expect(await backfillInterestConfig(f.deps, [row()])).toEqual({rows: 1, applied: 1, deadLetter: 0})
		expect(await backfillInterestSignalWeights(f.deps, WEIGHTS)).toEqual({rows: 3, applied: 3, deadLetter: 0})
	})
	it("backfillInterest sums both tables", async () => {
		const f = fixture({model: new ModelStore(), offsetsDir: (d) => `${d}/x`})
		async function* a() {
			yield row()
		}
		async function* b() {
			yield* WEIGHTS
		}
		expect(await backfillInterest(f.deps, {config: a(), weights: b()})).toEqual({rows: 4, applied: 4, deadLetter: 0})
	})
	it("dead-letters a row the tables could not hold", async () => {
		const f = fixture({offsetsDir: (d) => `${d}/x`})
		const {half_life_days: _omitted, ...missing} = PG
		const bad = [row({spread_max_neighbours: "1.5"}), row({half_life_days: "x"}), row({spread_fraction: ""}), missing]
		expect(await backfillInterestConfig(f.deps, [...bad, row()])).toEqual({rows: 5, applied: 1, deadLetter: 4})
		const badW = [
			{kind: "", weight: "1", decays: true, note: null},
			{kind: "x", weight: "nope", decays: true, note: null},
			{kind: "x", weight: "1", decays: "t" as unknown as boolean, note: null},
		]
		expect(await backfillInterestSignalWeights(f.deps, [...badW, WEIGHTS[0] as InterestSignalWeightRow])).toEqual({
			rows: 4,
			applied: 1,
			deadLetter: 3,
		})
	})
})

describe("flag", () => {
	const base = {
		KAFKA_BROKER: "b",
		KAFKA_USERNAME: "u",
		KAFKA_PASSWORD: "p",
		KAFKA_SSL_CA_PEM: "c",
		GEODB_KEY_PATH: "k",
		GEODB_BINDING_PATH: "b",
	}
	it("is off by default and needs GEODB_RECORD_KEYS", () => {
		expect(loadConfig(base).interestConfig).toEqual({enabled: false})
		expect(interestConfigKeyedEnabled(loadConfig(base))).toBe(false)
		expect(interestConfigKeyedEnabled(loadConfig({...base, GEODB_INTEREST_CONFIG: "1"}))).toBe(false)
		expect(interestConfigKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1"}))).toBe(false)
		expect(interestConfigKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_INTEREST_CONFIG: "1"}))).toBe(true)
	})
})

describe("interest gates", () => {
	function configDir(geoHalfLife: string) {
		const dir = mkdtempSync(join(tmpdir(), "ic-gate-"))
		const rec = interestConfigRecordId()
		const geo: Record<string, string> = {id: "1", ...PG, half_life_days: geoHalfLife}
		for (const [n, v] of Object.entries(geo)) writeFileSync(join(dir, `ic-${n}.tsv`), `${rec}\t${v}\t1\n`)
		// pg prints the epoch as 3600.000000 on some versions; numbers compare as numbers.
		writeFileSync(join(dir, "pg.tsv"), `1\t30\t0.2\t10\t3\t20\t3600.000000\n`)
		return dir
	}
	it("config: passes when Postgres and the record agree", () => {
		const dir = configDir("30")
		const r = gateInterestConfig(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
	})
	it("config: reports a changed value", () => {
		const dir = configDir("14")
		expect(gateInterestConfig(join(dir, "pg.tsv"), dir).valueDiffs.map((d) => d.column)).toEqual(["half_life_days"])
	})

	function weightsDir(geoVote: string, extra = "") {
		const dir = mkdtempSync(join(tmpdir(), "isw-gate-"))
		const rec = (k: string) => interestSignalWeightRecordId(k)
		const kinds = ["vote", "follow"]
		const file = (n: string, f: (k: string) => string | undefined) =>
			writeFileSync(
				join(dir, `isw-${n}.tsv`),
				[...kinds, ...(extra ? [extra] : [])].flatMap((k) => (f(k) === undefined ? [] : [`${rec(k)}\t${f(k)}\t1\n`])).join(""),
			)
		file("kind", (k) => k)
		file("weight", (k) => (k === "vote" ? geoVote : "3.0"))
		file("decays", (k) => String(k === "vote"))
		file("note", (k) => (k === "vote" ? "n" : undefined))
		writeFileSync(join(dir, "pg.tsv"), "vote\t1\tt\tn\nfollow\t3\tf\t\n")
		return dir
	}
	it("weights: passes when they agree (a null note is the empty string, 3 equals 3.0)", () => {
		const dir = weightsDir("1")
		const r = gateInterestSignalWeights(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(2)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
		expect(r.refCountViolations).toEqual([])
	})
	it("weights: reports a changed weight and a removed kind as geo-db-only", () => {
		const dir = weightsDir("2", "old_kind")
		const r = gateInterestSignalWeights(join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs.map((d) => d.column)).toEqual(["weight"])
		expect(r.geoOnly).toEqual([interestSignalWeightRecordId("old_kind")])
	})
})
