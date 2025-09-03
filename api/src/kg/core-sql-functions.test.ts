import { describe, expect, it, beforeAll, afterAll } from "vitest"
import { Pool } from "pg"

describe("Core SQL functions integration tests", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({
			connectionString: process.env.DATABASE_URL
		})
	})

	afterAll(async () => {
		await pool?.end()
	})

	describe("search function", () => {
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
			}
		})

		it("should accept space_id parameter", async () => {
			const result = await pool.query(`
				SELECT * FROM types('00000000-0000-0000-0000-000000000000'::uuid) LIMIT 5
			`)

			// Should be callable without error
			expect(result.rows).toBeDefined()
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
	})

	describe("Entity helper functions", () => {
		it("entities_name function should be callable", async () => {
			// First get an entity ID to test with
			const entityResult = await pool.query(`
				SELECT id FROM entities LIMIT 1
			`)
			
			if (entityResult.rows.length > 0) {
				const entityId = entityResult.rows[0].id
				const result = await pool.query(`
					SELECT entities_name(e) as name
					FROM entities e 
					WHERE id = $1
				`, [entityId])

				expect(result.rows).toHaveLength(1)
				// Name can be null or string
				expect(typeof result.rows[0].name === 'string' || result.rows[0].name === null).toBe(true)
			} else {
				// Skip if no entities exist
				expect(true).toBe(true)
			}
		})

		it("entities_description function should be callable", async () => {
			const entityResult = await pool.query(`
				SELECT id FROM entities LIMIT 1
			`)
			
			if (entityResult.rows.length > 0) {
				const entityId = entityResult.rows[0].id
				const result = await pool.query(`
					SELECT entities_description(e) as description
					FROM entities e 
					WHERE id = $1
				`, [entityId])

				expect(result.rows).toHaveLength(1)
				// Description can be null or string
				expect(typeof result.rows[0].description === 'string' || result.rows[0].description === null).toBe(true)
			} else {
				expect(true).toBe(true)
			}
		})

		it("entities_space_ids function should be callable", async () => {
			const entityResult = await pool.query(`
				SELECT id FROM entities LIMIT 1
			`)
			
			if (entityResult.rows.length > 0) {
				const entityId = entityResult.rows[0].id
				const result = await pool.query(`
					SELECT entities_space_ids(e) as space_ids
					FROM entities e 
					WHERE id = $1
				`, [entityId])

				expect(result.rows).toHaveLength(1)
				// Space IDs can be null or array
				expect(Array.isArray(result.rows[0].space_ids) || result.rows[0].space_ids === null).toBe(true)
			} else {
				expect(true).toBe(true)
			}
		})
	})

	describe("Property helper functions", () => {
		it("properties_name function should be callable", async () => {
			const propertyResult = await pool.query(`
				SELECT id FROM properties LIMIT 1
			`)
			
			if (propertyResult.rows.length > 0) {
				const propertyId = propertyResult.rows[0].id
				const result = await pool.query(`
					SELECT properties_name(p) as name
					FROM properties p 
					WHERE id = $1
				`, [propertyId])

				expect(result.rows).toHaveLength(1)
				expect(typeof result.rows[0].name === 'string' || result.rows[0].name === null).toBe(true)
			} else {
				expect(true).toBe(true)
			}
		})

		it("properties_description function should be callable", async () => {
			const propertyResult = await pool.query(`
				SELECT id FROM properties LIMIT 1
			`)
			
			if (propertyResult.rows.length > 0) {
				const propertyId = propertyResult.rows[0].id
				const result = await pool.query(`
					SELECT properties_description(p) as description
					FROM properties p 
					WHERE id = $1
				`, [propertyId])

				expect(result.rows).toHaveLength(1)
				expect(typeof result.rows[0].description === 'string' || result.rows[0].description === null).toBe(true)
			} else {
				expect(true).toBe(true)
			}
		})

		it("properties_renderable_type function should be callable", async () => {
			const propertyResult = await pool.query(`
				SELECT id FROM properties LIMIT 1
			`)
			
			if (propertyResult.rows.length > 0) {
				const propertyId = propertyResult.rows[0].id
				const result = await pool.query(`
					SELECT properties_renderable_type(p) as renderable_type
					FROM properties p 
					WHERE id = $1
				`, [propertyId])

				expect(result.rows).toHaveLength(1)
				// Should return UUID or null
			} else {
				expect(true).toBe(true)
			}
		})

		it("properties_unit function should be callable", async () => {
			const propertyResult = await pool.query(`
				SELECT id FROM properties LIMIT 1
			`)
			
			if (propertyResult.rows.length > 0) {
				const propertyId = propertyResult.rows[0].id
				const result = await pool.query(`
					SELECT properties_unit(p) as unit
					FROM properties p 
					WHERE id = $1
				`, [propertyId])

				expect(result.rows).toHaveLength(1)
			} else {
				expect(true).toBe(true)
			}
		})

		it("properties_format function should be callable", async () => {
			const propertyResult = await pool.query(`
				SELECT id FROM properties LIMIT 1
			`)
			
			if (propertyResult.rows.length > 0) {
				const propertyId = propertyResult.rows[0].id
				const result = await pool.query(`
					SELECT properties_format(p) as format
					FROM properties p 
					WHERE id = $1
				`, [propertyId])

				expect(result.rows).toHaveLength(1)
			} else {
				expect(true).toBe(true)
			}
		})
	})

	describe("Space helper functions", () => {
		it("spaces_page function should be callable", async () => {
			const spaceResult = await pool.query(`
				SELECT id FROM spaces LIMIT 1
			`)
			
			if (spaceResult.rows.length > 0) {
				const spaceId = spaceResult.rows[0].id
				const result = await pool.query(`
					SELECT spaces_page(s) as page
					FROM spaces s 
					WHERE id = $1
				`, [spaceId])

				expect(result.rows).toHaveLength(1)
			} else {
				expect(true).toBe(true)
			}
		})
	})
})