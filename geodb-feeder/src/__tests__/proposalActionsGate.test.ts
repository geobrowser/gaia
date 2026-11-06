import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {gateProposalActions, PG_COLUMNS} from "../proposalActionsGate.js"
import {proposalActionRecordId} from "../proposalActionsKeyed.js"

const P = "11111111111141118111111111111111"
const T = "22222222222242228222222222222222"

function setup(geoType: string, extraGeo = false) {
	const dir = mkdtempSync(join(tmpdir(), "pa-gate-"))
	const rec = proposalActionRecordId({proposal: P, block: 7n, index: 0})
	const pgRow: Record<string, string> = {proposal: P, index: "0", block: "7", action_type: "AddMember", target: T}
	const geo: Record<string, string> = {proposal: cid("entity", P), index: "0", block: "7", action_type: geoType, target: cid("entity", T)}
	for (const [n, v] of Object.entries(geo)) {
		const lines = [`${rec}\t${v}\t1`]
		if (extraGeo && n === "proposal") lines.push(`${"a".repeat(32)}\t${v}\t1`)
		if (extraGeo && n !== "proposal") lines.push(`${"a".repeat(32)}\t${v}\t1`)
		writeFileSync(join(dir, `pa-${n}.tsv`), `${lines.join("\n")}\n`)
	}
	for (const n of PG_COLUMNS) if (!(n in geo)) writeFileSync(join(dir, `pa-${n}.tsv`), "")
	writeFileSync(join(dir, "pg.tsv"), `${PG_COLUMNS.map((c) => pgRow[c] ?? "\\N").join("\t")}\n`)
	return dir
}

describe("proposal actions gate", () => {
	it("passes when Postgres and the records agree, nulls matching absent columns", () => {
		const dir = setup("AddMember")
		const r = gateProposalActions(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
		expect(r.pgChecksum).toBe(r.geoChecksum)
	})
	it("reports a changed value and a geo-db-only record", () => {
		const dir = setup("Flag", true)
		const r = gateProposalActions(join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs.map((d) => d.column)).toEqual(["action_type"])
		expect(r.geoOnly).toEqual(["a".repeat(32)])
	})
})
