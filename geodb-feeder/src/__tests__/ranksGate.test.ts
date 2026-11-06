import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {gateRanks, PG_COLUMNS} from "../ranksGate.js"
import {rankingItemRecordId} from "../ranksKeyed.js"

const R = "11111111111141118111111111111111"
const E = "22222222222242228222222222222222"
const S = "33333333333343338333333333333333"

function setup(geoWeight: string, extraGeo = false) {
	const dir = mkdtempSync(join(tmpdir(), "ranks-gate-"))
	const rec = rankingItemRecordId(R, E, S)
	const pgRow: Record<string, string> = {ranking: R, entity: E, space: S, position: "a0", weight: "1.50"}
	const geo: Record<string, string> = {ranking: cid("entity", R), entity: cid("entity", E), space: cid("entity", S), position: "a0", weight: geoWeight}
	for (const [n, v] of Object.entries(geo)) {
		const lines = [`${rec}\t${v}\t1`]
		if (extraGeo) lines.push(`${"a".repeat(32)}\t${v}\t1`)
		writeFileSync(join(dir, `ri-${n}.tsv`), `${lines.join("\n")}\n`)
	}
	writeFileSync(join(dir, "pg.tsv"), `${PG_COLUMNS.items.map((c) => pgRow[c]).join("\t")}\n`)
	return dir
}

describe("ranks gate", () => {
	it("passes when Postgres and the records agree (a decimal compares by value)", () => {
		const dir = setup("1.5")
		const r = gateRanks("items", join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
	})
	it("reports a changed value and a geo-db-only record", () => {
		const dir = setup("2", true)
		const r = gateRanks("items", join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs.map((d) => d.column)).toEqual(["weight"])
		expect(r.geoOnly).toEqual(["a".repeat(32)])
	})
})
