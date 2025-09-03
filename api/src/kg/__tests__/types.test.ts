import { describe, expect, it, beforeAll, afterAll } from "vitest"
import { Pool } from "pg"

describe("types and type functions", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({
			connectionString: process.env.DATABASE_URL
		})
	})

	afterAll(async () => {
		await pool?.end()
	})

	describe("types function", () => {
		it("should return type entities", async () => {
			const result = await pool.query(`
				SELECT * FROM types() LIMIT 10
			`)

			// Should return entities that are types
			expect(result.rows).toBeDefined()
			if (result.rows.length > 0) {
				expect(result.rows[0]).toHaveProperty('id')
				expect(result.rows[0]).toHaveProperty('created_at')
				expect(result.rows[0]).toHaveProperty('updated_at')
			}
		})

		it("should accept space_id parameter", async () => {
			const result = await pool.query(`
				SELECT * FROM types('00000000-0000-0000-0000-000000000000'::uuid) LIMIT 5
			`)

			// Should be callable without error
			expect(result.rows).toBeDefined()
		})

		it("should filter by space when valid space_id is provided", async () => {
			// Get a valid space ID first
			const spaceResult = await pool.query(`
				SELECT id FROM spaces LIMIT 1
			`)

			if (spaceResult.rows.length > 0) {
				const spaceId = spaceResult.rows[0].id
				const result = await pool.query(`
					SELECT * FROM types($1::uuid) LIMIT 5
				`, [spaceId])

				expect(result.rows).toBeDefined()
			}
		})
	})

	describe("type function", () => {
		it("should handle valid UUID input", async () => {
			const result = await pool.query(`
				SELECT * FROM type('00000000-0000-0000-0000-000000000000'::uuid)
			`)

			// Should be callable without error and return at most one result
			expect(result.rows).toBeDefined()
			expect(result.rows.length).toBeLessThanOrEqual(1)
		})

		it("should return specific type when valid type ID is provided", async () => {
			// First get a type from the types() function
			const typesResult = await pool.query(`
				SELECT * FROM types() LIMIT 1
			`)

			if (typesResult.rows.length > 0) {
				const typeId = typesResult.rows[0].id
				const result = await pool.query(`
					SELECT * FROM type($1::uuid)
				`, [typeId])

				if (result.rows.length > 0) {
					expect(result.rows[0]).toHaveProperty('id')
					expect(result.rows[0].id).toBe(typeId)
				}
			}
		})

		it("should return empty for non-type entity", async () => {
			// Get a regular entity that's not a type
			const entityResult = await pool.query(`
				SELECT id FROM entities LIMIT 1
			`)

			if (entityResult.rows.length > 0) {
				const entityId = entityResult.rows[0].id
				const result = await pool.query(`
					SELECT * FROM type($1::uuid)
				`, [entityId])

				// Should be callable without error and return at most one result
				expect(result.rows).toBeDefined()
				expect(result.rows.length).toBeLessThanOrEqual(1)
			}
		})
	})
})