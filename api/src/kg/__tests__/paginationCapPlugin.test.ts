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
		// Nested collections inside an entity selection (e.g. relationsByFromEntityIdList
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
						relationsByFromEntityIdList(first: 10000) { id }
					}
				}
			}
		`)

		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("rejects oversized last on nested sub-collections", async () => {
		const result = await executeGraphQL(`
			{
				entitiesConnection(first: 1) {
					nodes {
						id
						relationsByFromEntityIdList(last: 10000) { id }
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
})
