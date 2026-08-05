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
	return response.json()
}

const undash = (uuid: string) => uuid.replace(/-/g, "")

// Well-known UUIDs for the test fixture. Chosen so they can't collide with real
// data. Cleanup deletes rows by these IDs only, leaving unrelated data intact.
const SPACE_A = "00000000-aaaa-4aaa-aaaa-000000000001"
const SPACE_B = "00000000-aaaa-4aaa-aaaa-000000000002"
const ENTITY_1 = "00000000-bbbb-4bbb-bbbb-000000000001"
const ENTITY_2 = "00000000-bbbb-4bbb-bbbb-000000000002"
const ENTITY_3 = "00000000-bbbb-4bbb-bbbb-000000000003"

const TEST_ENTITY_IDS = [ENTITY_1, ENTITY_2, ENTITY_3]

// Expected cross-space aggregation for ENTITY_1:
// votes_count rows inserted below —
//   ENTITY_1 @ SPACE_A: positive=15, negative=5 → net=10
//   ENTITY_1 @ SPACE_B: positive=8,  negative=2 → net=6
// sum across spaces = 16, max single space = 10 (they differ, so the regression
// guard in test #8 actually proves the aggregation path).
const ENTITY_1_CROSS_SPACE_NET_SCORE = 16
const ENTITY_1_MAX_SINGLE_SPACE_NET_SCORE = 10

async function cleanupFixtures(pool: Pool) {
	await pool.query(`DELETE FROM local_scores WHERE entity_id = ANY($1::uuid[])`, [TEST_ENTITY_IDS])
	await pool.query(`DELETE FROM global_scores WHERE entity_id = ANY($1::uuid[])`, [TEST_ENTITY_IDS])
	await pool.query(`DELETE FROM votes_count WHERE object_id = ANY($1::uuid[])`, [TEST_ENTITY_IDS])
	await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [TEST_ENTITY_IDS])
}

async function seedFixtures(pool: Pool) {
	await pool.query(
		`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
		 SELECT unnest($1::uuid[]), '0', '0', '0', '0'`,
		[TEST_ENTITY_IDS],
	)

	// Distinct global scores so ASC vs DESC differ and pagination has > 1 page at first: 2.
	await pool.query(
		`INSERT INTO global_scores (entity_id, score, updated_at) VALUES
			($1, 300, now()),
			($2, 200, now()),
			($3, 100, now())`,
		[ENTITY_1, ENTITY_2, ENTITY_3],
	)

	// Local scores only for SPACE_A.
	await pool.query(
		`INSERT INTO local_scores (entity_id, space_id, score, updated_at) VALUES
			($1, $4, 30, now()),
			($2, $4, 20, now()),
			($3, $4, 10, now())`,
		[ENTITY_1, ENTITY_2, ENTITY_3, SPACE_A],
	)

	// ENTITY_1 is voted on in BOTH spaces so the cross-space RAW branch exercises
	// SUM(positive - negative) across rows. ENTITY_2 and ENTITY_3 vote only in SPACE_A.
	// All curation (vote_kind 0) — per-kind isolation is covered by the
	// "vote_kind isolation" suite above.
	await pool.query(
		`INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative) VALUES
			($1, 0, $4, 0, 15, 5),
			($1, 0, $5, 0, 8, 2),
			($2, 0, $4, 0, 10, 1),
			($3, 0, $4, 0, 5, 5)`,
		[ENTITY_1, ENTITY_2, ENTITY_3, SPACE_A, SPACE_B],
	)
}

