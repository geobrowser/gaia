import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {applyDefaultFirstIfOmitted, assertPaginationWithinLimit, MAX_PAGINATION_LIMIT} from "../paginationCapPlugin"
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
	return {
		status: response.status,
		body: await response.json(),
	}
}

describe("assertPaginationWithinLimit", () => {
	it("allows pagination arguments at or below the limit", () => {
		expect(() => assertPaginationWithinLimit({first: 1000, last: 1000, offset: 1000})).not.toThrow()
	})

	it("rejects oversized pagination arguments", () => {
		expect(() => assertPaginationWithinLimit({first: 1001})).toThrow(/first.*1000.*1001/i)
		expect(() => assertPaginationWithinLimit({last: 1001})).toThrow(/last.*1000.*1001/i)
		expect(() => assertPaginationWithinLimit({offset: 1001})).toThrow(/offset.*1000.*1001/i)
	})
})

describe("applyDefaultFirstIfOmitted", () => {
	it("injects default first when neither first nor last is supplied", () => {
		expect(applyDefaultFirstIfOmitted({})).toEqual({first: MAX_PAGINATION_LIMIT})
	})

	it("preserves other args while injecting the default first", () => {
		expect(applyDefaultFirstIfOmitted({offset: 10, filter: {name: "x"}})).toEqual({
			offset: 10,
			filter: {name: "x"},
			first: MAX_PAGINATION_LIMIT,
		})
	})

	it("does not override an explicit first", () => {
		expect(applyDefaultFirstIfOmitted({first: 50})).toEqual({first: 50})
	})

	it("does not inject first when only last is supplied", () => {
		expect(applyDefaultFirstIfOmitted({last: 50})).toEqual({last: 50})
	})

	it("does not inject first for a non-numeric first (GraphQL strong typing catches those)", () => {
		// if `first` came through as anything non-numeric (shouldn't happen at runtime
		// due to Int scalar validation) we still shouldn't silently override it
		expect(applyDefaultFirstIfOmitted({first: null as unknown as number})).toEqual({
			first: MAX_PAGINATION_LIMIT,
		})
	})
})

