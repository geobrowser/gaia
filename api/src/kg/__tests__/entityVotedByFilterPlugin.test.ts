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
		data?: {
			entities?: Array<{id: string}>
			entitiesConnection?: {totalCount: number; nodes: Array<{id: string}>}
			relationsConnection?: {nodes: Array<{fromEntityId: string}>}
		}
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
// A stance the voter took and then took back: the row survives, rewritten to
// vote_type 2 ("neither"). GEO-2962.
const RETRACTED_ENTITY = "00000000-beef-4000-8000-00000000000d"
// A veracity answer of "dispute" (vote_type 1) — held, on the other side.
const DISPUTED_ENTITY = "00000000-beef-4000-8000-00000000000e"
// Target of the fixture relations the facet test groups over.
const TAG_ENTITY = "00000000-beef-4000-8000-00000000000f"
const TAG_PROPERTY = "00000000-beef-4000-8000-000000000010"
const SPACE = "00000000-beef-4000-8000-000000000003"
const VOTED_OR_NOT = [STANCE_ENTITY, CURATION_ENTITY, UNVOTED_ENTITY, RETRACTED_ENTITY, DISPUTED_ENTITY]
const ALL_ENTITIES = [...VOTED_OR_NOT, TAG_ENTITY]
const REL_IDS = VOTED_OR_NOT.map((_, i) => `00000000-beef-4000-8000-0000000001${String(i).padStart(2, "0")}`)

const undash = (s: string) => s.replace(/-/g, "")
const idsOf = (r: {data?: {entities?: Array<{id: string}>}}) => (r.data?.entities ?? []).map((e) => e.id)

