import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

/**
 * GEO-3150: `entitiesInBestOrder(entityIds)` returns the named entities in Best's order (ranking
 * score DESC, ties by id DESC), unscored ones last in the caller's order, missing ids omitted and
 * duplicates once. Run through the real GraphQL server so PostGraphile's handling of a SETOF
 * function's order is part of what is tested, not assumed.
 */

async function query(source: string) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query: source}),
		}),
		{},
	)
	return response.json() as Promise<{
		errors?: Array<{message: string}>
		data?: {entitiesInBestOrder?: Array<{id: string; rankingScore: number | string | null}>}
	}>
}

const undash = (id: string) => id.replaceAll("-", "")

// Scored: HIGH (0.9), MID_A and MID_B tied (0.5; MID_B has the larger id so it comes first),
// LOW (0.1). Unscored: U1 and U2, which must come last in the order they are requested.
const IDS = {
	HIGH: "31500000-0000-4000-8000-000000000001",
	MID_A: "31500000-0000-4000-8000-000000000002",
	MID_B: "31500000-0000-4000-8000-000000000003",
	LOW: "31500000-0000-4000-8000-000000000004",
	U1: "31500000-0000-4000-8000-000000000005",
	U2: "31500000-0000-4000-8000-000000000006",
}
const MISSING = "31500000-0000-4000-8000-0000000000ff"
const SCORES: Record<string, number> = {HIGH: 0.9, MID_A: 0.5, MID_B: 0.5, LOW: 0.1}

describe("entitiesInBestOrder", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		const all = Object.values(IDS)
		await pool.query("DELETE FROM entity_ranking_scores WHERE entity_id = ANY($1::uuid[])", [all])
		await pool.query("DELETE FROM entities WHERE id = ANY($1::uuid[])", [all])
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT x, '1790000000', '0', '1790000000', '0' FROM unnest($1::uuid[]) AS t(x)`,
			[all],
		)
		for (const [key, score] of Object.entries(SCORES)) {
			await pool.query(
				"INSERT INTO entity_ranking_scores (entity_id, quality_score, ranking_score) VALUES ($1, $2, $2)",
				[IDS[key as keyof typeof IDS], score],
			)
		}
	})

	afterAll(async () => {
		const all = Object.values(IDS)
		await pool.query("DELETE FROM entity_ranking_scores WHERE entity_id = ANY($1::uuid[])", [all])
		await pool.query("DELETE FROM entities WHERE id = ANY($1::uuid[])", [all])
		await pool.end()
	})

	const order = async (ids: string[]) => {
		const list = ids.map((id) => `"${id}"`).join(", ")
		const result = await query(`{ entitiesInBestOrder(entityIds: [${list}]) { id rankingScore } }`)
		expect(result.errors).toBeUndefined()
		return (result.data?.entitiesInBestOrder ?? []).map((e) => e.id)
	}

	it("orders by Best's score, ties by id descending, unscored last in request order", async () => {
		// Requested deliberately out of order, with U2 before U1.
		const got = await order([IDS.U2, IDS.LOW, IDS.MID_A, IDS.U1, IDS.HIGH, IDS.MID_B])
		expect(got).toEqual([IDS.HIGH, IDS.MID_B, IDS.MID_A, IDS.LOW, IDS.U2, IDS.U1].map(undash))
	})

	it("omits ids that do not exist and returns duplicates once, at their first position", async () => {
		const got = await order([IDS.U1, MISSING, IDS.U2, IDS.U1, IDS.LOW])
		expect(got).toEqual([IDS.LOW, IDS.U1, IDS.U2].map(undash))
	})

	it("exposes each entity's Best score alongside the order", async () => {
		const result = await query(`{ entitiesInBestOrder(entityIds: ["${IDS.HIGH}", "${IDS.U1}"]) { id rankingScore } }`)
		const [high, unscored] = result.data?.entitiesInBestOrder ?? []
		expect(Number(high?.rankingScore)).toBeCloseTo(0.9)
		expect(unscored?.rankingScore ?? null).toBeNull()
	})

	it("refuses more than 1000 ids", async () => {
		const ids = Array.from({length: 1001}, (_, i) => `31500000-0000-4000-8000-${String(i).padStart(12, "0")}`)
		const result = await query(`{ entitiesInBestOrder(entityIds: [${ids.map((id) => `"${id}"`).join(", ")}]) { id } }`)
		expect(result.errors?.[0]?.message ?? "").toMatch(/at most 1000 ids|1000/)
	})
})
