import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {loadConfig} from "../config.js"
import {
	applyEntityRankingConfig,
	backfillEntityRankingConfig,
	ENTITY_RANKING_COLUMNS,
	type EntityRankingConfigRow,
	entityRankingConfigFromRow,
	entityRankingConfigIntents,
	entityRankingConfigKeyedEnabled,
	entityRankingConfigRecordId,
} from "../entityRankingConfigKeyed.js"
import {gateEntityRankingConfig, PG_COLUMNS} from "../entityRankingConfigGate.js"
import {intPart, recordEntityId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {fixture, ModelStore} from "./keyedHarness.js"

const COL = RECORD_COLUMNS.entityRankingConfig

/** The live values (0089 retune: tau 100000, weight 4.0) with the migrations' defaults for the rest. */
const PG: Record<string, string> = {
	tau_seconds: "100000",
	wilson_z: "1.96",
	prior_positive: "1",
	prior_negative: "1",
	quality_floor: "0.01",
	intrinsic_cap: "0.5",
	intrinsic_property_target: "10",
	intrinsic_relation_target: "5",
	participation_weight: "4.0",
	participation_cap: "19",
	comment_weight: "0",
	comment_cap: "30",
	topic_interested_weight: "0.5",
	topic_debate_weight: "0.5",
	topic_cap: "2.0",
	watch_window_days: "30",
	watch_prior_impressions: "20",
	watch_reference_seconds: "10",
	watch_ratio_weight: "3.0",
	watch_completed_weight: "1.0",
	watch_plays_weight: "0.25",
	watch_claim_votes_weight: "0.5",
	watch_cap: "8",
	watch_fresh_boost: "5",
	watch_fresh_impressions: "50",
	trending_boost: "8",
	trending_topic_boost: "2.5",
	trending_day_factor: "0.65",
	trending_debate_factor: "0.75",
	trending_debate_window_days: "7",
	trending_horizon_days: "16",
}
const row = (o: Record<string, string> = {}): EntityRankingConfigRow => ({...PG, ...o})

describe("entity_ranking_config record", () => {
	it("is one record in the config family, keyed by the table's single row", () => {
		expect(RECORD_KINDS.entityRankingConfig.family).toBe("config")
		expect(entityRankingConfigRecordId()).toBe(recordEntityId(RECORD_KINDS.entityRankingConfig.id, [intPart(1)]))
	})

	it("mirrors every column once, each with its own pinned id", () => {
		expect(ENTITY_RANKING_COLUMNS).toHaveLength(31)
		expect(new Set(ENTITY_RANKING_COLUMNS.map((c) => c.pg)).size).toBe(31)
		expect(ENTITY_RANKING_COLUMNS.map((c) => c.key).sort()).toEqual(Object.keys(COL).filter((k) => k !== "id").sort())
		expect(new Set(Object.values(COL)).size).toBe(32)
		// The five integer columns of the table (api/drizzle: intrinsic_*_target, watch_window_days, trending_*_days).
		expect(ENTITY_RANKING_COLUMNS.filter((c) => c.int).map((c) => c.pg)).toEqual([
			"intrinsic_property_target",
			"intrinsic_relation_target",
			"watch_window_days",
			"trending_debate_window_days",
			"trending_horizon_days",
		])
	})

	it("sends the same intents for the same values, and different ones for a changed value", () => {
		const base = entityRankingConfigFromRow(row())
		expect(entityRankingConfigIntents(base)).toEqual(entityRankingConfigIntents(entityRankingConfigFromRow(row())))
		expect(entityRankingConfigIntents(entityRankingConfigFromRow(row({tau_seconds: "45000"})))).not.toEqual(
			entityRankingConfigIntents(base),
		)
		expect(entityRankingConfigIntents(entityRankingConfigFromRow(row({watch_window_days: "14"})))).not.toEqual(
			entityRankingConfigIntents(base),
		)
	})

	it("writes decimals as canonical decimals and integers as numbers", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: (d) => `${d}/x`})
		expect(
			await applyEntityRankingConfig(f.deps, entityRankingConfigFromRow(row()), {partition: -1, offset: "backfill"}),
		).toBe("applied")
		const rec = entityRankingConfigRecordId()
		expect(model.cell(rec, COL.id)).toEqual([".{ .number = 1 }"])
		expect(model.cell(rec, COL.tauSeconds)).toEqual(['.{ .decimal = "1,5" }'])
		expect(model.cell(rec, COL.wilsonZ)).toEqual(['.{ .decimal = "196,-2" }'])
		expect(model.cell(rec, COL.participationWeight)).toEqual(['.{ .decimal = "4,0" }'])
		expect(model.cell(rec, COL.watchWindowDays)).toEqual([".{ .number = 30 }"])
		expect(model.cell(rec, COL.trendingHorizonDays)).toEqual([".{ .number = 16 }"])
		for (const {key} of ENTITY_RANKING_COLUMNS) expect(model.cell(rec, COL[key])).toHaveLength(1)
	})

	it("a changed value replaces the old one and a rerun with the old value ends on it (A, B, A)", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: (d) => `${d}/x`})
		const meta = {partition: -1, offset: "backfill"}
		const a = entityRankingConfigFromRow(row())
		await applyEntityRankingConfig(f.deps, a, meta)
		await applyEntityRankingConfig(f.deps, entityRankingConfigFromRow(row({tau_seconds: "45000"})), meta)
		expect(model.cell(entityRankingConfigRecordId(), COL.tauSeconds)).toEqual(['.{ .decimal = "45,3" }'])
		await applyEntityRankingConfig(f.deps, a, meta)
		expect(model.cell(entityRankingConfigRecordId(), COL.tauSeconds)).toEqual(['.{ .decimal = "1,5" }'])
	})
})