describe("EntityVotedByFilterPlugin", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await pool.query(`DELETE FROM user_votes WHERE user_id = ANY($1::uuid[])`, [[VOTER, OTHER_VOTER]])
		await pool.query(`DELETE FROM relations WHERE id = ANY($1::uuid[])`, [REL_IDS])
		await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [ALL_ENTITIES])
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT x, now(), 0, now(), 0 FROM unnest($1::uuid[]) x`,
			[ALL_ENTITIES],
		)
		// vote_kind: 0 curation, 1 stance, 2 veracity
		// vote_type: 0 agree/verify, 1 disagree/dispute, 2 neither (retracted)
		await pool.query(
			`INSERT INTO user_votes (user_id,object_id,object_type,space_id,vote_type,vote_kind,voted_at) VALUES
			 ($1,$3,0,$5,0,1,now()),
			 ($1,$4,0,$5,0,0,now()),
			 ($2,$6,0,$5,0,1,now()),
			 ($1,$7,0,$5,2,1,now()),
			 ($1,$8,0,$5,1,2,now())`,
			[
				VOTER,
				OTHER_VOTER,
				STANCE_ENTITY,
				CURATION_ENTITY,
				SPACE,
				UNVOTED_ENTITY,
				RETRACTED_ENTITY,
				DISPUTED_ENTITY,
			],
		)
		// Every voted-or-not fixture carries one TAG_PROPERTY relation to TAG_ENTITY,
		// so a relationsConnection over them is the shape of a facet count.
		await pool.query(
			`INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id)
			 SELECT unnest($1::uuid[]), unnest($2::uuid[]), $3, unnest($2::uuid[]), $4, $5`,
			[REL_IDS, VOTED_OR_NOT, TAG_PROPERTY, TAG_ENTITY, SPACE],
		)
	})

	afterAll(async () => {
		await pool.query(`DELETE FROM user_votes WHERE user_id = ANY($1::uuid[])`, [[VOTER, OTHER_VOTER]])
		await pool.query(`DELETE FROM relations WHERE id = ANY($1::uuid[])`, [REL_IDS])
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

	// ==========================================================================
	// votedByTypes (GEO-2962)
	// ==========================================================================

	it("still counts a retracted position when votedByTypes is omitted", async () => {
		// Today's behaviour, which must not move for existing callers.
		const r = await executeGraphQL(
			`{ entities(votedBy: "${undash(VOTER)}", votedByKinds: [1, 2], first: 50) { id } }`,
		)
		expect(r.errors).toBeUndefined()
		expect(idsOf(r)).toContain(undash(RETRACTED_ENTITY))
	})

	it("leaves out retracted positions with votedByTypes: [0, 1]", async () => {
		const r = await executeGraphQL(
			`{ entities(votedBy: "${undash(VOTER)}", votedByKinds: [1, 2], votedByTypes: [0, 1], first: 50) { id } }`,
		)
		expect(r.errors).toBeUndefined()
		const ids = idsOf(r)
		expect(ids).not.toContain(undash(RETRACTED_ENTITY))
		// Both sides of a held position stay: agree on a stance, dispute on veracity.
		expect(ids).toContain(undash(STANCE_ENTITY))
		expect(ids).toContain(undash(DISPUTED_ENTITY))
		expect(ids).not.toContain(undash(CURATION_ENTITY))
	})

	it("selects only the retracted rows with votedByTypes: [2]", async () => {
		const r = await executeGraphQL(`{ entities(votedBy: "${undash(VOTER)}", votedByTypes: [2], first: 50) { id } }`)
		expect(r.errors).toBeUndefined()
		expect(idsOf(r)).toEqual([undash(RETRACTED_ENTITY)])
	})

	it("counts held positions in totalCount, which is what the profile rail reads", async () => {
		const r = await executeGraphQL(
			`{ entitiesConnection(votedBy: "${undash(VOTER)}", votedByKinds: [1, 2], votedByTypes: [0, 1]) { totalCount } }`,
		)
		expect(r.errors).toBeUndefined()
		expect(r.data?.entitiesConnection?.totalCount).toBe(2)
	})

	it("treats an empty votedByTypes as no restriction", async () => {
		const r = await executeGraphQL(
			`{ entities(votedBy: "${undash(VOTER)}", votedByKinds: [1, 2], votedByTypes: [], first: 50) { id } }`,
		)
		expect(r.errors).toBeUndefined()
		expect(idsOf(r)).toContain(undash(RETRACTED_ENTITY))
	})

	it("ignores votedByTypes when votedBy is absent, rather than erroring", async () => {
		const r = await executeGraphQL(`{ entities(votedByTypes: [0, 1], first: 3) { id } }`)
		expect(r.errors).toBeUndefined()
		expect(Array.isArray(r.data?.entities)).toBe(true)
	})

	// ==========================================================================
	// EntityFilter.votedBy (GEO-2894)
	// ==========================================================================

	const fixtureIds = `[${VOTED_OR_NOT.map((id) => `"${undash(id)}"`).join(", ")}]`
	const heldPositions = `{userId: "${undash(VOTER)}", kinds: [1, 2], types: [0, 1]}`

	it("matches the argument form when used positively in filter", async () => {
		const r = await executeGraphQL(
			`{ entities(filter: {id: {in: ${fixtureIds}}, votedBy: ${heldPositions}}, first: 50) { id } }`,
		)
		expect(r.errors).toBeUndefined()
		expect(idsOf(r).sort()).toEqual([STANCE_ENTITY, DISPUTED_ENTITY].map(undash).sort())
	})

	it("excludes what the viewer has answered with not: {votedBy}", async () => {
		const r = await executeGraphQL(
			`{ entitiesConnection(filter: {id: {in: ${fixtureIds}}, not: {votedBy: ${heldPositions}}}) {
				totalCount
				nodes { id }
			} }`,
		)
		expect(r.errors).toBeUndefined()
		const ids = (r.data?.entitiesConnection?.nodes ?? []).map((n) => n.id).sort()
		// Held positions are gone. What remains: never voted on by this user, voted
		// on only as curation, and the retracted one — which is unanswered again.
		expect(ids).toEqual([UNVOTED_ENTITY, CURATION_ENTITY, RETRACTED_ENTITY].map(undash).sort())
		expect(r.data?.entitiesConnection?.totalCount).toBe(3)
	})

	it("excludes any vote at all when kinds and types are omitted", async () => {
		const r = await executeGraphQL(
			`{ entities(filter: {id: {in: ${fixtureIds}}, not: {votedBy: {userId: "${undash(VOTER)}"}}}, first: 50) { id } }`,
		)
		expect(r.errors).toBeUndefined()
		// Another user's vote on UNVOTED_ENTITY does not count against this viewer.
		expect(idsOf(r)).toEqual([undash(UNVOTED_ENTITY)])
	})

	it("composes with the type argument", async () => {
		const r = await executeGraphQL(
			`{ entities(typeIds: {in: ["${undash(SPACE)}"]}, filter: {not: {votedBy: ${heldPositions}}}, first: 50) { id } }`,
		)
		// The fixtures carry no types, so nothing matches; both must apply without error.
		expect(r.errors).toBeUndefined()
		expect(idsOf(r)).toEqual([])
	})

	it("reaches a relation facet through fromEntity", async () => {
		// The facet counts are relationsConnection aggregates filtered by
		// `fromEntity: EntityFilter`, so the same exclusion must hold there or the
		// counts describe a different corpus from the list.
		const r = await executeGraphQL(
			`{ relationsConnection(filter: {
				typeId: {is: "${undash(TAG_PROPERTY)}"}
				fromEntity: {not: {votedBy: ${heldPositions}}}
			}) { nodes { fromEntityId } } }`,
		)
		expect(r.errors).toBeUndefined()
		const from = (r.data?.relationsConnection?.nodes ?? []).map((n) => n.fromEntityId).sort()
		expect(from).toEqual([UNVOTED_ENTITY, CURATION_ENTITY, RETRACTED_ENTITY].map(undash).sort())
	})

	it("rejects a votedBy filter without a userId", async () => {
		const r = await executeGraphQL(`{ entities(filter: {votedBy: {kinds: [1]}}, first: 1) { id } }`)
		expect(r.errors?.[0]?.message).toMatch(/userId/)
	})
})
