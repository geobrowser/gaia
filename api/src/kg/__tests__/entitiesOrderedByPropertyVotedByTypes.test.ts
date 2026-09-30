import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

/**
 * `entitiesOrderedByProperty(votedBy:)` must be able to leave out positions the
 * voter has taken back (GEO-2962, migration 0099).
 *
 * `user_votes` is unique per (user, object, object_type, space, kind), so a
 * retraction rewrites the row to vote_type 2 rather than deleting it. Without
 * `votedByTypes` the Top sort on a Positions tab lists those claims, and the
 * front end has to fetch the complete id list to post-filter it.
 *
 * vote_type: 0 agree/verify, 1 disagree/dispute, 2 neither (retracted).
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

const SPACE = "00000000-9962-4962-9962-000000000001"
const TYPE_ID = "00000000-9962-4962-9962-0000000000aa"
const PROPERTY_ID = "00000000-9962-4962-9962-0000000000bb"
const VOTER = "00000000-9962-4962-9962-0000000000c1"

// Scored, held: agree (100) and dispute (10).
const E_AGREED = "00000000-2962-4962-8962-000000000001"
const E_DISPUTED = "00000000-2962-4962-8962-000000000002"
// Scored, retracted (vote_type 2). Its score is the highest, so if it leaks it
// leads the list.
const E_RETRACTED = "00000000-2962-4962-8962-000000000003"
// Unscored and retracted — only reachable through includeWithoutValue, which
// uses the vote set as its candidate source.
const E_RETRACTED_NOVAL = "00000000-2962-4962-8962-000000000004"
// Unscored and held.
const E_HELD_NOVAL = "00000000-2962-4962-8962-000000000005"

const ALL_ENTITY_IDS = [E_AGREED, E_DISPUTED, E_RETRACTED, E_RETRACTED_NOVAL, E_HELD_NOVAL]
const REL_IDS = ALL_ENTITY_IDS.map((_, i) => `00000000-2962-4962-7962-00000000000${i + 1}`)

const ORDER_QUERY = `
	query Q(
		$propertyId: UUID!
		$spaceIds: [UUID!]
		$typeIds: [UUID!]
		$include: Boolean
		$votedBy: UUID
		$votedByKinds: [Int!]
		$votedByTypes: [Int!]
	) {
		entitiesOrderedByProperty(
			propertyId: $propertyId
			spaceIds: $spaceIds
			typeIds: $typeIds
			dataType: "integer"
			sortDirection: DESC
			includeWithoutValue: $include
			votedBy: $votedBy
			votedByKinds: $votedByKinds
			votedByTypes: $votedByTypes
			first: 50
		) {
			id
		}
	}
`

async function order(variables: Record<string, unknown>): Promise<string[]> {
	const result = await executeGraphQL(ORDER_QUERY, {propertyId: PROPERTY_ID, ...variables})
	expect(result.errors, JSON.stringify(result.errors)).toBeUndefined()
	return (result.data?.entitiesOrderedByProperty ?? []).map((e) => e.id)
}

describe("entitiesOrderedByProperty votedByTypes", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await cleanup()

		await pool.query(
			`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xvotedbytypes') ON CONFLICT DO NOTHING`,
			[SPACE],
		)
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT unnest($1::uuid[]), '0', '0', '0', '0'`,
			[ALL_ENTITY_IDS],
		)
		await pool.query(
			`INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id)
			 SELECT unnest($1::uuid[]), unnest($2::uuid[]), $3, unnest($2::uuid[]), $4, $5`,
			[REL_IDS, ALL_ENTITY_IDS, TYPES_RELATION_ID, TYPE_ID, SPACE],
		)
		await pool.query(
			`INSERT INTO "values" (id, property_id, entity_id, space_id, integer) VALUES
				($1, $7, $2, $8, 100),
				($3, $7, $4, $8, 10),
				($5, $7, $6, $8, 500)`,
			[
				`vbt-${E_AGREED}`,
				E_AGREED,
				`vbt-${E_DISPUTED}`,
				E_DISPUTED,
				`vbt-${E_RETRACTED}`,
				E_RETRACTED,
				PROPERTY_ID,
				SPACE,
			],
		)
		// (user, object, 0, space, vote_type, voted_at, vote_kind)
		await pool.query(
			`INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, voted_at, vote_kind)
			 VALUES
				($1, $2, 0, $7, 0, now(), 1),
				($1, $3, 0, $7, 1, now(), 2),
				($1, $4, 0, $7, 2, now(), 1),
				($1, $5, 0, $7, 2, now(), 1),
				($1, $6, 0, $7, 0, now(), 1)`,
			[VOTER, E_AGREED, E_DISPUTED, E_RETRACTED, E_RETRACTED_NOVAL, E_HELD_NOVAL, SPACE],
		)
	})

	afterAll(async () => {
		await cleanup()
		await pool.end()
	})

	async function cleanup() {
		await pool.query(`DELETE FROM user_votes WHERE user_id = $1`, [VOTER])
		await pool.query(`DELETE FROM "values" WHERE entity_id = ANY($1::uuid[])`, [ALL_ENTITY_IDS])
		await pool.query(`DELETE FROM relations WHERE id = ANY($1::uuid[])`, [REL_IDS])
		await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [ALL_ENTITY_IDS])
		await pool.query(`DELETE FROM spaces WHERE id = $1`, [SPACE])
	}

	it("keeps the retracted row when votedByTypes is omitted", async () => {
		// Today's behaviour; existing callers must see no change.
		const ids = await order({spaceIds: [SPACE], typeIds: [TYPE_ID], votedBy: VOTER, votedByKinds: [1, 2]})
		expect(ids).toEqual([E_RETRACTED, E_AGREED, E_DISPUTED].map(undash))
	})

	it("drops the retracted row with votedByTypes: [0, 1]", async () => {
		const ids = await order({
			spaceIds: [SPACE],
			typeIds: [TYPE_ID],
			votedBy: VOTER,
			votedByKinds: [1, 2],
			votedByTypes: [0, 1],
		})
		expect(ids).toEqual([E_AGREED, E_DISPUTED].map(undash))
	})

	it("drops retracted rows from the value-less branch too", async () => {
		// With includeWithoutValue and no typeIds the candidates ARE the vote set,
		// so the type filter has to reach it or retracted claims come back as
		// unscored rows at the bottom.
		const ids = await order({
			spaceIds: [SPACE],
			votedBy: VOTER,
			votedByKinds: [1, 2],
			votedByTypes: [0, 1],
			include: true,
		})
		expect(ids).toEqual([E_AGREED, E_DISPUTED, E_HELD_NOVAL].map(undash))
	})

	it("drops retracted rows from the typed candidate set", async () => {
		const ids = await order({
			typeIds: [TYPE_ID],
			votedBy: VOTER,
			votedByTypes: [0, 1],
			include: true,
		})
		expect(ids).toEqual([E_AGREED, E_DISPUTED, E_HELD_NOVAL].map(undash))
	})

	it("treats an empty votedByTypes as no restriction", async () => {
		const ids = await order({spaceIds: [SPACE], typeIds: [TYPE_ID], votedBy: VOTER, votedByTypes: []})
		expect(ids).toEqual([E_RETRACTED, E_AGREED, E_DISPUTED].map(undash))
	})

	it("ignores votedByTypes without votedBy", async () => {
		const ids = await order({spaceIds: [SPACE], typeIds: [TYPE_ID], votedByTypes: [0, 1]})
		expect(ids).toEqual([E_RETRACTED, E_AGREED, E_DISPUTED].map(undash))
	})
})
