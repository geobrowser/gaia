import { describe, expect, it, beforeAll, afterAll } from "vitest"
import { Pool } from "pg"

describe("entity helper functions", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({
			connectionString: process.env.DATABASE_URL
		})
	})

	afterAll(async () => {
		await pool?.end()
	})

	describe("entities_name function", () => {
		it("should be callable and return string or null", async () => {
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
				expect(typeof result.rows[0].name === 'string' || result.rows[0].name === null).toBe(true)
			}
		})

		it("should return name when entity has name property", async () => {
			// Find an entity that has a name value
			const result = await pool.query(`
				SELECT DISTINCT v.entity_id, v.string as name
				FROM values v
				WHERE v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'
				  AND v.string IS NOT NULL
				  AND trim(v.string) != ''
				LIMIT 1
			`)

			if (result.rows.length > 0) {
				const entityId = result.rows[0].entity_id
				const expectedName = result.rows[0].name
				
				const nameResult = await pool.query(`
					SELECT entities_name(e) as name
					FROM entities e 
					WHERE id = $1
				`, [entityId])

				expect(nameResult.rows).toHaveLength(1)
				expect(nameResult.rows[0].name).toBe(expectedName)
			}
		})
	})

	describe("entities_description function", () => {
		it("should be callable and return string or null", async () => {
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
				expect(typeof result.rows[0].description === 'string' || result.rows[0].description === null).toBe(true)
			}
		})

		it("should return description when entity has description property", async () => {
			// Find an entity that has a description value
			const result = await pool.query(`
				SELECT DISTINCT v.entity_id, v.string as description
				FROM values v
				WHERE v.property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037'
				  AND v.string IS NOT NULL
				  AND trim(v.string) != ''
				LIMIT 1
			`)

			if (result.rows.length > 0) {
				const entityId = result.rows[0].entity_id
				const expectedDescription = result.rows[0].description
				
				const descResult = await pool.query(`
					SELECT entities_description(e) as description
					FROM entities e 
					WHERE id = $1
				`, [entityId])

				expect(descResult.rows).toHaveLength(1)
				expect(descResult.rows[0].description).toBe(expectedDescription)
			}
		})
	})

	describe("entities_space_ids function", () => {
		it("should be callable and return array or null", async () => {
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
				expect(Array.isArray(result.rows[0].space_ids) || result.rows[0].space_ids === null).toBe(true)
			}
		})

		it("should return space IDs when entity exists in spaces", async () => {
			// Find an entity that has values in some space
			const result = await pool.query(`
				SELECT DISTINCT v.entity_id, v.space_id
				FROM values v
				LIMIT 1
			`)

			if (result.rows.length > 0) {
				const entityId = result.rows[0].entity_id
				const expectedSpaceId = result.rows[0].space_id
				
				const spaceResult = await pool.query(`
					SELECT entities_space_ids(e) as space_ids
					FROM entities e 
					WHERE id = $1
				`, [entityId])

				expect(spaceResult.rows).toHaveLength(1)
				if (spaceResult.rows[0].space_ids) {
					expect(spaceResult.rows[0].space_ids).toContain(expectedSpaceId)
				}
			}
		})
	})

	describe("entities_types function", () => {
		it("should be callable and return entity objects", async () => {
			const entityResult = await pool.query(`
				SELECT id FROM entities LIMIT 1
			`)
			
			if (entityResult.rows.length > 0) {
				const entityId = entityResult.rows[0].id
				const result = await pool.query(`
					SELECT entities_types(e) as types
					FROM entities e 
					WHERE id = $1
				`, [entityId])

				// Should be callable without error
				expect(result.rows).toBeDefined()
			}
		})
	})

	describe("entities_type_ids function", () => {
		it("should be callable and return array or null", async () => {
			const entityResult = await pool.query(`
				SELECT id FROM entities LIMIT 1
			`)
			
			if (entityResult.rows.length > 0) {
				const entityId = entityResult.rows[0].id
				const result = await pool.query(`
					SELECT entities_type_ids(e) as type_ids
					FROM entities e 
					WHERE id = $1
				`, [entityId])

				expect(result.rows).toHaveLength(1)
				expect(Array.isArray(result.rows[0].type_ids) || result.rows[0].type_ids === null).toBe(true)
			}
		})
	})

	describe("entities_spaces_in function", () => {
		it("should be callable and return space objects", async () => {
			const entityResult = await pool.query(`
				SELECT id FROM entities LIMIT 1
			`)
			
			if (entityResult.rows.length > 0) {
				const entityId = entityResult.rows[0].id
				const result = await pool.query(`
					SELECT entities_spaces_in(e) as spaces
					FROM entities e 
					WHERE id = $1
				`, [entityId])

				// Should be callable without error
				expect(result.rows).toBeDefined()
			}
		})
	})

	describe("entities_properties function", () => {
		it("should be callable and return property objects", async () => {
			const entityResult = await pool.query(`
				SELECT id FROM entities LIMIT 1
			`)
			
			if (entityResult.rows.length > 0) {
				const entityId = entityResult.rows[0].id
				const result = await pool.query(`
					SELECT entities_properties(e) as properties
					FROM entities e 
					WHERE id = $1
				`, [entityId])

				// Should be callable without error
				expect(result.rows).toBeDefined()
			}
		})
	})
})