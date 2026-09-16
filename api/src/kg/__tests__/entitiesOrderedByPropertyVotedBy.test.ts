import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

/**
 * `entitiesOrderedByProperty` must be able to scope its result to the entities a
 * given user voted on, so a person's Positions tab can offer the same Top sort
 * that Explore does for a space (GEO-2928, migration 0088).
 *
 * The cases that carry the change are the ones about *bounding*: before 0088,
 * `includeWithoutValue` required `spaceIds`, because `spaceIds` was the only
 * thing keeping the value-less candidate scan finite. `votedBy` bounds it at
 * least as tightly, so either is now accepted — and the front end hit exactly
 * that error, so it is asserted here rather than assumed.
 *
 * Vote kinds: 0 curation, 1 stance, 2 veracity. Stance and veracity together are
 * what "a position" means; an unfiltered count overstates a person's positions
 * roughly threefold, which is why `votedByKinds` exists and is not defaulted.
 */

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
		data?: {entitiesOrderedByProperty?: Array<{id: string}> | null}
	}>
}

const undash = (uuid: string) => uuid.replace(/-/g, "")

const TYPES_RELATION_ID = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"

const SPACE = "00000000-9999-4999-9999-000000000001"
const TYPE_ID = "00000000-9999-4999-9999-0000000000aa"
const PROPERTY_ID = "00000000-9999-4999-9999-0000000000bb"

const VOTER = "00000000-9999-4999-9999-0000000000c1"
const OTHER_VOTER = "00000000-9999-4999-9999-0000000000c2"

// Scored and voted (stance). integer = 100 / 10.
const E_VOTED_HIGH = "00000000-8888-4888-8888-000000000001"
const E_VOTED_LOW = "00000000-8888-4888-8888-000000000002"
// Voted (stance) but carries NO value for PROPERTY_ID — only reachable with
// includeWithoutValue, and the whole point of the Top sort on a profile.
const E_VOTED_NOVAL = "00000000-8888-4888-8888-000000000003"
// Voted with kind 0 (curation) only — must drop out under votedByKinds: [1, 2].
const E_VOTED_CURATION = "00000000-8888-4888-8888-000000000004"
// Scored, same type and space, but voted on by somebody else entirely.
const E_OTHER_VOTER = "00000000-8888-4888-8888-000000000005"
// Scored, same type and space, never voted on at all.
const E_UNVOTED = "00000000-8888-4888-8888-000000000006"

const ALL_ENTITY_IDS = [E_VOTED_HIGH, E_VOTED_LOW, E_VOTED_NOVAL, E_VOTED_CURATION, E_OTHER_VOTER, E_UNVOTED]

const REL_IDS = ALL_ENTITY_IDS.map((_, i) => `00000000-7777-4777-7777-00000000000${i + 1}`)

const ORDER_QUERY = `
	query Q(
		$propertyId: UUID!
		$spaceIds: [UUID!]
		$typeIds: [UUID!]
		$dir: SortOrder!
		$include: Boolean
		$votedBy: UUID
		$votedByKinds: [Int!]
	) {
		entitiesOrderedByProperty(
			propertyId: $propertyId
			spaceIds: $spaceIds
			typeIds: $typeIds
			dataType: "integer"
			sortDirection: $dir
			includeWithoutValue: $include
			votedBy: $votedBy
			votedByKinds: $votedByKinds
			first: 50
		) {
			id
		}
	}
`

async function order(variables: Record<string, unknown>): Promise<string[]> {
	const result = await executeGraphQL(ORDER_QUERY, {
		propertyId: PROPERTY_ID,
		dir: "DESC",
		...variables,
	})
	expect(result.errors, JSON.stringify(result.errors)).toBeUndefined()
	return (result.data?.entitiesOrderedByProperty ?? []).map((e) => e.id)
}

