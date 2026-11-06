import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {loadConfig} from "../config.js"
import {gateForYouConfig, PG_COLUMNS} from "../forYouConfigGate.js"
import {
	applyForYouConfig,
	backfillForYouConfig,
	type ForYouConfig,
	type ForYouConfigRow,
	forYouConfigFromRow,
	forYouConfigIntents,
	forYouConfigKeyedEnabled,
	forYouConfigRecordId,
} from "../forYouConfigKeyed.js"
import {intPart, recordEntityId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {fixture, ModelStore} from "./keyedHarness.js"

const COL = RECORD_COLUMNS.forYouConfig
const cfg = (o: Partial<ForYouConfig> = {}): ForYouConfig => ({
	maxBoostDays: 6,
	interestHalfSaturation: 2,
	explorationShare: 0.1,
	maxTopics: 50n,
	...o,
})
const row = (o: Partial<ForYouConfigRow> = {}): ForYouConfigRow => ({
	max_boost_days: "6",
	interest_half_saturation: "2",
	exploration_share: "0.1",
	max_topics: "50",
	...o,
})

describe("for_you_config record", () => {
	it("is one record in the config family, keyed by the table's single row", () => {
		expect(RECORD_KINDS.forYouConfig.family).toBe("config")
		expect(forYouConfigRecordId()).toBe(recordEntityId(RECORD_KINDS.forYouConfig.id, [intPart(1)]))
	})

	it("sends the same intents for the same values, and different ones for a changed value", () => {
		expect(forYouConfigIntents(cfg())).toEqual(forYouConfigIntents(cfg()))
		expect(forYouConfigIntents(cfg({explorationShare: 0.2}))).not.toEqual(forYouConfigIntents(cfg()))
	})

	it("a row and the values it holds send the same intents", () => {
		expect(forYouConfigIntents(forYouConfigFromRow(row()))).toEqual(forYouConfigIntents(cfg()))
	})

	it("writes the decimals as canonical decimals and max_topics as a number", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: (d) => `${d}/x`})
		expect(await applyForYouConfig(f.deps, cfg(), {partition: -1, offset: "backfill"})).toBe("applied")
		const rec = forYouConfigRecordId()
		expect(model.cell(rec, COL.id)).toEqual([".{ .number = 1 }"])
		expect(model.cell(rec, COL.explorationShare)).toEqual(['.{ .decimal = "1,-1" }'])
		expect(model.cell(rec, COL.maxBoostDays)).toEqual(['.{ .decimal = "6,0" }'])
		expect(model.cell(rec, COL.maxTopics)).toEqual([".{ .number = 50 }"])
	})

	it("a changed value replaces the old one and a rerun with the old value ends on it (A, B, A)", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: (d) => `${d}/x`})
		const meta = {partition: -1, offset: "backfill"}
		await applyForYouConfig(f.deps, cfg(), meta)
		await applyForYouConfig(f.deps, cfg({maxTopics: 80n}), meta)
		expect(model.cell(forYouConfigRecordId(), COL.maxTopics)).toEqual([".{ .number = 80 }"])
		await applyForYouConfig(f.deps, cfg(), meta)
		expect(model.cell(forYouConfigRecordId(), COL.maxTopics)).toEqual([".{ .number = 50 }"])
	})
})

describe("backfillForYouConfig", () => {
	it("writes the row", async () => {
		const f = fixture({model: new ModelStore(), offsetsDir: (d) => `${d}/x`})
		expect(await backfillForYouConfig(f.deps, [row()])).toEqual({rows: 1, applied: 1, deadLetter: 0})
	})
	it("dead-letters a row the table could not hold", async () => {
		const f = fixture({offsetsDir: (d) => `${d}/x`})
		const bad = [row({max_topics: "1.5"}), row({max_boost_days: "x"}), row({exploration_share: ""})]
		expect(await backfillForYouConfig(f.deps, [...bad, row()])).toEqual({rows: 4, applied: 1, deadLetter: 3})
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
		expect(loadConfig(base).forYouConfig).toEqual({enabled: false})
		expect(forYouConfigKeyedEnabled(loadConfig(base))).toBe(false)
		expect(forYouConfigKeyedEnabled(loadConfig({...base, GEODB_FOR_YOU_CONFIG: "1"}))).toBe(false)
		expect(forYouConfigKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1"}))).toBe(false)
		expect(forYouConfigKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_FOR_YOU_CONFIG: "1"}))).toBe(
			true,
		)
	})
})

describe("for_you_config gate", () => {
	function setup(geoTopics: string) {
		const dir = mkdtempSync(join(tmpdir(), "fyc-gate-"))
		const rec = forYouConfigRecordId()
		const pg: Record<string, string> = {
			id: "1",
			max_boost_days: "6",
			interest_half_saturation: "2",
			exploration_share: "0.10",
			max_topics: "50",
		}
		const geo: Record<string, string> = {
			id: "1",
			max_boost_days: "6",
			interest_half_saturation: "2",
			exploration_share: "0.1",
			max_topics: geoTopics,
		}
		for (const [n, v] of Object.entries(geo)) writeFileSync(join(dir, `fyc-${n}.tsv`), `${rec}\t${v}\t1\n`)
		writeFileSync(join(dir, "pg.tsv"), `${PG_COLUMNS.map((c) => pg[c]).join("\t")}\n`)
		return dir
	}
	it("passes when Postgres and the record agree (numbers compare as numbers)", () => {
		const dir = setup("50")
		const r = gateForYouConfig(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
	})
	it("reports a changed value", () => {
		const dir = setup("80")
		expect(gateForYouConfig(join(dir, "pg.tsv"), dir).valueDiffs.map((d) => d.column)).toEqual(["max_topics"])
	})
})
