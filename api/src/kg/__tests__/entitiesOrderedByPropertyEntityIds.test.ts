import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

/**
 * `entitiesOrderedByProperty` can be restricted to a list of entities with
 * `entityIds` (migration 0094).
 *
 * geogenesis data blocks and collections used `filter: { id: { in: [...] } }`
 * for this, which applies OUTSIDE the function, after it has sorted every entity
 * carrying the property. Sorting one entity by name took 12-25 s on testnet.
 * `entityIds` applies inside, so it bounds the scan the same way `votedBy` does
 * (0088) — including standing in for `spaceIds` as the bound that
 * `includeWithoutValue` requires.
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

const SPACE = "00000000-9999-4999-9999-000000000101"
const TYPE_ID = "00000000-9999-4999-9999-0000000001aa"
const PROPERTY_ID = "00000000-9999-4999-9999-0000000001bb"
const VOTER = "00000000-9999-4999-9999-0000000001c1"

// Scored: integer = 40 / 30 / 20 / 10.
const E_40 = "00000000-6666-4666-8666-000000000001"
const E_30 = "00000000-6666-4666-8666-000000000002"
const E_20 = "00000000-6666-4666-8666-000000000003"
const E_10 = "00000000-6666-4666-8666-000000000004"
// Same type and space, but no value for PROPERTY_ID.
const E_NOVAL = "00000000-6666-4666-8666-000000000005"
const E_NOVAL_2 = "00000000-6666-4666-8666-000000000006"

const ALL_ENTITY_IDS = [E_40, E_30, E_20, E_10, E_NOVAL, E_NOVAL_2]
const REL_IDS = ALL_ENTITY_IDS.map((_, i) => `00000000-5555-4555-8555-00000000000${i + 1}`)

const ORDER_QUERY = `
	query Q(
		$propertyId: UUID!
		$spaceIds: [UUID!]
		$typeIds: [UUID!]
		$dir: SortOrder!
		$include: Boolean
		$votedBy: UUID
		$entityIds: [UUID!]
	) {
		entitiesOrderedByProperty(
			propertyId: $propertyId
			spaceIds: $spaceIds
			typeIds: $typeIds
			dataType: "integer"
			sortDirection: $dir
			includeWithoutValue: $include
			votedBy: $votedBy
			entityIds: $entityIds
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

describe("entitiesOrderedByProperty entityIds", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await cleanup()

		await pool.query(
			`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xentityids') ON CONFLICT DO NOTHING`,
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
				($1, $9, $2, $10, 40),
				($3, $9, $4, $10, 30),
				($5, $9, $6, $10, 20),
				($7, $9, $8, $10, 10)`,
			[`ei-${E_40}`, E_40, `ei-${E_30}`, E_30, `ei-${E_20}`, E_20, `ei-${E_10}`, E_10, PROPERTY_ID, SPACE],
		)
		// VOTER voted on E_40, E_20 and E_NOVAL.
		await pool.query(
			`INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, voted_at, vote_kind)
			 VALUES ($1, $2, 0, $5, 1, now(), 1), ($1, $3, 0, $5, 1, now(), 1), ($1, $4, 0, $5, 1, now(), 1)`,
			[VOTER, E_40, E_20, E_NOVAL, SPACE],
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

	it("returns every scored entity when entityIds is omitted", async () => {
		// The control: callers that do not send the argument see no change.
		const ids = await order({spaceIds: [SPACE], typeIds: [TYPE_ID]})
		expect(ids).toEqual([E_40, E_30, E_20, E_10].map(undash))
	})

	it("restricts to entityIds with no space or type bound, ordered by the property", async () => {
		// The shape that was slow in production: only the ids bound the scan.
		const ids = await order({entityIds: [E_10, E_30]})
		expect(ids).toEqual([E_30, E_10].map(undash))
	})

	it("returns a single listed entity", async () => {
		const ids = await order({entityIds: [E_20]})
		expect(ids).toEqual([E_20].map(undash))
	})

	it("intersects entityIds with spaceIds and typeIds", async () => {
		const ids = await order({spaceIds: [SPACE], typeIds: [TYPE_ID], entityIds: [E_40, E_10]})
		expect(ids).toEqual([E_40, E_10].map(undash))
	})

	it("intersects entityIds with votedBy", async () => {
		// Voted: E_40, E_20. Listed: E_20, E_30. Only E_20 is both.
		const ids = await order({votedBy: VOTER, entityIds: [E_20, E_30]})
		expect(ids).toEqual([E_20].map(undash))
	})

	it("accepts includeWithoutValue with entityIds alone", async () => {
		// Before 0094 this raised: nothing bounded the value-less scan. The listed
		// ids are the candidates, so they bound it; an unlisted value-less entity
		// (E_NOVAL_2) must not appear.
		const ids = await order({entityIds: [E_30, E_NOVAL], include: true})
		expect(ids).toEqual([E_30, E_NOVAL].map(undash))
	})

	it("restricts the value-less candidates when typeIds is also given", async () => {
		const ids = await order({typeIds: [TYPE_ID], entityIds: [E_NOVAL, E_10], include: true})
		expect(ids).toEqual([E_10, E_NOVAL].map(undash))
	})

	it("restricts the voted candidates when votedBy is also given", async () => {
		// Voted: E_40, E_20, E_NOVAL. Listed: E_NOVAL, E_30. Only E_NOVAL is both.
		const ids = await order({votedBy: VOTER, entityIds: [E_NOVAL, E_30], include: true})
		expect(ids).toEqual([E_NOVAL].map(undash))
	})

	it("treats an empty entityIds as no restriction", async () => {
		const ids = await order({spaceIds: [SPACE], typeIds: [TYPE_ID], entityIds: []})
		expect(ids).toEqual([E_40, E_30, E_20, E_10].map(undash))
	})

	it("still rejects includeWithoutValue with nothing bounding it", async () => {
		const result = await executeGraphQL(ORDER_QUERY, {
			propertyId: PROPERTY_ID,
			dir: "DESC",
			typeIds: [TYPE_ID],
			include: true,
			entityIds: [],
		})
		expect(result.errors?.[0]?.message).toContain("required when include_without_value is true")
	})
})
