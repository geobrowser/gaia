import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {gateTypeRanking, parseTypeRankingRow, typeRankingKey} from "../typeRankingGate.js"

const T = "11111111-1111-4111-8111-111111111111"
const E1 = "22222222222242228222222222222222"
const E2 = "33333333-3333-4333-8333-333333333333"

function files(pg: string[], geo: string[]) {
	const dir = mkdtempSync(join(tmpdir(), "type-ranking-gate-"))
	writeFileSync(join(dir, "pg.tsv"), `${pg.join("\n")}\n`)
	writeFileSync(join(dir, "geo.tsv"), `${[...geo, "more"].join("\n")}\n`)
	return [join(dir, "pg.tsv"), join(dir, "geo.tsv")] as const
}

const geo = (type: string, entity: string, score: number) => `${score}\t${typeRankingKey(type, entity)}\t`

describe("type ranking gate", () => {
	it("keys a row by the raw type and entity uuids, dashes or not", () => {
		const key = typeRankingKey(T, E1)
		expect(key).toHaveLength(64)
		expect(key.slice(0, 32)).toBe("11111111111141118111111111111111")
		expect(typeRankingKey(T, E2)).toBe(typeRankingKey(T.replaceAll("-", ""), E2.replaceAll("-", "")))
	})

	it("reads a derived scan row and ignores the rest", () => {
		expect(parseTypeRankingRow(geo(T, E1, 2.5))).toEqual({key: typeRankingKey(T, E1), score: 2.5})
		expect(parseTypeRankingRow("more")).toBeNull()
		expect(parseTypeRankingRow("1\tabcd\t")).toBeNull()
	})

	it("passes when Postgres and geo-db agree", () => {
		const [pg, g] = files([`${T}\t${E1}\t2.5`, `${T}\t${E2}\t1`], [geo(T, E1, 2.5), geo(T, E2, 1)])
		const r = gateTypeRanking(pg, g)
		expect(r).toMatchObject({pgRows: 2, geoRows: 2, matched: 2, geoOnly: [], pgOnly: [], scoreDiffs: []})
	})

	it("classifies a missing, an extra and a changed row", () => {
		const [pg, g] = files([`${T}\t${E1}\t2.5`, `${T}\t${E2}\t1`], [geo(T, E1, 9), geo("44444444444444448444444444444444", E2, 1)])
		const r = gateTypeRanking(pg, g)
		expect(r.matched).toBe(0)
		expect(r.scoreDiffs).toEqual([{type: T.replaceAll("-", ""), entity: E1, pg: 2.5, geo: 9}])
		expect(r.pgOnly).toEqual([`${T.replaceAll("-", "")}:${E2.replaceAll("-", "")}`])
		expect(r.geoOnly).toHaveLength(1)
	})

	it("tolerates decimal rounding of an equal score", () => {
		const [pg, g] = files([`${T}\t${E1}\t0.30000000000000004`], [geo(T, E1, 0.3)])
		expect(gateTypeRanking(pg, g).matched).toBe(1)
	})
})
