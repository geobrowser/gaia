import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {gateSpaceVotingSettings, PG_COLUMNS} from "../spaceVotingSettingsGate.js"
import {spaceVotingSettingsRecordId} from "../spaceVotingSettingsKeyed.js"

const S = "11111111111141118111111111111111"

function setup(geoQuorum: string, extraGeo = false) {
	const dir = mkdtempSync(join(tmpdir(), "svs-gate-"))
	const rec = spaceVotingSettingsRecordId(S)
	const pgRow: Record<string, string> = {
		space: S, partial: "500000", universal: "750000", flat: "3", quorum: "2", duration: "86400",
		disable_fast_path: "f", grace: "3600", updated_at: "1700000000", updated_at_block: "123",
	}
	const geo: Record<string, string> = {
		space: cid("entity", S), partial: "500000", universal: "750000", flat: "3", quorum: geoQuorum, duration: "86400",
		disable_fast_path: "false", grace: "3600", updated_at: "instant:1700000000.000000", updated_at_block: "123",
	}
	for (const [n, v] of Object.entries(geo)) {
		const lines = [`${rec}\t${v}\t1`]
		if (extraGeo) lines.push(`${"a".repeat(32)}\t${v}\t1`)
		writeFileSync(join(dir, `svs-${n}.tsv`), `${lines.join("\n")}\n`)
	}
	writeFileSync(join(dir, "pg.tsv"), `${PG_COLUMNS.map((c) => pgRow[c]).join("\t")}\n`)
	return dir
}

describe("space voting settings gate", () => {
	it("passes when Postgres and the records agree", () => {
		const dir = setup("2")
		const r = gateSpaceVotingSettings(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
		expect(r.pgChecksum).toBe(r.geoChecksum)
	})
	it("reports a changed value and a geo-db-only record", () => {
		const dir = setup("9", true)
		const r = gateSpaceVotingSettings(join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs.map((d) => d.column)).toEqual(["quorum"])
		expect(r.geoOnly).toEqual(["a".repeat(32)])
	})
})
