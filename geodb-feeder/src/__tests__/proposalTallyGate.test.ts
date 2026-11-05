import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {gateProposalTallies, parseTallyRow, tallyKey} from "../proposalTallyGate.js"

const P = "11111111111141118111111111111111"
const Q = "44444444444444448444444444444444"

const value = (yes: number, no: number, abstain: number) => {
	const b = Buffer.alloc(24)
	b.writeBigUInt64LE(BigInt(yes), 0)
	b.writeBigUInt64LE(BigInt(no), 8)
	b.writeBigUInt64LE(BigInt(abstain), 16)
	return b.toString("hex")
}

function files(pg: string[], geo: string[]) {
	const dir = mkdtempSync(join(tmpdir(), "tally-gate-"))
	writeFileSync(join(dir, "pg.tsv"), `${pg.join("\n")}\n`)
	writeFileSync(join(dir, "geo.tsv"), `${[...geo, "more"].join("\n")}\n`)
	return [join(dir, "pg.tsv"), join(dir, "geo.tsv")] as const
}

const row = (proposal: string, version: number, v: string) => `3\t${tallyKey(proposal, version)}\t${v}`

describe("proposal tally gate", () => {
	it("keys a tally by the proposal's entity id and its version, big-endian", () => {
		const key = tallyKey(P, 258)
		expect(key).toHaveLength(40)
		expect(key.endsWith("00000102")).toBe(true)
	})

	it("reads a derived scan row and ignores the rest", () => {
		expect(parseTallyRow(row(P, 1, value(2, 1, 0)))?.tally).toEqual({yes: 2, no: 1, abstain: 0})
		expect(parseTallyRow("more")).toBeNull()
		expect(parseTallyRow("3\tabcd\t00")).toBeNull()
	})

	it("passes when Postgres and geo-db agree", () => {
		const [pg, geo] = files([`${P}\t1\t2\t1\t0`, `${Q}\t1\t0\t0\t0`], [row(P, 1, value(2, 1, 0))])
		const r = gateProposalTallies(pg, geo)
		expect(r).toMatchObject({pgVersions: 1, geoVersions: 1, matched: 1, geoOnly: [], pgOnly: [], countDiffs: []})
	})

	it("reports a count difference with both sides", () => {
		const [pg, geo] = files([`${P}\t1\t2\t1\t0`], [row(P, 1, value(1, 2, 0))])
		const r = gateProposalTallies(pg, geo)
		expect(r.matched).toBe(0)
		expect(r.countDiffs).toEqual([{proposal: P, version: 1, pg: {yes: 2, no: 1, abstain: 0}, geo: {yes: 1, no: 2, abstain: 0}}])
	})

	it("separates a version geo-db lacks from one only geo-db holds", () => {
		const [pg, geo] = files([`${P}\t1\t1\t0\t0`], [row(Q, 3, value(0, 1, 0))])
		const r = gateProposalTallies(pg, geo)
		expect(r.pgOnly).toEqual([`${P}:1`])
		expect(r.geoOnly).toEqual([tallyKey(Q, 3)])
	})
})
