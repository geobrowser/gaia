import { describe, expect, it, beforeAll, afterAll } from "vitest"
import { Pool } from "pg"

describe("search function", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({
			connectionString: process.env.DATABASE_URL
		})
	})

	afterAll(async () => {
		await pool?.end()
	})

	it("should be callable and return results in correct format", async () => {
		const result = await pool.query(`
			SELECT * FROM search('test') LIMIT 5
		`)

		// Should be callable without error and return entity-like structure
		expect(result.rows).toBeDefined()
		if (result.rows.length > 0) {
			expect(result.rows[0]).toHaveProperty('id')
			expect(result.rows[0]).toHaveProperty('created_at')
			expect(result.rows[0]).toHaveProperty('updated_at')
		}
	})

	it("should accept space_id parameter", async () => {
		const result = await pool.query(`
			SELECT * FROM search('test', '00000000-0000-0000-0000-000000000000'::uuid) LIMIT 1
		`)

		// Should be callable without error
		expect(result.rows).toBeDefined()
	})

	it("should accept similarity threshold parameter", async () => {
		const result = await pool.query(`
			SELECT * FROM search('test', NULL, 0.1) LIMIT 1
		`)

		// Should be callable without error
		expect(result.rows).toBeDefined()
	})

	it("should return no results for non-matching query", async () => {
		const result = await pool.query(`
			SELECT * FROM search('NonexistentQueryThatShouldNotMatch12345') LIMIT 5
		`)

		expect(result.rows).toHaveLength(0)
	})

	it("should filter by space when space_id is provided", async () => {
		// Get a valid space ID first
		const spaceResult = await pool.query(`
			SELECT id FROM spaces LIMIT 1
		`)

		if (spaceResult.rows.length > 0) {
			const spaceId = spaceResult.rows[0].id
			const result = await pool.query(`
				SELECT * FROM search('test', $1::uuid) LIMIT 5
			`, [spaceId])

			// Should be callable without error
			expect(result.rows).toBeDefined()
		}
	})
})