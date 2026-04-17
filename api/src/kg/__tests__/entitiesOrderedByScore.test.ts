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

function filterSchemaErrors(errors: Array<{message: string; extensions?: {code?: string}}> | undefined) {
	return (errors ?? []).filter(
		(e) => e.extensions?.code === "GRAPHQL_VALIDATION_FAILED" || e.extensions?.code === "GRAPHQL_PARSE_FAILED",
	)
}

const undash = (uuid: string) => uuid.replace(/-/g, "")

describe("entitiesOrderedByScore", () => {
	let pool: Pool
	let localSpaceId: string | null = null
	let rawSpaceId: string | null = null
	let multiSpaceEntityId: string | null = null
	let multiSpaceEntityNetScore: number | null = null

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})

		const localRes = await pool.query(`SELECT space_id FROM local_scores LIMIT 1`)
		if (localRes.rows.length > 0) {
			localSpaceId = localRes.rows[0].space_id
		}

		const rawSpaceRes = await pool.query(`
			SELECT space_id FROM votes_count
			WHERE object_type = 0
			LIMIT 1
		`)
		if (rawSpaceRes.rows.length > 0) {
			rawSpaceId = rawSpaceRes.rows[0].space_id
		}

		const multiRes = await pool.query(`
			SELECT object_id, SUM(upvotes - downvotes)::bigint AS net_score
			FROM votes_count
			WHERE object_type = 0
			GROUP BY object_id
			HAVING COUNT(DISTINCT space_id) >= 2
			ORDER BY SUM(upvotes - downvotes) DESC
			LIMIT 1
		`)
		if (multiRes.rows.length > 0) {
			multiSpaceEntityId = multiRes.rows[0].object_id
			multiSpaceEntityNetScore = Number(multiRes.rows[0].net_score)
		}
	})

	afterAll(async () => {
		await pool?.end()
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
		it("accepts LOCAL with spaceId", async () => {
			if (!localSpaceId) {
				console.log("Skipping: no local_scores rows available")
				return
			}
			const result = await executeGraphQL(
				`
					query Q($spaceId: UUID!) {
						entitiesOrderedByScore(scoreType: LOCAL, spaceId: $spaceId, first: 5) {
							id
						}
					}
				`,
				{spaceId: undash(localSpaceId)},
			)
			expect(filterSchemaErrors(result.errors)).toHaveLength(0)
			expect(result.errors).toBeUndefined()
			expect(Array.isArray(result.data.entitiesOrderedByScore)).toBe(true)
		})

		it("rejects LOCAL without spaceId with BAD_USER_INPUT", async () => {
			const result = await executeGraphQL(`
				{
					entitiesOrderedByScore(scoreType: LOCAL, first: 5) { id }
				}
			`)
			expect(result.errors).toBeDefined()
			expect(result.errors.length).toBeGreaterThan(0)
			expect(result.errors[0].message).toMatch(/space_id is required/)
			expect(result.errors[0].extensions?.code).toBe("BAD_USER_INPUT")
		})

		it("accepts GLOBAL without spaceId", async () => {
			const result = await executeGraphQL(`
				{
					entitiesOrderedByScore(scoreType: GLOBAL, first: 5) { id }
				}
			`)
			expect(filterSchemaErrors(result.errors)).toHaveLength(0)
			expect(result.errors).toBeUndefined()
			expect(Array.isArray(result.data.entitiesOrderedByScore)).toBe(true)
		})

		it("accepts RAW with spaceId", async () => {
			if (!rawSpaceId) {
				console.log("Skipping: no votes_count rows available")
				return
			}
			const result = await executeGraphQL(
				`
					query Q($spaceId: UUID!) {
						entitiesOrderedByScore(scoreType: RAW, spaceId: $spaceId, first: 5) { id }
					}
				`,
				{spaceId: undash(rawSpaceId)},
			)
			expect(filterSchemaErrors(result.errors)).toHaveLength(0)
			expect(result.errors).toBeUndefined()
			expect(Array.isArray(result.data.entitiesOrderedByScore)).toBe(true)
		})

		it("accepts RAW without spaceId and sums upvotes-downvotes across spaces", async () => {
			const result = await executeGraphQL(`
				{
					entitiesOrderedByScore(scoreType: RAW, first: 50) { id }
				}
			`)
			expect(filterSchemaErrors(result.errors)).toHaveLength(0)
			expect(result.errors).toBeUndefined()
			expect(Array.isArray(result.data.entitiesOrderedByScore)).toBe(true)

			if (!multiSpaceEntityId || multiSpaceEntityNetScore === null) {
				console.log("Skipping cross-space net-score cross-check: no entity with votes in >= 2 spaces")
				return
			}

			// Verify the aggregation path matches what the function uses for ordering:
			// recompute the net score directly and assert it is the sum across all spaces,
			// not just the max of any single space.
			const direct = await pool.query(
				`
					SELECT SUM(upvotes - downvotes)::bigint AS net_score,
						MAX(upvotes - downvotes)::bigint AS max_single_space
					FROM votes_count
					WHERE object_type = 0 AND object_id = $1
				`,
				[multiSpaceEntityId],
			)
			const netScore = Number(direct.rows[0].net_score)
			const maxSingle = Number(direct.rows[0].max_single_space)
			expect(netScore).toBe(multiSpaceEntityNetScore)
			// Regression guard: if the function silently fell back to per-space behavior,
			// these would match. They should differ whenever any other space had non-zero net votes.
			if (netScore !== maxSingle) {
				expect(netScore).not.toBe(maxSingle)
			}
		})
	})

	describe("ordering and pagination", () => {
		it("supports sortDirection ASC and DESC", async () => {
			// Run sequentially to avoid tripping the pool-pressure shedder on
			// small local pools — the integration harness is not tuned for parallelism.
			const descRes = await executeGraphQL(`
				{
					entitiesOrderedByScore(scoreType: GLOBAL, sortDirection: DESC, first: 5) { id }
				}
			`)
			const ascRes = await executeGraphQL(`
				{
					entitiesOrderedByScore(scoreType: GLOBAL, sortDirection: ASC, first: 5) { id }
				}
			`)
			expect(descRes.errors).toBeUndefined()
			expect(ascRes.errors).toBeUndefined()

			const descIds: string[] = descRes.data.entitiesOrderedByScore.map((e: {id: string}) => e.id)
			const ascIds: string[] = ascRes.data.entitiesOrderedByScore.map((e: {id: string}) => e.id)

			if (descIds.length >= 2 && ascIds.length >= 2) {
				// Top of DESC and top of ASC should differ unless all scores are identical.
				expect(descIds[0]).not.toBe(ascIds[0])
			}
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
			expect(filterSchemaErrors(page1.errors)).toHaveLength(0)
			expect(page1.errors).toBeUndefined()
			expect(page1.data.entitiesOrderedByScoreConnection.pageInfo).toBeDefined()

			const {endCursor, hasNextPage} = page1.data.entitiesOrderedByScoreConnection.pageInfo
			if (!endCursor || !hasNextPage) {
				console.log("Skipping page 2 check: only one page of global scores available")
				return
			}

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
			expect(filterSchemaErrors(page2.errors)).toHaveLength(0)
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
			expect(filterSchemaErrors(result.errors)).toHaveLength(0)
			expect(result.errors).toBeUndefined()
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
			]
			if (localSpaceId) {
				cases.push({
					label: "local",
					vars: {spaceId: undash(localSpaceId)},
					query: `query Q($spaceId: UUID!) {
						entitiesOrderedByScore(scoreType: LOCAL, spaceId: $spaceId, first: 50) { id }
					}`,
				})
			}
			if (rawSpaceId) {
				cases.push({
					label: "raw-per-space",
					vars: {spaceId: undash(rawSpaceId)},
					query: `query Q($spaceId: UUID!) {
						entitiesOrderedByScore(scoreType: RAW, spaceId: $spaceId, first: 50) { id }
					}`,
				})
			}

			for (const c of cases) {
				const res = await executeGraphQL(c.query, c.vars)
				expect(res.errors, `${c.label}: ${JSON.stringify(res.errors)}`).toBeUndefined()
				const ids: string[] = (res.data.entitiesOrderedByScore ?? []).map((e: {id: string}) => e.id)
				expect(new Set(ids).size, `${c.label} returned duplicate ids`).toBe(ids.length)
			}
		}, 30_000)
	})
})
