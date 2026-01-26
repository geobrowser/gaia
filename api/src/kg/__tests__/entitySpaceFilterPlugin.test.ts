import { describe, expect, it, beforeAll, afterAll } from "vitest"
import { Pool } from "pg"
import { graphqlServer } from "../postgraphile"

// Helper to execute GraphQL queries against the yoga server
async function executeGraphQL(query: string, variables?: Record<string, unknown>) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ query, variables }),
		}),
		{}
	)
	return response.json()
}

describe("EntitySpaceFilterPlugin", () => {
	let pool: Pool
	let testSpaceId: string | null = null

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
	})

	afterAll(async () => {
		await pool?.end()
	})

	describe("spaceId argument", () => {
		it("should filter entities by single space ID", async () => {
			if (!testSpaceId) {
				console.log("Skipping test: no space with entities found")
				return
			}

			const result = await executeGraphQL(`
				query TestSpaceFilter($spaceId: UUID!) {
					entities(spaceId: $spaceId, first: 5) {
						id
						spaceIds
					}
				}
			`, { spaceId: testSpaceId })

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

			const result = await executeGraphQL(`
				query TestSpaceFilter($spaceId: UUID!) {
					entities(spaceId: $spaceId, first: 5) {
						id
					}
				}
			`, { spaceId: fakeSpaceId })

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

			const result = await executeGraphQL(`
				query TestSpaceFilter($spaceId: UUID!) {
					entities(spaceIds: { is: $spaceId }, first: 5) {
						id
						spaceIds
					}
				}
			`, { spaceId: testSpaceId })

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

			const result = await executeGraphQL(`
				query TestSpaceFilter($spaceIds: [UUID!]!) {
					entities(spaceIds: { in: $spaceIds }, first: 5) {
						id
						spaceIds
					}
				}
			`, { spaceIds: [testSpaceId] })

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

			const result = await executeGraphQL(`
				query TestSpaceFilter($spaceId: UUID!) {
					entities(spaceIds: { isNot: $spaceId }, first: 5) {
						id
						spaceIds
					}
				}
			`, { spaceId: testSpaceId })

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

			const result = await executeGraphQL(`
				query TestSpaceFilter($spaceIds: [UUID!]!) {
					entities(spaceIds: { notIn: $spaceIds }, first: 5) {
						id
						spaceIds
					}
				}
			`, { spaceIds: [testSpaceId] })

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

			const entitiesField = result.data.__type.fields.find(
				(f: { name: string }) => f.name === "entities"
			)
			expect(entitiesField).toBeDefined()

			const spaceIdArg = entitiesField.args.find(
				(a: { name: string }) => a.name === "spaceId"
			)
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

			const entitiesField = result.data.__type.fields.find(
				(f: { name: string }) => f.name === "entities"
			)
			expect(entitiesField).toBeDefined()

			const spaceIdsArg = entitiesField.args.find(
				(a: { name: string }) => a.name === "spaceIds"
			)
			expect(spaceIdsArg).toBeDefined()
			expect(spaceIdsArg.type.name).toBe("UUIDFilter")
		})
	})
})
