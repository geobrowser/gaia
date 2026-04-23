import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

// Helper to execute GraphQL queries against the yoga server
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

/**
 * Read the cumulative count of sequential scans Postgres has run against
 * a table since startup. Compare before/after a query to detect whether
 * the query forced a full scan — a useful assertion for filter plugins
 * that must route through an indexed path.
 *
 * Note: the counter is per-table and increments once per executed Seq
 * Scan node. If the test DB has concurrent background activity that
 * happens to scan `entities`, the delta could be >0 for reasons outside
 * the query under test — in that case, loosen to `<= 1` (the bug
 * increments by exactly 1 per offending call).
 */
async function getSeqScanCount(pool: Pool, table: string): Promise<number> {
	const r = await pool.query(
		"SELECT seq_scan FROM pg_stat_user_tables WHERE schemaname = 'public' AND relname = $1",
		[table],
	)
	return Number(r.rows[0]?.seq_scan ?? 0)
}

describe("EntitySpaceFilterPlugin", () => {
	let pool: Pool
	let testSpaceId: string | null = null
	let testTypeId: string | null = null

	beforeAll(async () => {
		pool = new Pool({
			connectionString: process.env.DATABASE_URL,
		})

		// Find a space that has entities with values or relations
		const spaceResult = await pool.query(`
			SELECT DISTINCT v.space_id
			FROM values v
			WHERE v.space_id IS NOT NULL
			LIMIT 1
		`)

		if (spaceResult.rows.length > 0) {
			// Convert UUID to undashed format for GraphQL
			testSpaceId = spaceResult.rows[0].space_id.replace(/-/g, "")
		}

		// Find a type that has entities (via SystemIds.Types relation)
		const typeResult = await pool.query(`
			SELECT DISTINCT r.to_entity_id
			FROM relations r
			WHERE r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'
			LIMIT 1
		`)

		if (typeResult.rows.length > 0) {
			// Convert UUID to undashed format for GraphQL
			testTypeId = typeResult.rows[0].to_entity_id.replace(/-/g, "")
		}
	})

	afterAll(async () => {
		await pool?.end()
	})

	// ============================================================================
	// Space filter tests
	// ============================================================================

	describe("spaceId argument", () => {
		it("should filter entities by single space ID", async () => {
			if (!testSpaceId) {
				console.log("Skipping test: no space with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestSpaceFilter($spaceId: UUID!) {
					entities(spaceId: $spaceId, first: 5) {
						id
						spaceIds
					}
				}
			`,
				{spaceId: testSpaceId},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()
			expect(Array.isArray(result.data.entities)).toBe(true)

			// All returned entities should have the test space in their spaceIds
			for (const entity of result.data.entities) {
				expect(entity.spaceIds).toContain(testSpaceId)
			}
		})

		it("should return empty array for non-existent space ID", async () => {
			const fakeSpaceId = "00000000000000000000000000000000"

			const result = await executeGraphQL(
				`
				query TestSpaceFilter($spaceId: UUID!) {
					entities(spaceId: $spaceId, first: 5) {
						id
					}
				}
			`,
				{spaceId: fakeSpaceId},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toEqual([])
		})
	})

	describe("spaceIds argument with operators", () => {
		it("should filter with 'is' operator", async () => {
			if (!testSpaceId) {
				console.log("Skipping test: no space with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestSpaceFilter($spaceId: UUID!) {
					entities(spaceIds: { is: $spaceId }, first: 5) {
						id
						spaceIds
					}
				}
			`,
				{spaceId: testSpaceId},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			for (const entity of result.data.entities) {
				expect(entity.spaceIds).toContain(testSpaceId)
			}
		})

		it("should filter with 'in' operator", async () => {
			if (!testSpaceId) {
				console.log("Skipping test: no space with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestSpaceFilter($spaceIds: [UUID!]!) {
					entities(spaceIds: { in: $spaceIds }, first: 5) {
						id
						spaceIds
					}
				}
			`,
				{spaceIds: [testSpaceId]},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			for (const entity of result.data.entities) {
				expect(entity.spaceIds).toContain(testSpaceId)
			}
		})

		it("should filter with 'isNot' operator", async () => {
			if (!testSpaceId) {
				console.log("Skipping test: no space with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestSpaceFilter($spaceId: UUID!) {
					entities(spaceIds: { isNot: $spaceId }, first: 5) {
						id
						spaceIds
					}
				}
			`,
				{spaceId: testSpaceId},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			// None of the returned entities should have the test space
			for (const entity of result.data.entities) {
				expect(entity.spaceIds).not.toContain(testSpaceId)
			}
		})

		it("should filter with 'notIn' operator", async () => {
			if (!testSpaceId) {
				console.log("Skipping test: no space with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestSpaceFilter($spaceIds: [UUID!]!) {
					entities(spaceIds: { notIn: $spaceIds }, first: 5) {
						id
						spaceIds
					}
				}
			`,
				{spaceIds: [testSpaceId]},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			for (const entity of result.data.entities) {
				expect(entity.spaceIds).not.toContain(testSpaceId)
			}
		})

		it("should filter with 'isNull: false' to find entities with spaces", async () => {
			const result = await executeGraphQL(`
				query TestSpaceFilter {
					entities(spaceIds: { isNull: false }, first: 5) {
						id
						spaceIds
					}
				}
			`)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			// All returned entities should have at least one space
			for (const entity of result.data.entities) {
				expect(entity.spaceIds.length).toBeGreaterThan(0)
			}
		})

		it("should filter with 'isNull: true' to find entities without spaces", async () => {
			const result = await executeGraphQL(`
				query TestSpaceFilter {
					entities(spaceIds: { isNull: true }, first: 5) {
						id
						spaceIds
					}
				}
			`)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			// All returned entities should have no spaces
			for (const entity of result.data.entities) {
				expect(entity.spaceIds).toEqual([])
			}
		})
	})

	// ============================================================================
	// Type filter tests
	// ============================================================================

	describe("typeId argument", () => {
		it("should filter entities by single type ID", async () => {
			if (!testTypeId) {
				console.log("Skipping test: no type with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestTypeFilter($typeId: UUID!) {
					entities(typeId: $typeId, first: 5) {
						id
						typeIds
					}
				}
			`,
				{typeId: testTypeId},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()
			expect(Array.isArray(result.data.entities)).toBe(true)

			// All returned entities should have the test type in their typeIds
			for (const entity of result.data.entities) {
				expect(entity.typeIds).toContain(testTypeId)
			}
		})

		it("should return empty array for non-existent type ID", async () => {
			const fakeTypeId = "00000000000000000000000000000000"

			const result = await executeGraphQL(
				`
				query TestTypeFilter($typeId: UUID!) {
					entities(typeId: $typeId, first: 5) {
						id
					}
				}
			`,
				{typeId: fakeTypeId},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toEqual([])
		})
	})

	describe("typeIds argument with operators", () => {
		it("should filter with 'is' operator", async () => {
			if (!testTypeId) {
				console.log("Skipping test: no type with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestTypeFilter($typeId: UUID!) {
					entities(typeIds: { is: $typeId }, first: 5) {
						id
						typeIds
					}
				}
			`,
				{typeId: testTypeId},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			for (const entity of result.data.entities) {
				expect(entity.typeIds).toContain(testTypeId)
			}
		})

		it("should filter with 'in' operator", async () => {
			if (!testTypeId) {
				console.log("Skipping test: no type with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestTypeFilter($typeIds: [UUID!]!) {
					entities(typeIds: { in: $typeIds }, first: 5) {
						id
						typeIds
					}
				}
			`,
				{typeIds: [testTypeId]},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			for (const entity of result.data.entities) {
				expect(entity.typeIds).toContain(testTypeId)
			}
		})

		it("should filter with 'isNot' operator", async () => {
			if (!testTypeId) {
				console.log("Skipping test: no type with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestTypeFilter($typeId: UUID!) {
					entities(typeIds: { isNot: $typeId }, first: 5) {
						id
						typeIds
					}
				}
			`,
				{typeId: testTypeId},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			// None of the returned entities should have the test type
			for (const entity of result.data.entities) {
				expect(entity.typeIds).not.toContain(testTypeId)
			}
		})

		it("should filter with 'notIn' operator", async () => {
			if (!testTypeId) {
				console.log("Skipping test: no type with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestTypeFilter($typeIds: [UUID!]!) {
					entities(typeIds: { notIn: $typeIds }, first: 5) {
						id
						typeIds
					}
				}
			`,
				{typeIds: [testTypeId]},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			for (const entity of result.data.entities) {
				expect(entity.typeIds).not.toContain(testTypeId)
			}
		})

		it("should filter with 'isNull: false' to find entities with types", async () => {
			const result = await executeGraphQL(`
				query TestTypeFilter {
					entities(typeIds: { isNull: false }, first: 5) {
						id
						typeIds
					}
				}
			`)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			// All returned entities should have at least one type
			for (const entity of result.data.entities) {
				expect(entity.typeIds.length).toBeGreaterThan(0)
			}
		})

		it("should filter with 'isNull: true' to find entities without types", async () => {
			const result = await executeGraphQL(`
				query TestTypeFilter {
					entities(typeIds: { isNull: true }, first: 5) {
						id
						typeIds
					}
				}
			`)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			// All returned entities should have no types
			for (const entity of result.data.entities) {
				expect(entity.typeIds).toEqual([])
			}
		})

		// --------------------------------------------------------------------
		// `overlaps` and `contains` — array-set operators on the typeIds
		// computed column. Without the custom plugin handler, PostGraphile
		// falls back to `entities_type_ids(e) && $1` (or `@> $1`), which
		// forces a seq scan over the entities table and calls the computed
		// function per row — observed as nginx 504 in prod (PR #635).
		// These tests verify both:
		//   (1) correctness — overlaps is equivalent to `in`, contains is AND
		//   (2) plan shape — no new seq_scan on entities (plugin routes
		//       through the indexed EXISTS path on relations_to_entity_id_idx)
		// --------------------------------------------------------------------

		it("should filter with 'overlaps' operator (equivalent to 'in' for type arrays)", async () => {
			if (!testTypeId) {
				console.log("Skipping test: no type with entities found")
				return
			}

			const result = await executeGraphQL(
				`
				query TestTypeFilter($typeIds: [UUID!]!) {
					entities(typeIds: { overlaps: $typeIds }, first: 5) {
						id
						typeIds
					}
				}
			`,
				{typeIds: [testTypeId]},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			for (const entity of result.data.entities) {
				expect(entity.typeIds).toContain(testTypeId)
			}
		})

		it("should filter with 'contains' operator (requires all specified types)", async () => {
			if (!testTypeId) {
				console.log("Skipping test: no type with entities found")
				return
			}

			// Single-type `contains` — every returned entity must have that type.
			const result = await executeGraphQL(
				`
				query TestTypeFilter($typeIds: [UUID!]!) {
					entities(typeIds: { contains: $typeIds }, first: 5) {
						id
						typeIds
					}
				}
			`,
				{typeIds: [testTypeId]},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			for (const entity of result.data.entities) {
				expect(entity.typeIds).toContain(testTypeId)
			}
		})

		it("'overlaps' filter does not trigger a sequential scan on entities", async () => {
			// The bug we're fixing: without the custom plugin case, the generated
			// SQL is `entities_type_ids(e) && $1`, which forces `Seq Scan on
			// entities`. pg_stat_user_tables.seq_scan increments by 1 per full
			// sequential scan executed against the table. If the fix is working,
			// the counter should not move for this query.
			if (!testTypeId) {
				console.log("Skipping test: no type with entities found")
				return
			}

			const before = await getSeqScanCount(pool, "entities")
			const result = await executeGraphQL(
				`
				query TestTypeFilter($typeIds: [UUID!]!) {
					entities(typeIds: { overlaps: $typeIds }, first: 5) {
						id
						typeIds
					}
				}
			`,
				{typeIds: [testTypeId]},
			)
			const after = await getSeqScanCount(pool, "entities")

			expect(result.errors).toBeUndefined()
			// Tightest assertion: no seq_scan at all. If this ever gets flaky
			// because of an unrelated concurrent workload, loosen to
			// `expect(after - before).toBeLessThanOrEqual(1)` — still catches
			// the bug, which increments by at least 1 per test call.
			expect(after - before).toBe(0)
		})

		it("'contains' filter does not trigger a sequential scan on entities", async () => {
			if (!testTypeId) {
				console.log("Skipping test: no type with entities found")
				return
			}

			const before = await getSeqScanCount(pool, "entities")
			const result = await executeGraphQL(
				`
				query TestTypeFilter($typeIds: [UUID!]!) {
					entities(typeIds: { contains: $typeIds }, first: 5) {
						id
						typeIds
					}
				}
			`,
				{typeIds: [testTypeId]},
			)
			const after = await getSeqScanCount(pool, "entities")

			expect(result.errors).toBeUndefined()
			expect(after - before).toBe(0)
		})
	})

	// ============================================================================
	// Combined filter tests
	// ============================================================================

	describe("combined space and type filters", () => {
		it("should filter by both space and type", async () => {
			if (!testSpaceId || !testTypeId) {
				console.log("Skipping test: need both space and type with entities")
				return
			}

			const result = await executeGraphQL(
				`
				query TestCombinedFilter($spaceId: UUID!, $typeId: UUID!) {
					entities(spaceId: $spaceId, typeId: $typeId, first: 5) {
						id
						spaceIds
						typeIds
					}
				}
			`,
				{spaceId: testSpaceId, typeId: testTypeId},
			)

			expect(result.errors).toBeUndefined()
			expect(result.data.entities).toBeDefined()

			// All returned entities should have both the space and type
			for (const entity of result.data.entities) {
				expect(entity.spaceIds).toContain(testSpaceId)
				expect(entity.typeIds).toContain(testTypeId)
			}
		})
	})

	// ============================================================================
	// Schema introspection tests
	// ============================================================================

	describe("argument availability", () => {
		it("should expose spaceId argument on entities field", async () => {
			const result = await executeGraphQL(`
				query IntrospectEntities {
					__type(name: "Query") {
						fields {
							name
							args {
								name
								type { name }
							}
						}
					}
				}
			`)

			expect(result.errors).toBeUndefined()

			const entitiesField = result.data.__type.fields.find((f: {name: string}) => f.name === "entities")
			expect(entitiesField).toBeDefined()

			const spaceIdArg = entitiesField.args.find((a: {name: string}) => a.name === "spaceId")
			expect(spaceIdArg).toBeDefined()
			expect(spaceIdArg.type.name).toBe("UUID")
		})

		it("should expose spaceIds argument on entities field", async () => {
			const result = await executeGraphQL(`
				query IntrospectEntities {
					__type(name: "Query") {
						fields {
							name
							args {
								name
								type { name }
							}
						}
					}
				}
			`)

			expect(result.errors).toBeUndefined()

			const entitiesField = result.data.__type.fields.find((f: {name: string}) => f.name === "entities")
			expect(entitiesField).toBeDefined()

			const spaceIdsArg = entitiesField.args.find((a: {name: string}) => a.name === "spaceIds")
			expect(spaceIdsArg).toBeDefined()
			expect(spaceIdsArg.type.name).toBe("UUIDFilter")
		})

		it("should expose typeId argument on entities field", async () => {
			const result = await executeGraphQL(`
				query IntrospectEntities {
					__type(name: "Query") {
						fields {
							name
							args {
								name
								type { name }
							}
						}
					}
				}
			`)

			expect(result.errors).toBeUndefined()

			const entitiesField = result.data.__type.fields.find((f: {name: string}) => f.name === "entities")
			expect(entitiesField).toBeDefined()

			const typeIdArg = entitiesField.args.find((a: {name: string}) => a.name === "typeId")
			expect(typeIdArg).toBeDefined()
			expect(typeIdArg.type.name).toBe("UUID")
		})

		it("should expose typeIds argument on entities field", async () => {
			const result = await executeGraphQL(`
				query IntrospectEntities {
					__type(name: "Query") {
						fields {
							name
							args {
								name
								type { name }
							}
						}
					}
				}
			`)

			expect(result.errors).toBeUndefined()

			const entitiesField = result.data.__type.fields.find((f: {name: string}) => f.name === "entities")
			expect(entitiesField).toBeDefined()

			const typeIdsArg = entitiesField.args.find((a: {name: string}) => a.name === "typeIds")
			expect(typeIdsArg).toBeDefined()
			expect(typeIdsArg.type.name).toBe("UUIDFilter")
		})
	})
})
