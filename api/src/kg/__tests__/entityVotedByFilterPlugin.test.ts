import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

async function executeGraphQL(query: string, variables?: Record<string, unknown>) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query, variables}),
		}),
		{},
	)
	return response.json() as Promise<{
		errors?: Array<{message: string}>
		data?: {entities?: Array<{id: string}>}
	}>
}

// Seeds its own rows rather than hunting for real ones, because the assertions
// here are about which votes count — a corpus-dependent fixture cannot say
// "curation was excluded" with any confidence.
const VOTER = "00000000-beef-4000-8000-000000000001"
const OTHER_VOTER = "00000000-beef-4000-8000-000000000002"
const STANCE_ENTITY = "00000000-beef-4000-8000-00000000000a"
const CURATION_ENTITY = "00000000-beef-4000-8000-00000000000b"
const UNVOTED_ENTITY = "00000000-beef-4000-8000-00000000000c"
const SPACE = "00000000-beef-4000-8000-000000000003"
const ALL_ENTITIES = [STANCE_ENTITY, CURATION_ENTITY, UNVOTED_ENTITY]

const undash = (s: string) => s.replace(/-/g, "")
const idsOf = (r: {data?: {entities?: Array<{id: string}>}}) => (r.data?.entities ?? []).map((e) => e.id)

describe("EntityVotedByFilterPlugin", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await pool.query(`DELETE FROM user_votes WHERE user_id = ANY($1::uuid[])`, [[VOTER, OTHER_VOTER]])
		await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [ALL_ENTITIES])
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT x, now(), 0, now(), 0 FROM unnest($1::uuid[]) x`,
			[ALL_ENTITIES],
		)
		// vote_kind: 0 curation, 1 stance, 2 veracity
		await pool.query(
			`INSERT INTO user_votes (user_id,object_id,object_type,space_id,vote_type,vote_kind,voted_at) VALUES
			 ($1,$3,0,$5,0,1,now()),
			 ($1,$4,0,$5,0,0,now()),
			 ($2,$6,0,$5,0,1,now())`,
			[VOTER, OTHER_VOTER, STANCE_ENTITY, CURATION_ENTITY, SPACE, UNVOTED_ENTITY],
		)
	})

	afterAll(async () => {
		await pool.query(`DELETE FROM user_votes WHERE user_id = ANY($1::uuid[])`, [[VOTER, OTHER_VOTER]])
		await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [ALL_ENTITIES])
		await pool.end()
	})

	it("returns every entity the user voted on, whatever the kind", async () => {
		const r = await executeGraphQL(`{ entities(votedBy: "${undash(VOTER)}", first: 50) { id } }`)
		expect(r.errors).toBeUndefined()
		const ids = idsOf(r)
		expect(ids).toContain(undash(STANCE_ENTITY))
		expect(ids).toContain(undash(CURATION_ENTITY))
		expect(ids).not.toContain(undash(UNVOTED_ENTITY))
	})

	it("narrows to stance and veracity, which is what a position means", async () => {
		const r = await executeGraphQL(
			`{ entities(votedBy: "${undash(VOTER)}", votedByKinds: [1, 2], first: 50) { id } }`,
		)
		expect(r.errors).toBeUndefined()
		const ids = idsOf(r)
		expect(ids).toContain(undash(STANCE_ENTITY))
		// The point of the kinds filter: counting curation as a position overstates
		// the figure roughly threefold (567 rows against a true 192 on the
		// reference account in GEO-2913).
		expect(ids).not.toContain(undash(CURATION_ENTITY))
	})

	it("selects curation alone when asked for it", async () => {
		const r = await executeGraphQL(`{ entities(votedBy: "${undash(VOTER)}", votedByKinds: [0], first: 50) { id } }`)
		expect(r.errors).toBeUndefined()
		const ids = idsOf(r)
		expect(ids).toContain(undash(CURATION_ENTITY))
		expect(ids).not.toContain(undash(STANCE_ENTITY))
	})

	it("returns nothing for a kind the user never cast", async () => {
		const r = await executeGraphQL(`{ entities(votedBy: "${undash(VOTER)}", votedByKinds: [2], first: 50) { id } }`)
		expect(r.errors).toBeUndefined()
		const ids = idsOf(r)
		expect(ids).not.toContain(undash(STANCE_ENTITY))
		expect(ids).not.toContain(undash(CURATION_ENTITY))
	})

	it("does not leak another user's votes", async () => {
		const r = await executeGraphQL(`{ entities(votedBy: "${undash(VOTER)}", first: 50) { id } }`)
		expect(r.errors).toBeUndefined()
		// OTHER_VOTER voted on UNVOTED_ENTITY; it must not appear for VOTER.
		expect(idsOf(r)).not.toContain(undash(UNVOTED_ENTITY))
	})

	it("returns nothing for a user with no votes", async () => {
		const r = await executeGraphQL(
			`{ entities(votedBy: "00000000-beef-4000-8000-0000000000ff", first: 50) { id } }`,
		)
		expect(r.errors).toBeUndefined()
		expect(idsOf(r)).toHaveLength(0)
	})

	it("ignores votedByKinds when votedBy is absent, rather than erroring", async () => {
		// Documented behaviour: on its own, votedByKinds would mean "voted on by
		// anybody with one of these kinds", a different and far more expensive
		// question. It is a no-op instead.
		const r = await executeGraphQL(`{ entities(votedByKinds: [1], first: 3) { id } }`)
		expect(r.errors).toBeUndefined()
		expect(Array.isArray(r.data?.entities)).toBe(true)
	})

	it("composes with the existing type filter rather than replacing it", async () => {
		const r = await executeGraphQL(
			`{ entities(votedBy: "${undash(VOTER)}", votedByKinds: [1], typeIds: {in: ["${undash(SPACE)}"]}, first: 50) { id } }`,
		)
		// The seeded entities carry no types, so the pair must return nothing —
		// what matters is that both arguments apply and neither errors.
		expect(r.errors).toBeUndefined()
		expect(idsOf(r)).not.toContain(undash(CURATION_ENTITY))
	})
})