describe("entitiesOrderedByProperty votedBy", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await cleanup()

		await pool.query(
			`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xvotedby') ON CONFLICT DO NOTHING`,
			[SPACE],
		)
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT unnest($1::uuid[]), '0', '0', '0', '0'`,
			[ALL_ENTITY_IDS],
		)
		// Every fixture entity is of TYPE_ID in SPACE, so the type filter can never
		// be what separates them — only the votes can.
		await pool.query(
			`INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id)
			 SELECT unnest($1::uuid[]), unnest($2::uuid[]), $3, unnest($2::uuid[]), $4, $5`,
			[REL_IDS, ALL_ENTITY_IDS, TYPES_RELATION_ID, TYPE_ID, SPACE],
		)
		await pool.query(
			`INSERT INTO "values" (id, property_id, entity_id, space_id, integer) VALUES
				($1, $7, $2, $8, 100),
				($3, $7, $4, $8, 10),
				($5, $7, $6, $8, 50),
				($9, $7, $10, $8, 70),
				($11, $7, $12, $8, 5)`,
			[
				`vb-${E_VOTED_HIGH}`,
				E_VOTED_HIGH,
				`vb-${E_VOTED_LOW}`,
				E_VOTED_LOW,
				`vb-${E_OTHER_VOTER}`,
				E_OTHER_VOTER,
				PROPERTY_ID,
				SPACE,
				`vb-${E_UNVOTED}`,
				E_UNVOTED,
				`vb-${E_VOTED_CURATION}`,
				E_VOTED_CURATION,
			],
		)
		// E_VOTED_NOVAL deliberately gets no value row.
		await pool.query(
			`INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, voted_at, vote_kind)
			 VALUES
				($1, $2, 0, $8, 1, now(), 1),
				($1, $3, 0, $8, 1, now(), 2),
				($1, $4, 0, $8, 1, now(), 1),
				($1, $5, 0, $8, 1, now(), 0),
				($6, $7, 0, $8, 1, now(), 1)`,
			[VOTER, E_VOTED_HIGH, E_VOTED_LOW, E_VOTED_NOVAL, E_VOTED_CURATION, OTHER_VOTER, E_OTHER_VOTER, SPACE],
		)
	})

	afterAll(async () => {
		await cleanup()
		await pool.end()
	})

	async function cleanup() {
		await pool.query(`DELETE FROM user_votes WHERE user_id = ANY($1::uuid[])`, [[VOTER, OTHER_VOTER]])
		await pool.query(`DELETE FROM "values" WHERE entity_id = ANY($1::uuid[])`, [ALL_ENTITY_IDS])
		await pool.query(`DELETE FROM relations WHERE id = ANY($1::uuid[])`, [REL_IDS])
		await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [ALL_ENTITY_IDS])
		await pool.query(`DELETE FROM spaces WHERE id = $1`, [SPACE])
	}

	it("returns every scored entity of the type when votedBy is omitted", async () => {
		// The control. Without the argument nothing is scoped to a voter, so all
		// five scored entities come back — this is what the change must not alter.
		const ids = await order({spaceIds: [SPACE], typeIds: [TYPE_ID]})
		expect(ids).toEqual([E_VOTED_HIGH, E_UNVOTED, E_OTHER_VOTER, E_VOTED_LOW, E_VOTED_CURATION].map(undash))
	})

	it("scopes the scored result to one voter, ordered by the property", async () => {
		const ids = await order({spaceIds: [SPACE], typeIds: [TYPE_ID], votedBy: VOTER})
		// 100, 10, 5 — another voter's entity and the never-voted one are gone.
		expect(ids).toEqual([E_VOTED_HIGH, E_VOTED_LOW, E_VOTED_CURATION].map(undash))
	})

	it("narrows to stance and veracity with votedByKinds", async () => {
		const ids = await order({
			spaceIds: [SPACE],
			typeIds: [TYPE_ID],
			votedBy: VOTER,
			votedByKinds: [1, 2],
		})
		// The curation-only vote drops out; this is the threefold overcount the
		// argument exists to prevent.
		expect(ids).toEqual([E_VOTED_HIGH, E_VOTED_LOW].map(undash))
	})

	it("includes the voter's unscored entities, sorted as zero", async () => {
		const ids = await order({
			spaceIds: [SPACE],
			typeIds: [TYPE_ID],
			votedBy: VOTER,
			votedByKinds: [1, 2],
			include: true,
		})
		// integer is numeric, so a missing value sorts as zero: 100, 10, then 0.
		expect(ids).toEqual([E_VOTED_HIGH, E_VOTED_LOW, E_VOTED_NOVAL].map(undash))
	})

	it("accepts includeWithoutValue with votedBy and no spaceIds", async () => {
		// Before 0088 this raised `space_ids is required when include_without_value
		// is true`, which is what blocked the Top sort on the profile tabs. The
		// vote set bounds the scan in spaceIds' place.
		const ids = await order({
			typeIds: [TYPE_ID],
			votedBy: VOTER,
			votedByKinds: [1, 2],
			include: true,
		})
		expect(ids).toEqual([E_VOTED_HIGH, E_VOTED_LOW, E_VOTED_NOVAL].map(undash))
	})

	it("accepts includeWithoutValue with votedBy and no typeIds", async () => {
		// The other relaxed guard: without a type filter the candidate set is the
		// voted entities themselves, which is bounded and cheap.
		const ids = await order({
			spaceIds: [SPACE],
			votedBy: VOTER,
			votedByKinds: [1, 2],
			include: true,
		})
		expect(ids).toEqual([E_VOTED_HIGH, E_VOTED_LOW, E_VOTED_NOVAL].map(undash))
	})

	it("still rejects includeWithoutValue with neither spaceIds nor votedBy", async () => {
		// The guard must not have been removed, only widened — an unbounded
		// value-less scan is the thing it exists to prevent.
		const result = await executeGraphQL(ORDER_QUERY, {
			propertyId: PROPERTY_ID,
			dir: "DESC",
			typeIds: [TYPE_ID],
			include: true,
		})
		expect(result.errors?.[0]?.message).toContain("required when include_without_value is true")
	})

	it("honours sort direction under votedBy", async () => {
		const ids = await order({
			spaceIds: [SPACE],
			typeIds: [TYPE_ID],
			votedBy: VOTER,
			votedByKinds: [1, 2],
			include: true,
			dir: "ASC",
		})
		expect(ids).toEqual([E_VOTED_NOVAL, E_VOTED_LOW, E_VOTED_HIGH].map(undash))
	})

	it("returns nothing for a voter with no votes", async () => {
		const ids = await order({
			spaceIds: [SPACE],
			typeIds: [TYPE_ID],
			votedBy: "00000000-9999-4999-9999-0000000000ff",
		})
		expect(ids).toEqual([])
	})

	it("keeps the type filter when both votedBy and typeIds are given", async () => {
		// A Positions tab scoped to Claims must stay scoped: a voted entity of
		// another type must not reappear through the vote path.
		const ids = await order({
			spaceIds: [SPACE],
			typeIds: ["00000000-9999-4999-9999-0000000000ee"],
			votedBy: VOTER,
			include: true,
		})
		expect(ids).toEqual([])
	})
})
