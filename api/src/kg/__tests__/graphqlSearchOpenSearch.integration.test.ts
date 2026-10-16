import {Pool} from "pg"
import {afterAll, afterEach, beforeAll, describe, expect, it, vi} from "vitest"
import type {SearchClient, SearchQuery, SearchResponse} from "../../services/search"
import {log} from "../../services/telemetry"
import {setGraphqlSearchClient} from "../graphqlSearchOpenSearchPlugin"
import {graphqlServer} from "../postgraphile"

/**
 * End to end: real Yoga + PostGraphile + migrated Postgres (0110), with a fake OpenSearch client.
 * Needs a real database, like the other kg integration tests.
 */

// Every seeded entity is typed (the type entities too: PERSON is its own type). Other suites in this
// directory run in parallel on the same database and assert that `typeIds: {isNull: false}` only
// returns typed entities.
const SPACE = "00000000-1100-4110-8110-0000000000a1"
const TYPE_PERSON = "00000000-1100-4110-8110-0000000000b1"
const TYPE_OTHER = "00000000-1100-4110-8110-0000000000b2"
const NAME_PROPERTY = "a126ca53-0c8e-48d5-b888-82c734c38935"
const TYPES_RELATION_ID = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"

// Trigram order for the query "ada lovelace" would be E1 (exact) then E2; OpenSearch order below
// is deliberately the reverse so the two paths are distinguishable.
const E1 = "00000000-1100-4110-8110-000000000001" // "ada lovelace", Person
const E2 = "00000000-1100-4110-8110-000000000002" // "ada lovelace countess", other type
const E3 = "00000000-1100-4110-8110-000000000003" // "unrelated text", Person
const ALL = [E1, E2, E3]
const undash = (u: string) => u.replaceAll("-", "")

async function gql(query: string, variables?: Record<string, unknown>) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query, variables}),
		}),
		{},
	)
	return (await response.json()) as {errors?: Array<{message: string}>; data?: Record<string, any>}
}

function fake(impl: (q: SearchQuery) => Promise<string[]>): SearchClient & {calls: SearchQuery[]} {
	const calls: SearchQuery[] = []
	return {
		calls,
		async search(q) {
			calls.push(q)
			const ids = await impl(q)
			return {
				results: ids.map((entityId) => ({entityId})),
				total: ids.length,
				tookMs: 1,
			} as unknown as SearchResponse
		},
		async healthCheck() {
			return true
		},
	}
}

