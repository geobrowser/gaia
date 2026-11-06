import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {loadConfig} from "../config.js"
import {gateFeedExperiments} from "../feedExperimentsGate.js"
import {
	applyFeedExperiment,
	backfillFeedExperiments,
	type FeedExperimentRow,
	feedExperimentFromRow,
	feedExperimentIntents,
	feedExperimentRecordId,
	feedExperimentsKeyedEnabled,
} from "../feedExperimentsKeyed.js"
import {recordEntityId, textPart} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {fixture, ModelStore} from "./keyedHarness.js"

const C = RECORD_COLUMNS.feedExperiment
const row = (o: Partial<FeedExperimentRow> = {}): FeedExperimentRow => ({
	id: "exp-1",
	active: true,
	arm_a: "best",
	arm_b: "for-you",
	share: "0.5",
	salt: "0123456789abcdef",
	note: "first test",
	...o,
})
const meta = {partition: -1, offset: "backfill"}

describe("feed_experiments record", () => {
	it("is one record per experiment id in the config family", () => {
		expect(RECORD_KINDS.feedExperiment.family).toBe("config")
		expect(feedExperimentRecordId("exp-1")).toBe(recordEntityId(RECORD_KINDS.feedExperiment.id, [textPart("exp-1")]))
		expect(feedExperimentRecordId("exp-1")).not.toBe(feedExperimentRecordId("exp-2"))
		expect(new Set(Object.values(C)).size).toBe(7)
	})

	it("writes every column, the share as a canonical decimal, and no note statement for a null note", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: (d) => `${d}/x`})
		expect(await applyFeedExperiment(f.deps, feedExperimentFromRow(row()), meta)).toBe("applied")
		const rec = feedExperimentRecordId("exp-1")
		expect(model.cell(rec, C.id)).toEqual(['.{ .text = "exp-1" }'])
		expect(model.cell(rec, C.active)).toEqual([".{ .boolean = true }"])
		expect(model.cell(rec, C.armA)).toEqual(['.{ .text = "best" }'])
		expect(model.cell(rec, C.armB)).toEqual(['.{ .text = "for-you" }'])
		expect(model.cell(rec, C.share)).toEqual(['.{ .decimal = "5,-1" }'])
		expect(model.cell(rec, C.salt)).toEqual(['.{ .text = "0123456789abcdef" }'])
		expect(model.cell(rec, C.note)).toEqual(['.{ .text = "first test" }'])
		await applyFeedExperiment(f.deps, feedExperimentFromRow(row({id: "exp-2", note: null})), meta)
		expect(model.cell(feedExperimentRecordId("exp-2"), C.note)).toEqual([])
	})

	it("a changed value replaces the old one (A, B, A), for active and for share", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: (d) => `${d}/x`})
		const rec = feedExperimentRecordId("exp-1")
		await applyFeedExperiment(f.deps, feedExperimentFromRow(row()), meta)
		await applyFeedExperiment(f.deps, feedExperimentFromRow(row({active: false, share: "0.25"})), meta)
		expect(model.cell(rec, C.active)).toEqual([".{ .boolean = false }"])
		expect(model.cell(rec, C.share)).toEqual(['.{ .decimal = "25,-2" }'])
		await applyFeedExperiment(f.deps, feedExperimentFromRow(row()), meta)
		expect(model.cell(rec, C.active)).toEqual([".{ .boolean = true }"])
		expect(model.cell(rec, C.share)).toEqual(['.{ .decimal = "5,-1" }'])
	})

	it("sends the same intents for the same values and different ones for a changed value", () => {
		const e = feedExperimentFromRow(row())
		expect(feedExperimentIntents(e)).toEqual(feedExperimentIntents(feedExperimentFromRow(row())))
		expect(feedExperimentIntents({...e, active: false})).not.toEqual(feedExperimentIntents(e))
	})
})

describe("backfill", () => {
	it("writes every row, and dead-letters one the table could not hold", async () => {
		const f = fixture({model: new ModelStore(), offsetsDir: (d) => `${d}/x`})
		const bad = [
			row({id: ""}),
			row({arm_a: "other"}),
			row({share: "1.5"}),
			row({share: ""}),
			row({active: "t" as unknown as boolean}),
		]
		expect(await backfillFeedExperiments(f.deps, [row(), row({id: "exp-2"}), ...bad])).toEqual({
			rows: 7,
			applied: 2,
			deadLetter: 5,
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
		expect(loadConfig(base).feedExperiments).toEqual({enabled: false})
		expect(feedExperimentsKeyedEnabled(loadConfig({...base, GEODB_FEED_EXPERIMENTS: "1"}))).toBe(false)
		expect(feedExperimentsKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1"}))).toBe(false)
		expect(feedExperimentsKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_FEED_EXPERIMENTS: "1"}))).toBe(true)
	})
})

describe("feed experiments gate", () => {
	function dir(geoShare: string, extra = false) {
		const d = mkdtempSync(join(tmpdir(), "fe-gate-"))
		const ids = ["exp-1", ...(extra ? ["old"] : [])]
		const file = (n: string, f: (id: string) => string | undefined) =>
			writeFileSync(
				join(d, `fe-${n}.tsv`),
				ids.flatMap((id) => (f(id) === undefined ? [] : [`${feedExperimentRecordId(id)}\t${f(id)}\t1\n`])).join(""),
			)
		file("id", (id) => id)
		file("active", () => "true")
		file("arm_a", () => "best")
		file("arm_b", () => "for-you")
		file("share", () => geoShare)
		file("salt", () => "0123456789abcdef")
		file("note", (id) => (id === "exp-1" ? "first test" : undefined))
		writeFileSync(join(d, "pg.tsv"), "exp-1\tt\tbest\tfor-you\t0.5\t0123456789abcdef\tfirst test\n")
		return d
	}
	it("passes when Postgres and the record agree (0.5 equals 0.50)", () => {
		const d = dir("0.50")
		const r = gateFeedExperiments(join(d, "pg.tsv"), d)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
	})
	it("reports a changed share, and a deleted experiment as geo-db-only", () => {
		const d = dir("0.25", true)
		const r = gateFeedExperiments(join(d, "pg.tsv"), d)
		expect(r.valueDiffs.map((x) => x.column)).toEqual(["share"])
		expect(r.geoOnly).toEqual([feedExperimentRecordId("old")])
	})
})
