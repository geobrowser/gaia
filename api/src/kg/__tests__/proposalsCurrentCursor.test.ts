import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

async function executeGraphQL(query: string) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query}),
		}),
		{},
	)
	return response.json() as Promise<{
		errors?: Array<{message: string}>
		data?: {
			proposalsCurrentsConnection?: {
				pageInfo: {endCursor: string | null}
				nodes: Array<{id: string}>
			}
		}
	}>
}

/**
 * Guards cursor pagination on `proposals_current` (GEO-2920).
 *
 * The view has no primary key for postgraphile to infer, so it falls back to an
 * offset-shaped cursor — `["created_at_desc", 1]` — and then refuses to read it
 * back, failing every page after the first with INTERNAL_SERVER_ERROR. Migration
 * 0087 tells it which column identifies a row with a `@primaryKey id` smart
 * comment.
 *
 * The cursor *shape* is asserted rather than only the round trip, because the
 * round trip alone would still pass if someone later widened the ordering by
 * hand while leaving the view unkeyed. An offset-shaped cursor is the actual
 * defect, and it is visible without a second request.
 *
 * Note for anyone changing the view: 0072 recreated it with DROP VIEW + CREATE
 * VIEW, which discards comments. This test is what will tell you that happened.
 */

const SPACE = "00000000-cafe-4000-8000-00000000f001"
const EDITOR = "00000000-cafe-4000-8000-00000000e001"
const PROPOSALS = [1, 2, 3, 4].map((n) => `00000000-cafe-4000-8000-0000000000${String(n).padStart(2, "0")}`)

describe("proposalsCurrents cursor pagination", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await pool.query(`DELETE FROM proposal_versions WHERE proposal_id = ANY($1::uuid[])`, [PROPOSALS])
		await pool.query(`DELETE FROM proposals WHERE id = ANY($1::uuid[])`, [PROPOSALS])
		await pool.query(`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xtest') ON CONFLICT DO NOTHING`, [
			SPACE,
		])
		await pool.query(
			`INSERT INTO proposals (id, space_id, proposed_by, created_at, created_at_block, current_version)
			 SELECT x, $2, $3, 1700000000 + ord, 100 + ord, 1
			 FROM unnest($1::uuid[]) WITH ORDINALITY AS t(x, ord)`,
			[PROPOSALS, SPACE, EDITOR],
		)
		await pool.query(
			`INSERT INTO proposal_versions (proposal_id, proposal_version, voting_mode, start_time, end_time,
			   quorum, threshold, partial_percentage_support_threshold, universal_percentage_support_threshold,
			   flat_support_threshold, yes_count, no_count, abstain_count, version_created_at, version_created_at_block)
			 SELECT x, 1, 'Fast', 1700000000, 1700001000, 1, 1, 1, 1, 1, 0, 0, 0, 1700000000, 100
			 FROM unnest($1::uuid[]) x`,
			[PROPOSALS],
		)
	})

	afterAll(async () => {
		await pool.query(`DELETE FROM proposal_versions WHERE proposal_id = ANY($1::uuid[])`, [PROPOSALS])
		await pool.query(`DELETE FROM proposals WHERE id = ANY($1::uuid[])`, [PROPOSALS])
		await pool.end()
	})

	it("issues a value-based cursor, not an offset", async () => {
		const r = await executeGraphQL(
			`{ proposalsCurrentsConnection(first: 2, orderBy: CREATED_AT_DESC) { pageInfo { endCursor } nodes { id } } }`,
		)
		expect(r.errors).toBeUndefined()
		const cursor = r.data?.proposalsCurrentsConnection?.pageInfo.endCursor
		expect(cursor).toBeTruthy()

		const decoded = JSON.parse(Buffer.from(cursor as string, "base64").toString()) as [string, unknown]
		// ["created_at_desc", ["1700000004", "00000000-…"]] when keyed;
		// ["created_at_desc", 2] when not.
		expect(
			Array.isArray(decoded[1]),
			`cursor is offset-shaped (${JSON.stringify(decoded)}), which means postgraphile cannot ` +
				`identify a row in proposals_current — check the @primaryKey comment from migration 0087 ` +
				`survived, since DROP VIEW discards it`,
		).toBe(true)
	})

	it("accepts its own cursor as `after`", async () => {
		const first = await executeGraphQL(
			`{ proposalsCurrentsConnection(first: 2, orderBy: CREATED_AT_DESC) { pageInfo { endCursor } nodes { id } } }`,
		)
		const cursor = first.data?.proposalsCurrentsConnection?.pageInfo.endCursor as string
		const firstIds = (first.data?.proposalsCurrentsConnection?.nodes ?? []).map((n) => n.id)

		const second = await executeGraphQL(
			`{ proposalsCurrentsConnection(first: 2, after: "${cursor}", orderBy: CREATED_AT_DESC) { nodes { id } } }`,
		)
		expect(second.errors).toBeUndefined()
		const secondIds = (second.data?.proposalsCurrentsConnection?.nodes ?? []).map((n) => n.id)
		expect(secondIds.length).toBeGreaterThan(0)
		// A page that repeats the first page's rows would mean the cursor was
		// accepted but ignored, which is worse than an error.
		expect(secondIds.some((id) => firstIds.includes(id))).toBe(false)
	})

	it("exposes the row accessors postgraphile only generates for a keyed relation", async () => {
		const r = (await executeGraphQL(`{ __type(name:"Query"){ fields { name } } }`)) as unknown as {
			data: {__type: {fields: Array<{name: string}>}}
		}
		const names = r.data.__type.fields.map((f) => f.name)
		expect(names).toContain("proposalsCurrent")
		expect(names).toContain("proposalsCurrentByNodeId")
	})
})