describe("GraphQL search backed by OpenSearch ids", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		// One transaction, so a concurrent suite never sees half the fixture.
		const tx = await pool.connect()
		await tx.query("BEGIN")
		await tx.query(
			`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xsearchids') ON CONFLICT DO NOTHING`,
			[SPACE],
		)
		const names: Record<string, string> = {
			[E1]: "ada lovelace",
			[E2]: "ada lovelace countess",
			[E3]: "unrelated text",
		}
		for (const [i, id] of ALL.entries()) {
			await tx.query(
				`INSERT INTO "values" (id, property_id, entity_id, space_id, "text") VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING`,
				[`gqlsearchids-v${i}`, NAME_PROPERTY, id, SPACE, names[id]],
			)
		}
		const typed: Array<[string, string]> = [
			[E1, TYPE_PERSON],
			[E2, TYPE_OTHER],
			[E3, TYPE_PERSON],
			[TYPE_PERSON, TYPE_PERSON],
			[TYPE_OTHER, TYPE_PERSON],
		]
		for (const [i, [id, typeId]] of typed.entries()) {
			await tx.query(
				`INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id)
				 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING`,
				[
					`00000000-1100-4110-7110-00000000000${i + 1}`,
					`00000000-1100-4110-6110-00000000000${i + 1}`,
					TYPES_RELATION_ID,
					id,
					typeId,
					SPACE,
				],
			)
		}
		// Entities last, in one statement: no moment at which a seeded entity exists untyped.
		await tx.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT unnest($1::uuid[]), '0', '0', '0', '0' ON CONFLICT DO NOTHING`,
			[[...ALL, TYPE_PERSON, TYPE_OTHER]],
		)
		await tx.query("COMMIT")
		tx.release()
	})

	afterAll(async () => {
		setGraphqlSearchClient(null)
		// The rows stay. entitySpaceFilterPlugin.test.ts discovers whatever space and type it finds in
		// the shared database, and deleting them while it runs makes it fail. The seed is idempotent.
		await pool.end()
	})

	afterEach(() => {
		setGraphqlSearchClient(null)
		vi.restoreAllMocks()
		delete process.env.GRAPHQL_SEARCH_OPENSEARCH
	})

	const osOrder = [E3, E2, E1]

	it("returns entities in OpenSearch order, with nested selections", async () => {
		const c = fake(async () => osOrder)
		setGraphqlSearchClient(c)
		const res = await gql(`{ search(query: "ada lovelace", first: 10) { id name relationsList { id } } }`)
		expect(res.errors).toBeUndefined()
		expect(res.data?.search.map((e: {id: string}) => e.id)).toEqual(osOrder.map(undash))
		expect(res.data?.search[0].name).toBe("unrelated text")
		expect(c.calls[0]).toMatchObject({query: "ada lovelace", scope: "GLOBAL", limit: 30})
	})

	it("keeps first and offset working on the supplied ids", async () => {
		setGraphqlSearchClient(fake(async () => osOrder))
		const res = await gql(`{ search(query: "ada lovelace", first: 1, offset: 1) { id } }`)
		expect(res.errors).toBeUndefined()
		expect(res.data?.search.map((e: {id: string}) => e.id)).toEqual([undash(E2)])
	})

	it("keeps the connection filter, with aliases and variables", async () => {
		const c = fake(async () => osOrder)
		setGraphqlSearchClient(c)
		const res = await gql(
			`query Q($q: String!, $t: [UUID!]) { people: searchConnection(query: $q, first: 10, filter: {typeIds: {in: $t}}) { totalCount nodes { id } } }`,
			{q: "ada lovelace", t: [TYPE_PERSON]},
		)
		expect(res.errors).toBeUndefined()
		expect(res.data?.people.nodes.map((e: {id: string}) => e.id)).toEqual([undash(E3), undash(E1)])
		expect(res.data?.people.totalCount).toBe(2)
		expect(c.calls[0]).toMatchObject({type_ids: [undash(TYPE_PERSON)], limit: 100})
	})

	it("falls back to the trigram body when OpenSearch fails", async () => {
		vi.spyOn(log, "warn").mockImplementation(() => {})
		setGraphqlSearchClient(
			fake(async () => {
				throw new Error("down")
			}),
		)
		const res = await gql(`{ search(query: "ada lovelace", first: 10) { id } }`)
		expect(res.errors).toBeUndefined()
		expect(res.data?.search.map((e: {id: string}) => e.id).slice(0, 2)).toEqual([undash(E1), undash(E2)])
	})

	it("does not call OpenSearch when disabled with GRAPHQL_SEARCH_OPENSEARCH=0", async () => {
		process.env.GRAPHQL_SEARCH_OPENSEARCH = "0"
		const c = fake(async () => osOrder)
		setGraphqlSearchClient(c)
		const res = await gql(`{ search(query: "ada lovelace", first: 10) { id } }`)
		expect(res.errors).toBeUndefined()
		expect(c.calls).toHaveLength(0)
		expect(res.data?.search[0].id).toBe(undash(E1))
	})

	it("passes an explicit ids argument through untouched", async () => {
		const c = fake(async () => osOrder)
		setGraphqlSearchClient(c)
		const res = await gql(`{ search(query: "ada lovelace", ids: ["${E2}"]) { id } }`)
		expect(res.errors).toBeUndefined()
		expect(res.data?.search.map((e: {id: string}) => e.id)).toEqual([undash(E2)])
		expect(c.calls).toHaveLength(0)
	})
})
