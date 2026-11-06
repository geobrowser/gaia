import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {gateSpaceTopics} from "../spaceTopicsGate.js"
import {spaceTopicRecordId} from "../spaceTopicsKeyed.js"

const S = "11111111111141118111111111111111"
const T = "22222222222241118222222222222222"
const T2 = "33333333333341118333333333333333"

function setup(geoTopic: string | null, extraGeo = false) {
	const dir = mkdtempSync(join(tmpdir(), "st-gate-"))
	const rec = spaceTopicRecordId(S)
	const space = [`${rec}\t${cid("entity", S)}\t1`]
	const topic = geoTopic === null ? [] : [`${rec}\t${cid("entity", geoTopic)}\t1`]
	if (extraGeo) space.push(`${"a".repeat(32)}\t${cid("entity", S)}\t1`)
	writeFileSync(join(dir, "st-space.tsv"), `${space.join("\n")}\n`)
	writeFileSync(join(dir, "st-topic.tsv"), topic.length ? `${topic.join("\n")}\n` : "")
	writeFileSync(join(dir, "pg.tsv"), `${S}\t${T}\n`)
	return dir
}

describe("space topics gate", () => {
	it("passes when Postgres and the records agree", () => {
		const dir = setup(T)
		const r = gateSpaceTopics(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
		expect(r.pgChecksum).toBe(r.geoChecksum)
	})
	it("reports a different topic and a geo-db-only record", () => {
		const dir = setup(T2, true)
		const r = gateSpaceTopics(join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs.map((d) => d.column)).toEqual(["topic"])
		expect(r.geoOnly).toEqual(["a".repeat(32)])
	})
	it("reports a record whose topic was cleared while Postgres still has one", () => {
		const dir = setup(null)
		const r = gateSpaceTopics(join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs.map((d) => [d.column, d.geo])).toEqual([["topic", "<missing>"]])
	})
	it("reports a Postgres topic with no record", () => {
		const dir = setup(T)
		writeFileSync(join(dir, "pg.tsv"), `${S}\t${T}\n${"b".repeat(32)}\t${T}\n`)
		expect(gateSpaceTopics(join(dir, "pg.tsv"), dir).pgOnly).toHaveLength(1)
	})
})
