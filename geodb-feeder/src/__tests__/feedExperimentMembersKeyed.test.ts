import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {loadConfig} from "../config.js"
import {gateFeedExperimentMembers} from "../feedExperimentMembersGate.js"
import {
	applyFeedExperimentMember,
	backfillFeedExperimentMembers,
	type FeedExperimentMemberRow,
	feedExperimentMemberFromRow,
	feedExperimentMemberIntents,
	feedExperimentMemberRecordId,
	feedExperimentMembersKeyedEnabled,
} from "../feedExperimentMembersKeyed.js"
import {idPart, recordEntityId, textPart} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {fixture, ModelStore} from "./keyedHarness.js"

const C = RECORD_COLUMNS.feedExperimentMember
const U1 = "550e8400-e29b-41d4-a716-446655440000"
const U2 = "550e8400-e29b-41d4-a716-446655440001"
const row = (o: Partial<FeedExperimentMemberRow> = {}): FeedExperimentMemberRow => ({experiment_id: "exp-1", user_id: U1, interleaved: true, ...o})
const meta = {partition: -1, offset: "backfill"}

describe("feed_experiment_members record", () => {
	it("is one record per (experiment, user) in the config family; dashed or not, the same record", () => {
		expect(RECORD_KINDS.feedExperimentMember.family).toBe("config")
		expect(feedExperimentMemberRecordId("exp-1", U1)).toBe(
			recordEntityId(RECORD_KINDS.feedExperimentMember.id, [textPart("exp-1"), idPart(U1)]),
		)
		expect(feedExperimentMemberRecordId("exp-1", U1.replaceAll("-", ""))).toBe(feedExperimentMemberRecordId("exp-1", U1))
		expect(feedExperimentMemberRecordId("exp-1", U1)).not.toBe(feedExperimentMemberRecordId("exp-1", U2))
		expect(feedExperimentMemberRecordId("exp-1", U1)).not.toBe(feedExperimentMemberRecordId("exp-2", U1))
		expect(new Set(Object.values(C)).size).toBe(3)
	})

	it("writes both keys and the interleaved flag, A -> B -> A ending on A", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: (d) => `${d}/x`})
		expect(await applyFeedExperimentMember(f.deps, feedExperimentMemberFromRow(row()), meta)).toBe("applied")
		const rec = feedExperimentMemberRecordId("exp-1", U1)
		expect(model.cell(rec, C.experiment)).toEqual(['.{ .text = "exp-1" }'])
		expect(model.cell(rec, C.user)).toEqual([`.{ .text = "${U1}" }`])
		expect(model.cell(rec, C.interleaved)).toEqual([".{ .boolean = true }"])
		await applyFeedExperimentMember(f.deps, feedExperimentMemberFromRow(row({interleaved: false})), meta)
		expect(model.cell(rec, C.interleaved)).toEqual([".{ .boolean = false }"])
		await applyFeedExperimentMember(f.deps, feedExperimentMemberFromRow(row()), meta)
		expect(model.cell(rec, C.interleaved)).toEqual([".{ .boolean = true }"])
	})

	it("sends the same intents for the same row and different ones when interleaved flips", () => {
		const m = feedExperimentMemberFromRow(row())
		expect(feedExperimentMemberIntents(m)).toEqual(feedExperimentMemberIntents(feedExperimentMemberFromRow(row())))
		expect(feedExperimentMemberIntents({...m, interleaved: false})).not.toEqual(feedExperimentMemberIntents(m))
	})
})

describe("backfill", () => {
	it("writes every row, and dead-letters one the table could not hold", async () => {
		const f = fixture({model: new ModelStore(), offsetsDir: (d) => `${d}/x`})
		const bad = [row({experiment_id: ""}), row({user_id: "nope"}), row({interleaved: "t" as unknown as boolean})]
		expect(await backfillFeedExperimentMembers(f.deps, [row(), row({user_id: U2}), ...bad])).toEqual({rows: 5, applied: 2, deadLetter: 3})
	})
})

describe("flag", () => {
	const base = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}
	it("is off by default and needs GEODB_RECORD_KEYS", () => {
		expect(loadConfig(base).feedExperimentMembers).toEqual({enabled: false})
		expect(feedExperimentMembersKeyedEnabled(loadConfig({...base, GEODB_FEED_EXPERIMENT_MEMBERS: "1"}))).toBe(false)
		expect(feedExperimentMembersKeyedEnabled(loadConfig({...base, GEODB_FEED_EXPERIMENTS: "1", GEODB_RECORD_KEYS: "1"}))).toBe(false)
		expect(feedExperimentMembersKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_FEED_EXPERIMENT_MEMBERS: "1"}))).toBe(true)
	})
})

describe("members gate", () => {
	function dir(geoInterleaved: string, extra = false) {
		const d = mkdtempSync(join(tmpdir(), "fem-gate-"))
		const members = [[ "exp-1", U1], ...(extra ? [["exp-1", U2]] : [])] as [string, string][]
		const file = (n: string, f: (e: string, u: string) => string) =>
			writeFileSync(join(d, `fem-${n}.tsv`), members.map(([e, u]) => `${feedExperimentMemberRecordId(e, u)}\t${f(e, u)}\t1\n`).join(""))
		file("experiment_id", (e) => e)
		file("user_id", (_e, u) => u)
		file("interleaved", () => geoInterleaved)
		writeFileSync(join(d, "pg.tsv"), `exp-1\t${U1}\tt\n`)
		return d
	}
	it("passes when Postgres and the records agree", () => {
		const d = dir("true")
		const r = gateFeedExperimentMembers(join(d, "pg.tsv"), d)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
	})
	it("reports a flipped flag, and a removed membership as geo-db-only", () => {
		const d = dir("false", true)
		const r = gateFeedExperimentMembers(join(d, "pg.tsv"), d)
		expect(r.valueDiffs.map((x) => x.column)).toEqual(["interleaved"])
		expect(r.geoOnly).toEqual([feedExperimentMemberRecordId("exp-1", U2)])
	})
})
