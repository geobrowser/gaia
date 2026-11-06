import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {proposalEscalationRecordId} from "../proposalEscalationKeyed.js"
import {gateProposalVersions, PG_COLUMNS} from "../proposalVersionsGate.js"
import {proposalVersionRecordId} from "../proposalVersionsKeyed.js"

const P = "11111111111141118111111111111111"

type Row = {block: string; version: string; mode: string; quorum?: string}

/** Postgres rows and the matching geo-db dumps, where `geoMode` overrides the mode the records hold. */
function setup(rows: Row[], geoMode: Record<string, string> = {}, extraGeo = false) {
	const dir = mkdtempSync(join(tmpdir(), "pv-gate-"))
	const pgLines: string[] = []
	const cols: Record<string, string[]> = {}
	const put = (n: string, rec: string, v: string) => (cols[n] ??= []).push(`${rec}\t${v}\t1`)
	for (const r of rows) {
		const rec = proposalVersionRecordId({proposal: P, block: BigInt(r.block)})
		const pg: Record<string, string> = {
			proposal: P, version: r.version, block: r.block, voting_mode: r.mode, start_time: "10", end_time: "20", quorum: r.quorum ?? "5",
			partial: "1", universal: "2", flat: "3", execute_by: "\\N", name: "Flag", created_at: "99",
		}
		pgLines.push(PG_COLUMNS.map((c) => pg[c]).join("\t"))
		const geo: Record<string, string> = {...pg, proposal: cid("entity", P), voting_mode: geoMode[r.block] ?? r.mode}
		for (const [n, v] of Object.entries(geo)) if (n !== "version" && !(n === "execute_by")) put(n, rec, v)
	}
	if (extraGeo) for (const n of ["proposal", "block"]) put(n, "a".repeat(32), "1")
	for (const n of ["proposal", "block", ...PG_COLUMNS.slice(3)]) writeFileSync(join(dir, `pv-${n}.tsv`), cols[n] ? `${cols[n].join("\n")}\n` : "")
	writeFileSync(join(dir, "pg.tsv"), `${pgLines.join("\n")}\n`)
	return dir
}

describe("proposal versions gate", () => {
	it("passes when Postgres and the records agree, null execute_by matching an absent column", () => {
		const dir = setup([{block: "7", version: "1", mode: "Slow"}, {block: "9", version: "2", mode: "Slow"}])
		const r = gateProposalVersions(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(2)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
		expect(r.ordinalViolations).toEqual([])
		expect(r.orphanEscalations).toEqual([])
		expect(r.pgChecksum).toBe(r.geoChecksum)
	})
	it("reports a pg-only version, a changed value and a geo-db-only record", () => {
		const dir2 = setup([{block: "7", version: "1", mode: "Slow"}], {}, true)
		expect(gateProposalVersions(join(dir2, "pg.tsv"), dir2).geoOnly).toEqual(["a".repeat(32)])
		const dir3 = setup([{block: "7", version: "1", mode: "Slow"}])
		writeFileSync(join(dir3, "pv-quorum.tsv"), `${proposalVersionRecordId({proposal: P, block: 7n})}\t6\t1\n`)
		expect(gateProposalVersions(join(dir3, "pg.tsv"), dir3).valueDiffs.map((d) => d.column)).toEqual(["quorum"])
	})
	/** pe-*.tsv dumps for escalation records; `settings` is what the event set (absent execute_by = none). */
	const escalate = (dir: string, escs: {block: string; mode: string; quorum?: string}[]) => {
		const cols: Record<string, string[]> = {}
		const put = (n: string, rec: string, v: string) => (cols[n] ??= []).push(`${rec}\t${v}\t1`)
		for (const e of escs) {
			const rec = proposalEscalationRecordId({proposal: P, block: BigInt(e.block)})
			const vals: Record<string, string> = {proposal: cid("entity", P), block: e.block, voting_mode: e.mode, start_time: "10", end_time: "20", quorum: e.quorum ?? "5", partial: "1", universal: "2", flat: "3"}
			for (const [n, v] of Object.entries(vals)) put(n, rec, v)
		}
		for (const n of ["proposal", "block", "voting_mode", "start_time", "end_time", "quorum", "partial", "universal", "flat", "execute_by"])
			writeFileSync(join(dir, `pe-${n}.tsv`), cols[n] ? `${cols[n].join("\n")}\n` : "")
	}
	it("overlays an escalation onto the version current at its block, so Fast in geo-db and Slow in Postgres passes", () => {
		const dir = setup([{block: "7", version: "1", mode: "Slow"}], {"7": "Fast"})
		escalate(dir, [{block: "8", mode: "Slow"}])
		const r = gateProposalVersions(join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs).toEqual([])
		expect(r.orphanEscalations).toEqual([])
		expect(r.pgChecksum).toBe(r.geoChecksum)
	})
	it("without the escalation record the same difference fails", () => {
		const dir = setup([{block: "7", version: "1", mode: "Slow"}], {"7": "Fast"})
		expect(gateProposalVersions(join(dir, "pg.tsv"), dir).valueDiffs.map((d) => d.column)).toEqual(["voting_mode"])
	})
	it("hits only the latest version at or before the escalation's block", () => {
		const dir = setup([{block: "7", version: "1", mode: "Fast"}, {block: "9", version: "2", mode: "Slow", quorum: "6"}], {"9": "Fast"})
		escalate(dir, [{block: "10", mode: "Slow", quorum: "6"}])
		const r = gateProposalVersions(join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs).toEqual([])
	})
	it("reports an escalation before every version as an orphan, not a failure", () => {
		const dir = setup([{block: "7", version: "1", mode: "Slow"}])
		escalate(dir, [{block: "3", mode: "Fast"}])
		const r = gateProposalVersions(join(dir, "pg.tsv"), dir)
		expect(r.orphanEscalations).toEqual([proposalEscalationRecordId({proposal: P, block: 3n})])
		expect(r.valueDiffs).toEqual([])
	})
	it("still fails a non-settings difference of an escalated record", () => {
		const dir = setup([{block: "7", version: "1", mode: "Slow"}], {"7": "Fast"})
		escalate(dir, [{block: "8", mode: "Slow"}])
		writeFileSync(join(dir, "pv-name.tsv"), `${proposalVersionRecordId({proposal: P, block: 7n})}\tOther\t1\n`)
		expect(gateProposalVersions(join(dir, "pg.tsv"), dir).valueDiffs.map((d) => d.column)).toEqual(["name"])
	})
	it("flags a Postgres ordinal that is not the block's rank", () => {
		const dir = setup([{block: "7", version: "1", mode: "Slow"}, {block: "9", version: "3", mode: "Slow"}])
		const r = gateProposalVersions(join(dir, "pg.tsv"), dir)
		expect(r.ordinalViolations).toHaveLength(1)
		expect(r.ordinalViolations[0]).toContain("version 3:rank 2")
	})
})
