import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {DEFAULT_PAGINATION_LIMITS_BY_DEPTH, MAX_PAGINATION_LIMIT} from "../paginationCapPlugin"
import {graphqlServer} from "../postgraphile"

/**
 * Proves the depth-tapered pagination defaults against a REAL PostGraphile
 * schema, by counting rows that actually come back.
 *
 * A unit test of `defaultLimitForDepth` proves arithmetic. It cannot prove the
 * part that was genuinely uncertain: that "depth" as this plugin computes it —
 * by walking `parentQueryBuilder` and counting only the builders it has tagged
 * — matches depth as a person reading the query would count it. PostGraphile
 * also builds child query builders for *singular* relations (`toEntity { ... }`),
 * which multiply nothing, and counting those would tighten a shallow query as
 * though it were deep.
 *
 * So each case below seeds more rows than the limit under test and asserts the
 * exact count returned.
 */

const SPACE = "00000000-beef-4000-8000-00000000f001"
const ROOT_TYPE = "00000000-beef-4000-8000-00000000d001"
const ROOT = "00000000-beef-4000-8000-000000000001"
const RELATION_TYPE = "00000000-beef-4000-8000-00000000c001"

/** Comfortably more than any default in the taper, so truncation is visible. */
const TARGET_COUNT = 140

const targetId = (n: number) => `00000000-beef-4000-8000-${String(n + 1000).padStart(12, "0")}`
const relationId = (n: number) => `00000000-beef-4000-8000-${String(n + 5000).padStart(12, "0")}`

async function executeGraphQL(query: string) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query}),
		}),
		{},
	)
	return response.json() as Promise<{errors?: Array<{message: string}>; data?: Record<string, unknown>}>
}

