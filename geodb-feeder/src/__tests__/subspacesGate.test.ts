import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {gateSubspaces} from "../subspacesGate.js"

const P = "11111111111141118111111111111111"
const C = "22222222222242228222222222222222"
const T = "33333333333343338333333333333333"

function setup(geo: Record<string, string[]>, pg: string[]) {
	const dir = mkdtempSync(join(tmpdir(), "ss-gate-"))
	for (const t of ["verified", "related", "topic"]) writeFileSync(join(dir, `subspaces-${t}.tsv`), (geo[t] ?? []).map((l) => `${l}\n`).join(""))
	writeFileSync(join(dir, "pg.tsv"), pg.map((l) => `${l}\n`).join(""))
	return dir
}
const edge = (a: string, b: string) => `${cid("entity", a)}\t${cid("entity", b)}`

describe("subspaces gate", () => {
	it("passes when the active edges are exactly Postgres's rows", () => {
		const dir = setup({verified: [edge(P, C)], topic: [edge(P, T)]}, [`${P}\t${C}\tverified`, `${P}\t${T}\ttopic`])
		const r = gateSubspaces(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(2)
		expect(r.pgOnly).toEqual([])
		expect(r.geoOnly).toEqual([])
		expect(r.pgChecksum).toBe(r.geoChecksum)
	})
	it("reports a Postgres-only row and a geo-db-only edge, and tells the types apart", () => {
		const dir = setup({related: [edge(P, C)]}, [`${P}\t${C}\tverified`])
		const r = gateSubspaces(join(dir, "pg.tsv"), dir)
		expect(r.pgOnly).toHaveLength(1)
		expect(r.geoOnly).toHaveLength(1)
	})
	it("rejects an unknown type", () => {
		const dir = setup({}, [`${P}\t${C}\tfriend`])
		expect(() => gateSubspaces(join(dir, "pg.tsv"), dir)).toThrow(/unknown subspace type/)
	})
})
