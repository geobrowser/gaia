import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {uuidToBase58} from "../../utils/uuid"
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

describe("EntitySpaceFilterPlugin", () => {
	let pool: Pool
	/** Base58-encoded UUID sent as GraphQL variable input (parseValue only accepts Base58) */
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
			testSpaceId = uuidToBase58(spaceResult.rows[0].space_id)
		}

		// Find a type that has entities (via SystemIds.Types relation)
		const typeResult = await pool.query(`
			SELECT DISTINCT r.to_entity_id
			FROM relations r
			WHERE r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'
			LIMIT 1
		`)

		if (typeResult.rows.length > 0) {
			testTypeId = uuidToBase58(typeResult.rows[0].to_entity_id)
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
			const fakeSpaceId = uuidToBase58("00000001-0000-0000-0000-000000000000")

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
			const fakeTypeId = uuidToBase58("00000001-0000-0000-0000-000000000000")

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
