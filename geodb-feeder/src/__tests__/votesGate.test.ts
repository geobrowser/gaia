import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {proposalVoteRecordId} from "../proposalVotesKeyed.js"
import {gateProposalVotes} from "../votesGate.js"

const P = "11111111111141118111111111111111"
const V = "22222222222242228222222222222222"
const S = "33333333333343338333333333333333"

function setup(vote: string, geoVote: string, extraGeo = false) {
	const dir = mkdtempSync(join(tmpdir(), "votes-gate-"))
	const rec = proposalVoteRecordId({proposal: P, version: 1, voter: V})
	const rows: Record<string, string> = {
		proposal: cid("entity", P),
		version: "1",
		voter: cid("entity", V),
		space: cid("entity", S),
		vote: geoVote,
		created_at: "instant:1700000000.000000000",
		block: "7",
	}
	for (const [n, v] of Object.entries(rows)) {
		const lines = [`${rec}\t${v}\t1`]
		if (extraGeo) lines.push(`${"a".repeat(32)}\t${v}\t1`)
		writeFileSync(join(dir, `pv-${n}.tsv`), `${lines.join("\n")}\n`)
	}
	writeFileSync(join(dir, "pg.tsv"), `${P}\t1\t${V}\t${S}\t${vote}\t1700000000\t7\n`)
	return dir
}

describe("votes gate", () => {
	it("passes when Postgres and the records agree", () => {
		const dir = setup("Yes", "1")
		const r = gateProposalVotes(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
		expect(r.pgChecksum).toBe(r.geoChecksum)
	})
	it("reports a changed vote and a geo-db-only record", () => {
		const dir = setup("Yes", "2", true)
		const r = gateProposalVotes(join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs.map((d) => d.column)).toEqual(["vote"])
		expect(r.geoOnly).toEqual(["a".repeat(32)])
	})
})