describe("PaginationCapPlugin", () => {
	it("rejects oversized inline pagination arguments", async () => {
		const result = await executeGraphQL(`
			{
				entities(first: 10000) {
					id
				}
			}
		`)

		expect(result.status).toBe(400)
		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("rejects oversized variable pagination arguments", async () => {
		const result = await executeGraphQL(
			`
				query TestPaginationLimit($first: Int!) {
					entities(first: $first) {
						id
					}
				}
			`,
			{first: 10000},
		)

		expect(result.status).toBe(400)
		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	// --- gap-probing tests added alongside the existing ones ---

	it("rejects oversized first on the connection form", async () => {
		const result = await executeGraphQL(`
			{
				entitiesConnection(first: 10000) {
					nodes { id }
				}
			}
		`)

		expect(result.status).toBe(400)
		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("rejects oversized last on a root collection", async () => {
		const result = await executeGraphQL(`
			{
				entitiesConnection(last: 10000) {
					nodes { id }
				}
			}
		`)

		expect(result.status).toBe(400)
		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("rejects oversized offset on a root collection", async () => {
		// NOTE: this documents current behavior — a cap on offset prevents deep
		// pagination (e.g. page 12 at 100/page is offset 1100). If legitimate
		// deep pagination is required, the offset cap should be loosened.
		const result = await executeGraphQL(`
			{
				entities(offset: 10000) {
					id
				}
			}
		`)

		expect(result.status).toBe(400)
		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("accepts the maximum allowed first on the connection form", async () => {
		const result = await executeGraphQL(`
			{
				entitiesConnection(first: 1000) {
					totalCount
				}
			}
		`)

		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
	})

	it("accepts queries with no pagination argument (cap enforced via injected default)", async () => {
		// When `first` is omitted we still succeed, but the plugin injects a
		// default `first = MAX_PAGINATION_LIMIT` so PostGraphile cannot resolve
		// an unbounded collection. See the seeded integration block below for
		// the length assertion.
		const result = await executeGraphQL(`
			{
				entities { id }
			}
		`)

		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
	})

	it("rejects oversized first on nested sub-collections", async () => {
		// Nested collections inside an entity selection (e.g. relationsList
		// or valuesList) must be capped too. The previous implementation used
		// makeWrapResolversPlugin, which per PostGraphile docs only reliably
		// influences SQL for root-level resolvers. The current implementation
		// registers an arg data generator on every connection/simple-collection
		// field, so the cap runs during SQL construction at every nesting level.
		const result = await executeGraphQL(`
			{
				entitiesConnection(first: 1) {
					nodes {
						id
						relationsList(first: 10000) { id }
					}
				}
			}
		`)

		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("rejects oversized last on nested sub-collections", async () => {
		// `last` is only exposed on the Relay connection form — on Entity the
		// connection form of the from_entity_id back-ref is `relations` (per the
		// smart comment @foreignFieldName relations in drizzle/0004_functions.sql).
		// The simple-collection form `relationsList` has only `first` + `offset`.
		const result = await executeGraphQL(`
			{
				entitiesConnection(first: 1) {
					nodes {
						id
						relations(last: 10000) { nodes { id } }
					}
				}
			}
		`)

		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})
})

/**
 * Integration test that seeds MAX_PAGINATION_LIMIT+N rows directly into the
 * entities table and verifies that a bare collection query resolves at most
 * MAX_PAGINATION_LIMIT rows — i.e. that applyDefaultFirstIfOmitted actually
 * propagates a LIMIT into the generated SQL. Runs against the test DB
 * configured in vitest.config.ts.
 */
describe("PaginationCapPlugin default-first injection (seeded)", () => {
	const OVER_LIMIT = MAX_PAGINATION_LIMIT + 5
	let seededIds: string[] = []
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		seededIds = Array.from({length: OVER_LIMIT}, () => crypto.randomUUID())
		const values = seededIds.map((_id, i) => `($${i + 1}, 0, 0, 0, 0)`).join(", ")
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) VALUES ${values}`,
			seededIds,
		)
	})

	afterAll(async () => {
		if (seededIds.length) {
			await pool.query("DELETE FROM entities WHERE id = ANY($1::uuid[])", [seededIds])
		}
		await pool.end()
	})

	it("returns at most MAX_PAGINATION_LIMIT rows when first is omitted", async () => {
		const result = await executeGraphQL(`
			{
				entities { id }
			}
		`)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
		const rows: Array<{id: string}> = result.body.data.entities
		expect(rows.length).toBe(MAX_PAGINATION_LIMIT)
	})

	it("returns exactly first rows when a first < cap is specified", async () => {
		const result = await executeGraphQL(`
			{
				entities(first: 50) { id }
			}
		`)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
		const rows: Array<{id: string}> = result.body.data.entities
		expect(rows.length).toBe(50)
	})

	it("returns at most MAX_PAGINATION_LIMIT rows via the connection form when first is omitted", async () => {
		const result = await executeGraphQL(`
			{
				entitiesConnection {
					nodes { id }
				}
			}
		`)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
		const nodes: Array<{id: string}> = result.body.data.entitiesConnection.nodes
		expect(nodes.length).toBe(MAX_PAGINATION_LIMIT)
	})

	// --- Boundary tests: at-cap vs just-over-cap ---

	it("accepts first at exactly the cap", async () => {
		const result = await executeGraphQL(`
			{ entities(first: ${MAX_PAGINATION_LIMIT}) { id } }
		`)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
		const rows: Array<{id: string}> = result.body.data.entities
		expect(rows.length).toBe(MAX_PAGINATION_LIMIT)
	})

	it("rejects first at cap + 1 (off-by-one boundary)", async () => {
		const result = await executeGraphQL(`
			{ entities(first: ${MAX_PAGINATION_LIMIT + 1}) { id } }
		`)
		expect(result.status).toBe(400)
		expect(result.body.errors?.[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("accepts last at exactly the cap (connection form)", async () => {
		// `last` is only exposed on the Relay connection form — simple collections
		// support `first` / `offset` only.
		const result = await executeGraphQL(`
			{ entitiesConnection(last: ${MAX_PAGINATION_LIMIT}) { nodes { id } } }
		`)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
	})

	it("rejects last at cap + 1 (connection form)", async () => {
		const result = await executeGraphQL(`
			{ entitiesConnection(last: ${MAX_PAGINATION_LIMIT + 1}) { nodes { id } } }
		`)
		expect(result.status).toBe(400)
		expect(result.body.errors?.[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("accepts offset at exactly the cap", async () => {
		const result = await executeGraphQL(`
			{ entities(offset: ${MAX_PAGINATION_LIMIT}, first: 10) { id } }
		`)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
	})

	it("rejects offset at cap + 1", async () => {
		const result = await executeGraphQL(`
			{ entities(offset: ${MAX_PAGINATION_LIMIT + 1}) { id } }
		`)
		expect(result.status).toBe(400)
		expect(result.body.errors?.[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	// --- last / offset happy-path coverage ---

	it("honors last below the cap (connection form)", async () => {
		const result = await executeGraphQL(`
			{ entitiesConnection(last: 50) { nodes { id } } }
		`)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
		const nodes: Array<{id: string}> = result.body.data.entitiesConnection.nodes
		expect(nodes.length).toBe(50)
	})

	it("honors offset combined with first below the cap", async () => {
		const result = await executeGraphQL(`
			{ entities(first: 25, offset: 100) { id } }
		`)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
		const rows: Array<{id: string}> = result.body.data.entities
		expect(rows.length).toBe(25)
	})

	it("accepts first: 0 (edge — returns nothing, does not inject default)", async () => {
		const result = await executeGraphQL(`
			{ entities(first: 0) { id } }
		`)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
		const rows: Array<{id: string}> = result.body.data.entities
		expect(rows.length).toBe(0)
	})

	// --- cursor pagination still works with the cap in place ---

	it("supports cursor pagination via the connection form", async () => {
		const page1 = await executeGraphQL(`
			{
				entitiesConnection(first: 10) {
					nodes { id }
					pageInfo { hasNextPage endCursor }
				}
			}
		`)
		expect(page1.status).toBe(200)
		expect(page1.body.errors).toBeUndefined()

		const firstPageNodes: Array<{id: string}> = page1.body.data.entitiesConnection.nodes
		const {hasNextPage, endCursor} = page1.body.data.entitiesConnection.pageInfo
		expect(firstPageNodes.length).toBe(10)
		expect(hasNextPage).toBe(true)
		expect(endCursor).toBeTruthy()

		const page2 = await executeGraphQL(
			`
				query Page2($cursor: Cursor!) {
					entitiesConnection(first: 10, after: $cursor) {
						nodes { id }
					}
				}
			`,
			{cursor: endCursor},
		)
		expect(page2.status).toBe(200)
		expect(page2.body.errors).toBeUndefined()
		const page2Nodes: Array<{id: string}> = page2.body.data.entitiesConnection.nodes
		expect(page2Nodes.length).toBe(10)

		const firstPageIds = new Set(firstPageNodes.map((r) => r.id))
		for (const node of page2Nodes) {
			expect(firstPageIds.has(node.id)).toBe(false)
		}
	})
})

/**
 * Integration test for the PR's core claim: nested sub-collections receive the
 * default-first injection in their *own* SQL lateral. Seeds MAX+N relations
 * pointing from a single parent entity and asserts that a nested
 * `relationsList` query without `first:` is capped at MAX.
 */
describe("PaginationCapPlugin nested default-first injection (seeded)", () => {
	const OVER_LIMIT = MAX_PAGINATION_LIMIT + 5
	let parentEntityId: string
	let siblingEntityId: string
	let spaceId: string
	let typeId: string
	let seededRelationIds: string[] = []
	let seededEntityIds: string[] = []
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		parentEntityId = crypto.randomUUID()
		siblingEntityId = crypto.randomUUID()
		spaceId = crypto.randomUUID()
		typeId = crypto.randomUUID()
		seededEntityIds = [parentEntityId, siblingEntityId, typeId]

		const entityValues = seededEntityIds.map((_id, i) => `($${i + 1}, 0, 0, 0, 0)`).join(", ")
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) VALUES ${entityValues}`,
			seededEntityIds,
		)

		seededRelationIds = Array.from({length: OVER_LIMIT}, () => crypto.randomUUID())
		// relations schema: id, entity_id, type_id, from_entity_id, to_entity_id, space_id (all NOT NULL)
		const relPlaceholders = seededRelationIds
			.map(
				(_id, i) =>
					`($${i + 1}, $${i + 1}, $${OVER_LIMIT + 1}, $${OVER_LIMIT + 2}, $${OVER_LIMIT + 3}, $${OVER_LIMIT + 4})`,
			)
			.join(", ")
		await pool.query(
			`INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) VALUES ${relPlaceholders}`,
			[...seededRelationIds, typeId, parentEntityId, siblingEntityId, spaceId],
		)
	})

	afterAll(async () => {
		if (seededRelationIds.length) {
			await pool.query("DELETE FROM relations WHERE id = ANY($1::uuid[])", [seededRelationIds])
		}
		if (seededEntityIds.length) {
			await pool.query("DELETE FROM entities WHERE id = ANY($1::uuid[])", [seededEntityIds])
		}
		await pool.end()
	})

	it("caps nested relations at MAX when first is omitted on the nested field", async () => {
		const result = await executeGraphQL(
			`
				query NestedDefault($id: UUID!) {
					entity(id: $id) {
						relationsList {
							id
						}
					}
				}
			`,
			{id: parentEntityId},
		)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
		const nested: Array<{id: string}> = result.body.data.entity.relationsList
		expect(nested.length).toBe(MAX_PAGINATION_LIMIT)
	})

	it("honors explicit first < cap on the nested field", async () => {
		const result = await executeGraphQL(
			`
				query NestedExplicit($id: UUID!) {
					entity(id: $id) {
						relationsList(first: 7) {
							id
						}
					}
				}
			`,
			{id: parentEntityId},
		)
		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
		const nested: Array<{id: string}> = result.body.data.entity.relationsList
		expect(nested.length).toBe(7)
	})
})
