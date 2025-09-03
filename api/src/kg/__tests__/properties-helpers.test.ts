import { describe, expect, it, beforeAll, afterAll } from "vitest"
import { Pool } from "pg"

describe("property helper functions", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({
			connectionString: process.env.DATABASE_URL
		})
	})

	afterAll(async () => {
		await pool?.end()
	})

	describe("properties_name function", () => {
		it("should be callable and return string or null", async () => {
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
			}
		})

		it("should return name when property has name value", async () => {
			// Find a property that has a name value
			const result = await pool.query(`
				SELECT DISTINCT v.entity_id as property_id, v.string as name
				FROM values v
				JOIN properties p ON v.entity_id = p.id
				WHERE v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'
				  AND v.string IS NOT NULL
				  AND trim(v.string) != ''
				LIMIT 1
			`)

			if (result.rows.length > 0) {
				const propertyId = result.rows[0].property_id
				const expectedName = result.rows[0].name
				
				const nameResult = await pool.query(`
					SELECT properties_name(p) as name
					FROM properties p 
					WHERE id = $1
				`, [propertyId])

				expect(nameResult.rows).toHaveLength(1)
				expect(nameResult.rows[0].name).toBe(expectedName)
			}
		})
	})

	describe("properties_description function", () => {
		it("should be callable and return string or null", async () => {
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
			}
		})

		it("should return description when property has description value", async () => {
			// Find a property that has a description value
			const result = await pool.query(`
				SELECT DISTINCT v.entity_id as property_id, v.string as description
				FROM values v
				JOIN properties p ON v.entity_id = p.id
				WHERE v.property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037'
				  AND v.string IS NOT NULL
				  AND trim(v.string) != ''
				LIMIT 1
			`)

			if (result.rows.length > 0) {
				const propertyId = result.rows[0].property_id
				const expectedDescription = result.rows[0].description
				
				const descResult = await pool.query(`
					SELECT properties_description(p) as description
					FROM properties p 
					WHERE id = $1
				`, [propertyId])

				expect(descResult.rows).toHaveLength(1)
				expect(descResult.rows[0].description).toBe(expectedDescription)
			}
		})
	})

	describe("properties_renderable_type function", () => {
		it("should be callable and return UUID or null", async () => {
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
				// Should return UUID string or null
				const renderableType = result.rows[0].renderable_type
				expect(typeof renderableType === 'string' || renderableType === null).toBe(true)
			}
		})

		it("should return null for system name property", async () => {
			const result = await pool.query(`
				SELECT properties_renderable_type(p) as renderable_type
				FROM properties p 
				WHERE id = 'a126ca53-0c8e-48d5-b888-82c734c38935'
			`)

			if (result.rows.length > 0) {
				expect(result.rows[0].renderable_type).toBeNull()
			}
		})

		it("should return null for system description property", async () => {
			const result = await pool.query(`
				SELECT properties_renderable_type(p) as renderable_type
				FROM properties p 
				WHERE id = '9b1f76ff-9711-404c-861e-59dc3fa7d037'
			`)

			if (result.rows.length > 0) {
				expect(result.rows[0].renderable_type).toBeNull()
			}
		})
	})

	describe("properties_unit function", () => {
		it("should be callable and return UUID or null", async () => {
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
				// Should return UUID string or null
				const unit = result.rows[0].unit
				expect(typeof unit === 'string' || unit === null).toBe(true)
			}
		})
	})

	describe("properties_format function", () => {
		it("should be callable and return string or null", async () => {
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
				expect(typeof result.rows[0].format === 'string' || result.rows[0].format === null).toBe(true)
			}
		})

		it("should return format when property has format value", async () => {
			// Find a property that has a format value
			const result = await pool.query(`
				SELECT DISTINCT v.entity_id as property_id, v.string as format
				FROM values v
				JOIN properties p ON v.entity_id = p.id
				WHERE v.property_id = '396f8c72-dfd0-4b57-91ea-09c1b9321b2f'
				  AND v.string IS NOT NULL
				  AND trim(v.string) != ''
				LIMIT 1
			`)

			if (result.rows.length > 0) {
				const propertyId = result.rows[0].property_id
				const expectedFormat = result.rows[0].format
				
				const formatResult = await pool.query(`
					SELECT properties_format(p) as format
					FROM properties p 
					WHERE id = $1
				`, [propertyId])

				expect(formatResult.rows).toHaveLength(1)
				expect(formatResult.rows[0].format).toBe(expectedFormat)
			}
		})
	})

	describe("properties_relation_value_types function", () => {
		it("should be callable and return entity objects", async () => {
			const propertyResult = await pool.query(`
				SELECT id FROM properties WHERE type = 'Relation' LIMIT 1
			`)
			
			if (propertyResult.rows.length > 0) {
				const propertyId = propertyResult.rows[0].id
				const result = await pool.query(`
					SELECT properties_relation_value_types(p) as value_types
					FROM properties p 
					WHERE id = $1
				`, [propertyId])

				// Should be callable without error
				expect(result.rows).toBeDefined()
			}
		})

		it("should return empty for system name property", async () => {
			const result = await pool.query(`
				SELECT properties_relation_value_types(p) as value_types
				FROM properties p 
				WHERE id = 'a126ca53-0c8e-48d5-b888-82c734c38935'
			`)

			expect(result.rows).toHaveLength(0)
		})

		it("should return empty for system description property", async () => {
			const result = await pool.query(`
				SELECT properties_relation_value_types(p) as value_types
				FROM properties p 
				WHERE id = '9b1f76ff-9711-404c-861e-59dc3fa7d037'
			`)

			expect(result.rows).toHaveLength(0)
		})
	})

	describe("properties_relation_value_type_ids function", () => {
		it("should be callable and return array or null", async () => {
			const propertyResult = await pool.query(`
				SELECT id FROM properties WHERE type = 'Relation' LIMIT 1
			`)
			
			if (propertyResult.rows.length > 0) {
				const propertyId = propertyResult.rows[0].id
				const result = await pool.query(`
					SELECT properties_relation_value_type_ids(p) as value_type_ids
					FROM properties p 
					WHERE id = $1
				`, [propertyId])

				expect(result.rows).toHaveLength(1)
				expect(Array.isArray(result.rows[0].value_type_ids) || result.rows[0].value_type_ids === null).toBe(true)
			}
		})

		it("should return null for system name property", async () => {
			const result = await pool.query(`
				SELECT properties_relation_value_type_ids(p) as value_type_ids
				FROM properties p 
				WHERE id = 'a126ca53-0c8e-48d5-b888-82c734c38935'
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0].value_type_ids).toBeNull()
		})

		it("should return null for system description property", async () => {
			const result = await pool.query(`
				SELECT properties_relation_value_type_ids(p) as value_type_ids
				FROM properties p 
				WHERE id = '9b1f76ff-9711-404c-861e-59dc3fa7d037'
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0].value_type_ids).toBeNull()
		})
	})
})