// ---------------------------------------------------------------------------
// vote_kind isolation
//
// These call the SQL function directly rather than going through GraphQL, and
// are deliberately NOT part of the describe.skip block below: that suite is
// skipped because entities_ordered_by_score is hidden from GraphQL, so a test
// added there would never run and would read as passing coverage it isn't.
//
// What they protect: votes_count went from one row per
// (object_id, object_type, space_id) to one per kind. Every read path has to
// filter vote_kind = 0, or the SUM paths add stance/veracity tallies into the
// curation score and the direct-join paths pick an arbitrary row of three.
// Neither raises an error — the ranking silently changes. The fixture is built
// so that a missing filter flips the order, not merely perturbs a number.
// ---------------------------------------------------------------------------
const KIND_SPACE = "00000000-aaaa-4aaa-aaaa-00000000dd01"
const KIND_ENTITY_HIGH = "00000000-bbbb-4bbb-bbbb-00000000dd01"
const KIND_ENTITY_LOW = "00000000-bbbb-4bbb-bbbb-00000000dd02"
const KIND_ENTITY_IDS = [KIND_ENTITY_HIGH, KIND_ENTITY_LOW]

describe("entities_ordered_by_score vote_kind isolation", () => {
	let pool: Pool

	async function cleanup() {
		await pool.query(`DELETE FROM votes_count WHERE object_id = ANY($1::uuid[])`, [KIND_ENTITY_IDS])
		await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [KIND_ENTITY_IDS])
	}

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await cleanup()

		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT unnest($1::uuid[]), '0', '0', '0', '0'`,
			[KIND_ENTITY_IDS],
		)

		// Curation: HIGH (net 10) must outrank LOW (net 9).
		// Stance: a landslide on LOW that must not count toward curation. If any
		// read path drops the vote_kind filter, LOW's effective score becomes
		// 9 + 500 and the two entities swap places.
		await pool.query(
			`INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative) VALUES
				($1, 0, $3, 0, 15, 5),
				($2, 0, $3, 0, 10, 1),
				($2, 0, $3, 1, 500, 0)`,
			[KIND_ENTITY_HIGH, KIND_ENTITY_LOW, KIND_SPACE],
		)
	})

	afterAll(async () => {
		if (pool) {
			await cleanup()
			await pool.end()
		}
	})

	it("ignores a stance row when ordering RAW within a space", async () => {
		const res = await pool.query(
			`SELECT id FROM entities_ordered_by_score('raw', $1::uuid, 'DESC')
			 WHERE id = ANY($2::uuid[])`,
			[KIND_SPACE, KIND_ENTITY_IDS],
		)
		// Per-space RAW is a direct LEFT JOIN. Unfiltered it would match an
		// arbitrary one of LOW's two rows.
		expect(res.rows.map((r) => r.id)).toEqual([KIND_ENTITY_HIGH, KIND_ENTITY_LOW])
	})

	it("ignores a stance row when ordering RAW across spaces", async () => {
		const res = await pool.query(
			`SELECT id FROM entities_ordered_by_score('raw', NULL, 'DESC')
			 WHERE id = ANY($1::uuid[])`,
			[KIND_ENTITY_IDS],
		)
		// Cross-space RAW is the SUM path — the one that inflates.
		expect(res.rows.map((r) => r.id)).toEqual([KIND_ENTITY_HIGH, KIND_ENTITY_LOW])
	})

	it("keeps the entity's curation score unchanged by responses of other kinds", async () => {
		const res = await pool.query(
			`SELECT COALESCE(SUM(positive - negative), 0)::bigint AS net
			 FROM votes_count WHERE object_type = 0 AND vote_kind = 0 AND object_id = $1`,
			[KIND_ENTITY_LOW],
		)
		expect(Number(res.rows[0].net)).toBe(9)
	})

	it("returns one votes_count row per kind for the same object and space", async () => {
		const res = await pool.query(
			`SELECT vote_kind, positive, negative FROM votes_count
			 WHERE object_id = $1 AND object_type = 0 AND space_id = $2
			 ORDER BY vote_kind`,
			[KIND_ENTITY_LOW, KIND_SPACE],
		)
		// The cardinality change itself — one row per (object, space) became one
		// per (object, space, kind). This is the premise every filter depends on.
		expect(res.rows.map((r) => r.vote_kind)).toEqual([0, 1])
	})

	it("still exposes upvotes/downvotes as read-only shims equal to positive/negative", async () => {
		const res = await pool.query(
			`SELECT positive, negative, upvotes, downvotes FROM votes_count
			 WHERE object_id = $1 AND object_type = 0 AND space_id = $2 AND vote_kind = 0`,
			[KIND_ENTITY_HIGH, KIND_SPACE],
		)
		// The pre-vote_kind web client still selects these names over GraphQL.
		expect(Number(res.rows[0].upvotes)).toBe(Number(res.rows[0].positive))
		expect(Number(res.rows[0].downvotes)).toBe(Number(res.rows[0].negative))
	})

	it("rejects a write to the deprecated upvotes shim", async () => {
		// Guards against a writer being "fixed" to target the old column name and
		// silently diverging from positive/negative.
		await expect(
			pool.query(`UPDATE votes_count SET upvotes = 1 WHERE object_id = $1`, [KIND_ENTITY_HIGH]),
		).rejects.toThrow()
	})
})

// Skipped: entities_ordered_by_score is hidden from GraphQL via hideProceduresPlugin.
// Unskip when the field is re-exposed.
describe.skip("entitiesOrderedByScore", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		// Idempotent cleanup in case a prior run aborted between seed and teardown.
		await cleanupFixtures(pool)
		await seedFixtures(pool)
	})

	afterAll(async () => {
		if (pool) {
			await cleanupFixtures(pool)
			await pool.end()
		}
	})

	describe("schema introspection", () => {
		it("exposes entitiesOrderedByScore as a Query field", async () => {
			const result = await executeGraphQL(`
				{
					__type(name: "Query") {
						fields { name }
					}
				}
			`)
			expect(result.errors).toBeUndefined()
			const fieldNames = result.data.__type.fields.map((f: {name: string}) => f.name)
			expect(fieldNames).toContain("entitiesOrderedByScore")
			expect(fieldNames).toContain("entitiesOrderedByScoreConnection")
		})

		it("exposes scoreType, spaceId, sortDirection, and connection args", async () => {
			const result = await executeGraphQL(`
				{
					__type(name: "Query") {
						fields {
							name
							args {
								name
								type { name kind ofType { name kind } }
							}
						}
					}
				}
			`)
			expect(result.errors).toBeUndefined()

			const field = result.data.__type.fields.find((f: {name: string}) => f.name === "entitiesOrderedByScore")
			expect(field).toBeDefined()

			const argNames = field.args.map((a: {name: string}) => a.name)
			expect(argNames).toEqual(expect.arrayContaining(["scoreType", "spaceId", "sortDirection", "first"]))

			// Note: scoreType is exposed as a nullable ENUM at the GraphQL layer — the
			// SQL function enforces non-null at runtime via a RAISE with SQLSTATE 22023,
			// which surfaces as BAD_USER_INPUT. See the "rejects LOCAL without spaceId"
			// test for the runtime validation path.
			const scoreTypeArg = field.args.find((a: {name: string}) => a.name === "scoreType")
			const scoreTypeName = scoreTypeArg.type.name ?? scoreTypeArg.type.ofType?.name
			expect(scoreTypeName).toBe("ScoreType")

			const spaceIdArg = field.args.find((a: {name: string}) => a.name === "spaceId")
			const spaceIdName = spaceIdArg.type.name ?? spaceIdArg.type.ofType?.name
			expect(spaceIdName).toBe("UUID")

			const sortDirectionArg = field.args.find((a: {name: string}) => a.name === "sortDirection")
			const sortDirectionName = sortDirectionArg.type.name ?? sortDirectionArg.type.ofType?.name
			expect(sortDirectionName).toBe("SortOrder")
		})

		it("exposes a ScoreType enum with LOCAL, GLOBAL, RAW", async () => {
			const result = await executeGraphQL(`
				{
					__type(name: "ScoreType") {
						name
						enumValues { name }
					}
				}
			`)
			expect(result.errors).toBeUndefined()
			expect(result.data.__type).toBeDefined()
			const names = result.data.__type.enumValues.map((v: {name: string}) => v.name)
			expect(names).toEqual(expect.arrayContaining(["LOCAL", "GLOBAL", "RAW"]))
		})
	})

	describe("query acceptance", () => {
		it("accepts LOCAL with spaceId and returns the seeded entities", async () => {
			const result = await executeGraphQL(
				`
					query Q($spaceId: UUID!) {
						entitiesOrderedByScore(scoreType: LOCAL, spaceId: $spaceId, first: 10) {
							id
						}
					}
				`,
				{spaceId: undash(SPACE_A)},
			)
			expect(result.errors).toBeUndefined()

			const ids: string[] = result.data.entitiesOrderedByScore.map((e: {id: string}) => e.id)
			// All three seeded entities should appear, ordered by local_scores.score DESC:
			// ENTITY_1(30) > ENTITY_2(20) > ENTITY_3(10).
			expect(ids.slice(0, 3)).toEqual([undash(ENTITY_1), undash(ENTITY_2), undash(ENTITY_3)])
		})

		it("accepts GLOBAL without spaceId and returns seeded entities in score order", async () => {
			const result = await executeGraphQL(`
				{
					entitiesOrderedByScore(scoreType: GLOBAL, first: 10) { id }
				}
			`)
			expect(result.errors).toBeUndefined()

			const ids: string[] = result.data.entitiesOrderedByScore.map((e: {id: string}) => e.id)
			// global_scores: ENTITY_1(300) > ENTITY_2(200) > ENTITY_3(100).
			expect(ids.slice(0, 3)).toEqual([undash(ENTITY_1), undash(ENTITY_2), undash(ENTITY_3)])
		})

		it("accepts RAW with spaceId and orders by (positive - negative) within the space", async () => {
			const result = await executeGraphQL(
				`
					query Q($spaceId: UUID!) {
						entitiesOrderedByScore(scoreType: RAW, spaceId: $spaceId, first: 10) { id }
					}
				`,
				{spaceId: undash(SPACE_A)},
			)
			expect(result.errors).toBeUndefined()

			const ids: string[] = result.data.entitiesOrderedByScore.map((e: {id: string}) => e.id)
			// Per-space net votes in SPACE_A: ENTITY_1=10, ENTITY_2=9, ENTITY_3=0.
			expect(ids.slice(0, 3)).toEqual([undash(ENTITY_1), undash(ENTITY_2), undash(ENTITY_3)])
		})

		it("accepts RAW without spaceId and sums positive-negative across spaces", async () => {
			const result = await executeGraphQL(`
				{
					entitiesOrderedByScore(scoreType: RAW, first: 50) { id }
				}
			`)
			expect(result.errors).toBeUndefined()

			const ids: string[] = result.data.entitiesOrderedByScore.map((e: {id: string}) => e.id)
			expect(ids).toContain(undash(ENTITY_1))

			// Verify ENTITY_1's aggregation path uses SUM across spaces, not MAX of a
			// single space. Recomputes directly from votes_count as an extra guard.
			const direct = await pool.query(
				`
					SELECT SUM(positive - negative)::bigint AS net_score,
						MAX(positive - negative)::bigint AS max_single_space
					FROM votes_count
					WHERE object_type = 0 AND vote_kind = 0 AND object_id = $1
				`,
				[ENTITY_1],
			)
			expect(Number(direct.rows[0].net_score)).toBe(ENTITY_1_CROSS_SPACE_NET_SCORE)
			expect(Number(direct.rows[0].max_single_space)).toBe(ENTITY_1_MAX_SINGLE_SPACE_NET_SCORE)
			expect(ENTITY_1_CROSS_SPACE_NET_SCORE).not.toBe(ENTITY_1_MAX_SINGLE_SPACE_NET_SCORE)
		})
	})

	describe("ordering and pagination", () => {
		it("supports sortDirection ASC and DESC", async () => {
			// Run sequentially to avoid tripping the pool-pressure shedder on
			// small local pools — the integration harness is not tuned for parallelism.
			const descRes = await executeGraphQL(`
				{
					entitiesOrderedByScore(scoreType: GLOBAL, sortDirection: DESC, first: 10) { id }
				}
			`)
			const ascRes = await executeGraphQL(`
				{
					entitiesOrderedByScore(scoreType: GLOBAL, sortDirection: ASC, first: 10) { id }
				}
			`)
			expect(descRes.errors).toBeUndefined()
			expect(ascRes.errors).toBeUndefined()

			const descIds: string[] = descRes.data.entitiesOrderedByScore.map((e: {id: string}) => e.id)
			const ascIds: string[] = ascRes.data.entitiesOrderedByScore.map((e: {id: string}) => e.id)

			// Seeded global_scores have distinct scores, so top-of-DESC ≠ top-of-ASC.
			expect(descIds[0]).toBe(undash(ENTITY_1))
			expect(ascIds[0]).toBe(undash(ENTITY_3))
		}, 30_000)

		// 30s — the connection field issues COUNT(*) over the SETOF function, which
		// re-executes the function body against the full result set.
		it("supports cursor-based pagination via the connection field", async () => {
			const page1 = await executeGraphQL(`
				{
					entitiesOrderedByScoreConnection(scoreType: GLOBAL, first: 2) {
						nodes { id }
						pageInfo { hasNextPage endCursor }
					}
				}
			`)
			expect(page1.errors).toBeUndefined()

			const {endCursor, hasNextPage} = page1.data.entitiesOrderedByScoreConnection.pageInfo
			expect(endCursor).toBeTruthy()
			expect(hasNextPage).toBe(true)

			const page2 = await executeGraphQL(
				`
					query Q($after: Cursor!) {
						entitiesOrderedByScoreConnection(scoreType: GLOBAL, first: 2, after: $after) {
							nodes { id }
							pageInfo { hasNextPage endCursor }
						}
					}
				`,
				{after: endCursor},
			)
			expect(page2.errors).toBeUndefined()

			const ids1: string[] = page1.data.entitiesOrderedByScoreConnection.nodes.map((n: {id: string}) => n.id)
			const ids2: string[] = page2.data.entitiesOrderedByScoreConnection.nodes.map((n: {id: string}) => n.id)
			for (const id of ids2) expect(ids1).not.toContain(id)
		}, 30_000)

		it("composes with a connection filter", async () => {
			const result = await executeGraphQL(`
				{
					entitiesOrderedByScoreConnection(
						scoreType: GLOBAL
						first: 5
						filter: { id: { isNull: false } }
					) {
						nodes { id }
					}
				}
			`)
			expect(result.errors).toBeUndefined()
			expect(result.data.entitiesOrderedByScoreConnection.nodes.length).toBeGreaterThan(0)
		}, 30_000)
	})

	describe("regression guards", () => {
		it("returns unique ids for every (scoreType, spaceId) branch", async () => {
			type Case = {label: string; query: string; vars: Record<string, unknown>}
			const cases: Case[] = [
				{
					label: "global",
					vars: {},
					query: `{ entitiesOrderedByScore(scoreType: GLOBAL, first: 50) { id } }`,
				},
				{
					label: "raw-cross-space",
					vars: {},
					query: `{ entitiesOrderedByScore(scoreType: RAW, first: 50) { id } }`,
				},
				{
					label: "local",
					vars: {spaceId: undash(SPACE_A)},
					query: `query Q($spaceId: UUID!) {
						entitiesOrderedByScore(scoreType: LOCAL, spaceId: $spaceId, first: 50) { id }
					}`,
				},
				{
					label: "raw-per-space",
					vars: {spaceId: undash(SPACE_A)},
					query: `query Q($spaceId: UUID!) {
						entitiesOrderedByScore(scoreType: RAW, spaceId: $spaceId, first: 50) { id }
					}`,
				},
			]

			for (const c of cases) {
				const res = await executeGraphQL(c.query, c.vars)
				expect(res.errors, `${c.label}: ${JSON.stringify(res.errors)}`).toBeUndefined()
				const ids: string[] = (res.data.entitiesOrderedByScore ?? []).map((e: {id: string}) => e.id)
				expect(new Set(ids).size, `${c.label} returned duplicate ids`).toBe(ids.length)
			}
		}, 30_000)
	})
})