describe("backfillEntityRankingConfig", () => {
	it("writes the row", async () => {
		const f = fixture({model: new ModelStore(), offsetsDir: (d) => `${d}/x`})
		expect(await backfillEntityRankingConfig(f.deps, [row()])).toEqual({rows: 1, applied: 1, deadLetter: 0})
	})
	it("dead-letters a row the table could not hold", async () => {
		const f = fixture({offsetsDir: (d) => `${d}/x`})
		const {tau_seconds: _omitted, ...missing} = PG
		const bad = [row({watch_window_days: "1.5"}), row({tau_seconds: "x"}), row({wilson_z: ""}), missing]
		expect(await backfillEntityRankingConfig(f.deps, [...bad, row()])).toEqual({rows: 5, applied: 1, deadLetter: 4})
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
		expect(loadConfig(base).entityRankingConfig).toEqual({enabled: false})
		expect(entityRankingConfigKeyedEnabled(loadConfig(base))).toBe(false)
		expect(entityRankingConfigKeyedEnabled(loadConfig({...base, GEODB_ENTITY_RANKING_CONFIG: "1"}))).toBe(false)
		expect(entityRankingConfigKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1"}))).toBe(false)
		expect(
			entityRankingConfigKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_ENTITY_RANKING_CONFIG: "1"})),
		).toBe(true)
	})
})

describe("entity_ranking_config gate", () => {
	function setup(geoTau: string) {
		const dir = mkdtempSync(join(tmpdir(), "erc-gate-"))
		const rec = entityRankingConfigRecordId()
		const pg: Record<string, string> = {id: "1", ...PG, participation_weight: "4.00"}
		const geo: Record<string, string> = {id: "1", ...PG, tau_seconds: geoTau}
		for (const [n, v] of Object.entries(geo)) writeFileSync(join(dir, `erc-${n}.tsv`), `${rec}\t${v}\t1\n`)
		writeFileSync(join(dir, "pg.tsv"), `${PG_COLUMNS.map((c) => pg[c]).join("\t")}\n`)
		return dir
	}
	it("passes when Postgres and the record agree (numbers compare as numbers)", () => {
		const dir = setup("100000")
		const r = gateEntityRankingConfig(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
	})
	it("reports a changed value", () => {
		const dir = setup("45000")
		expect(gateEntityRankingConfig(join(dir, "pg.tsv"), dir).valueDiffs.map((d) => d.column)).toEqual(["tau_seconds"])
	})
})