describe("depth-tapered pagination defaults", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		const allEntities = [
			ROOT,
			ROOT_TYPE,
			RELATION_TYPE,
			...Array.from({length: TARGET_COUNT}, (_, i) => targetId(i)),
		]

		await pool.query(`DELETE FROM relations WHERE space_id = $1`, [SPACE])
		await pool.query(`DELETE FROM values WHERE space_id = $1`, [SPACE])
		await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [allEntities])
		await pool.query(
			`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xdepth') ON CONFLICT DO NOTHING`,
			[SPACE],
		)
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT x, 1700000000, 100, 1700000000, 100 FROM unnest($1::uuid[]) x ON CONFLICT DO NOTHING`,
			[allEntities],
		)

		// Every target entity carries more values than any nested default, so a
		// values list two levels down shows its own limit rather than the row
		// count.
		await pool.query(
			`INSERT INTO values (id, entity_id, property_id, space_id, text)
			 SELECT gen_random_uuid(), t.x, $3, $2, 'v' || v.i
			 FROM unnest($1::uuid[]) t(x), generate_series(1, ${TARGET_COUNT}) v(i)`,
			[[ROOT, ...Array.from({length: TARGET_COUNT}, (_, i) => targetId(i))], SPACE, ROOT_TYPE],
		)

		// ROOT -> many targets, and every target -> many targets, so depth 1, 2
		// and 3 all have more rows available than their limit.
		await pool.query(
			`INSERT INTO relations (id, space_id, type_id, from_entity_id, to_entity_id, entity_id, position, verified)
			 SELECT relid, $4, $3, src, tgt, relid, 'a' || ord, false
			 FROM (
			   SELECT unnest($1::uuid[]) AS relid, unnest($2::uuid[]) AS src,
			          unnest($5::uuid[]) AS tgt, generate_series(1, array_length($1::uuid[], 1)) AS ord
			 ) s`,
			[
				// ROOT -> target[i] for the first half; target[0] -> target[i] for the rest.
				[
					...Array.from({length: TARGET_COUNT}, (_, i) => relationId(i)),
					...Array.from({length: TARGET_COUNT}, (_, i) => relationId(i + TARGET_COUNT)),
				],
				[
					...Array.from({length: TARGET_COUNT}, () => ROOT),
					...Array.from({length: TARGET_COUNT}, () => targetId(0)),
				],
				RELATION_TYPE,
				SPACE,
				[
					...Array.from({length: TARGET_COUNT}, (_, i) => targetId(i)),
					...Array.from({length: TARGET_COUNT}, (_, i) => targetId(i)),
				],
			],
		)
		await pool.query(`UPDATE entities SET id = id WHERE id = $1`, [ROOT])
	})

	afterAll(async () => {
		// Entities as well as their relations and values: this suite shares one
		// database with every other integration test, and leaving 140 typeless
		// entities behind changes the answer to other suites' aggregate queries.
		const allEntities = [
			ROOT,
			ROOT_TYPE,
			RELATION_TYPE,
			...Array.from({length: TARGET_COUNT}, (_, i) => targetId(i)),
		]
		await pool.query(`DELETE FROM relations WHERE space_id = $1`, [SPACE])
		await pool.query(`DELETE FROM values WHERE space_id = $1`, [SPACE])
		await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [allEntities])
		await pool.query(`DELETE FROM spaces WHERE id = $1`, [SPACE])
		await pool.end()
	})

	const [depth0, depth1, depth2] = DEFAULT_PAGINATION_LIMITS_BY_DEPTH as [number, number, number]

	it("seeded more rows than any default, so a truncation is real", () => {
		expect(TARGET_COUNT).toBeGreaterThan(Math.max(depth0, depth1, depth2))
	})

	it("gives a root collection the depth-0 default", async () => {
		const r = await executeGraphQL(`{ entities(filter: {id: {is: "${ROOT}"}}) { id } }`)
		expect(r.errors).toBeUndefined()
		// One matching row; the point is only that the root is not truncated
		// below what exists.
		expect((r.data?.entities as unknown[]).length).toBe(1)
	})

	it("gives a collection inside a root collection the depth-1 default", async () => {
		const r = await executeGraphQL(`{ entities(filter: {id: {is: "${ROOT}"}}) { id relationsList { id } } }`)
		expect(r.errors).toBeUndefined()
		const entities = r.data?.entities as Array<{relationsList: unknown[]}>
		expect(entities[0]?.relationsList.length).toBe(depth1)
	})

	it("gives a collection two levels in the depth-2 default", async () => {
		// entities -> relationsList -> toEntity -> valuesList. `toEntity` is a
		// SINGULAR relation: it must not count as a level, or this list would get
		// the depth-3 limit instead.
		const r = await executeGraphQL(
			`{ entities(filter: {id: {is: "${ROOT}"}}) {
				relationsList { toEntity { valuesList { id } } }
			} }`,
		)
		expect(r.errors).toBeUndefined()
		const entities = r.data?.entities as Array<{
			relationsList: Array<{toEntity: {valuesList: unknown[]} | null}>
		}>
		const lists = entities[0]?.relationsList.map((rel) => rel.toEntity?.valuesList.length ?? 0) ?? []
		expect(lists.length).toBe(depth1)
		for (const length of lists) {
			expect(length).toBe(depth2)
		}
	})

	it("does not count a singular relation as a level of nesting", async () => {
		// The same list, reached without the singular hop, must get the same
		// limit. If `toEntity` were being counted, these two would differ.
		const withHop = await executeGraphQL(
			`{ entities(filter: {id: {is: "${ROOT}"}}) { relationsList { toEntity { valuesList { id } } } } }`,
		)
		const withoutHop = await executeGraphQL(
			`{ entities(filter: {id: {is: "${targetId(0)}"}}) { valuesList { id } } }`,
		)
		const nested = (withHop.data?.entities as Array<{relationsList: Array<{toEntity: {valuesList: unknown[]}}>}>)[0]
			?.relationsList[0]?.toEntity.valuesList.length
		const direct = (withoutHop.data?.entities as Array<{valuesList: unknown[]}>)[0]?.valuesList.length
		expect(nested).toBe(depth2)
		expect(direct).toBe(depth1)
	})

	it("still honours an explicit `first` at any depth", async () => {
		const explicit = Math.max(depth1, depth2) + 30
		const r = await executeGraphQL(
			`{ entities(filter: {id: {is: "${ROOT}"}}) {
				relationsList(first: ${explicit}) { toEntity { valuesList(first: ${explicit}) { id } } }
			} }`,
		)
		expect(r.errors).toBeUndefined()
		const entities = r.data?.entities as Array<{
			relationsList: Array<{toEntity: {valuesList: unknown[]} | null}>
		}>
		expect(entities[0]?.relationsList.length).toBe(explicit)
		expect(entities[0]?.relationsList[0]?.toEntity?.valuesList.length).toBe(explicit)
	})

	it("still rejects an explicit `first` over the cap, at depth", async () => {
		const r = await executeGraphQL(
			`{ entities(filter: {id: {is: "${ROOT}"}}) {
				relationsList(first: ${MAX_PAGINATION_LIMIT + 1}) { id }
			} }`,
		)
		expect(r.errors?.[0]?.message).toContain(`cannot exceed ${MAX_PAGINATION_LIMIT}`)
	})

	it("bounds the product that the old flat default did not", async () => {
		// The shape that motivated the change: an explicit `first` at the root
		// and nothing below it. Under a flat default of 100 this materialises
		// 100 x 100 rows; under the taper it is depth1 x depth2.
		const r = await executeGraphQL(
			`{ entities(filter: {id: {is: "${ROOT}"}}) {
				relationsList { toEntity { valuesList { id } } }
			} }`,
		)
		const entities = r.data?.entities as Array<{
			relationsList: Array<{toEntity: {valuesList: unknown[]} | null}>
		}>
		const total = (entities[0]?.relationsList ?? []).reduce(
			(sum, rel) => sum + (rel.toEntity?.valuesList.length ?? 0),
			0,
		)
		expect(total).toBe(depth1 * depth2)
		expect(total).toBeLessThan(100 * 100)
	})
})
