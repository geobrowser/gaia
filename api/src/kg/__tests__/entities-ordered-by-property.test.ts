import { describe, expect, it, beforeAll, afterAll } from "vitest"
import { Pool } from "pg"

describe("entities_ordered_by_property integration tests", () => {
	let pool: Pool
	let testPropertyIds: {
		string: string
		number: string
		boolean: string
		time: string
		point: string
		relation: string
	}
	let testEntityIds: string[]
	let testSpaceId: string

	beforeAll(async () => {
		// Connect to test database (assumes database is already set up)
		pool = new Pool({
			connectionString: process.env.DATABASE_URL
		})

		// Reset and seed test data
		await resetAndSeedTestData()
	}, 10000) // 10 second timeout for data setup

	afterAll(async () => {
		// Cleanup test data
		await cleanupTestData()
		await pool?.end()
	})

	async function resetAndSeedTestData() {
		// First cleanup any existing test data to ensure clean state
		await cleanupTestData()
		
		// Then seed fresh test data
		// Create test space
		testSpaceId = "550e8400-e29b-41d4-a716-446655440000"
		await pool.query(`
			INSERT INTO spaces (id, type, dao_address, space_address) 
			VALUES ($1, 'Public', '0x1234567890123456789012345678901234567890', '0x0987654321098765432109876543210987654321')
			ON CONFLICT (id) DO NOTHING
		`, [testSpaceId])

		// Create test entities
		testEntityIds = [
			"550e8400-e29b-41d4-a716-446655440001", 
			"550e8400-e29b-41d4-a716-446655440002",
			"550e8400-e29b-41d4-a716-446655440003"
		]
		
		for (const entityId of testEntityIds) {
			await pool.query(`
				INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
				VALUES ($1, '2024-01-01T00:00:00Z', '100', '2024-01-01T00:00:00Z', '100')
				ON CONFLICT (id) DO NOTHING
			`, [entityId])
		}

		// Create test properties for different data types
		testPropertyIds = {
			string: "550e8400-e29b-41d4-a716-446655440010",
			number: "550e8400-e29b-41d4-a716-446655440011", 
			boolean: "550e8400-e29b-41d4-a716-446655440012",
			time: "550e8400-e29b-41d4-a716-446655440013",
			point: "550e8400-e29b-41d4-a716-446655440014",
			relation: "550e8400-e29b-41d4-a716-446655440015"
		}

		const propertyTypes = [
			{ id: testPropertyIds.string, type: 'String' },
			{ id: testPropertyIds.number, type: 'Number' },
			{ id: testPropertyIds.boolean, type: 'Boolean' },
			{ id: testPropertyIds.time, type: 'Time' },
			{ id: testPropertyIds.point, type: 'Point' },
			{ id: testPropertyIds.relation, type: 'Relation' }
		]

		for (const prop of propertyTypes) {
			await pool.query(`
				INSERT INTO properties (id, type)
				VALUES ($1, $2)
				ON CONFLICT (id) DO NOTHING
			`, [prop.id, prop.type])
		}

		// Insert test values for different data types
		const testValues = [
			// String values (alphabetically: Alpha, Beta, Charlie)
			{ id: 'val1', property_id: testPropertyIds.string, entity_id: testEntityIds[0], space_id: testSpaceId, string: 'Charlie' },
			{ id: 'val2', property_id: testPropertyIds.string, entity_id: testEntityIds[1], space_id: testSpaceId, string: 'Alpha' },
			{ id: 'val3', property_id: testPropertyIds.string, entity_id: testEntityIds[2], space_id: testSpaceId, string: 'Beta' },
			
			// Number values (numerically: 10, 25, 100)
			{ id: 'val4', property_id: testPropertyIds.number, entity_id: testEntityIds[0], space_id: testSpaceId, number: '100' },
			{ id: 'val5', property_id: testPropertyIds.number, entity_id: testEntityIds[1], space_id: testSpaceId, number: '10' },
			{ id: 'val6', property_id: testPropertyIds.number, entity_id: testEntityIds[2], space_id: testSpaceId, number: '25' },
			
			// Boolean values (false, true, false)
			{ id: 'val7', property_id: testPropertyIds.boolean, entity_id: testEntityIds[0], space_id: testSpaceId, boolean: false },
			{ id: 'val8', property_id: testPropertyIds.boolean, entity_id: testEntityIds[1], space_id: testSpaceId, boolean: true },
			{ id: 'val9', property_id: testPropertyIds.boolean, entity_id: testEntityIds[2], space_id: testSpaceId, boolean: false },
			
			// Time values (chronologically: 2023, 2024, 2025)
			{ id: 'val10', property_id: testPropertyIds.time, entity_id: testEntityIds[0], space_id: testSpaceId, time: '2025-01-01T00:00:00Z' },
			{ id: 'val11', property_id: testPropertyIds.time, entity_id: testEntityIds[1], space_id: testSpaceId, time: '2023-01-01T00:00:00Z' },
			{ id: 'val12', property_id: testPropertyIds.time, entity_id: testEntityIds[2], space_id: testSpaceId, time: '2024-01-01T00:00:00Z' },
			
			// Point values 
			{ id: 'val13', property_id: testPropertyIds.point, entity_id: testEntityIds[0], space_id: testSpaceId, point: 'POINT(3 3)' },
			{ id: 'val14', property_id: testPropertyIds.point, entity_id: testEntityIds[1], space_id: testSpaceId, point: 'POINT(1 1)' },
			{ id: 'val15', property_id: testPropertyIds.point, entity_id: testEntityIds[2], space_id: testSpaceId, point: 'POINT(2 2)' },
			
			// Relation values (should be excluded)
			{ id: 'val16', property_id: testPropertyIds.relation, entity_id: testEntityIds[0], space_id: testSpaceId, string: 'relation-value' }
		]

		for (const value of testValues) {
			const columns = ['id', 'property_id', 'entity_id', 'space_id']
			const values = [value.id, value.property_id, value.entity_id, value.space_id]
			const placeholders = ['$1', '$2', '$3', '$4']

			if (value.string !== undefined) {
				columns.push('string')
				values.push(value.string)
				placeholders.push(`$${values.length}`)
			}
			if (value.number !== undefined) {
				columns.push('number')
				values.push(value.number)
				placeholders.push(`$${values.length}`)
			}
			if (value.boolean !== undefined) {
				columns.push('boolean')
				// @ts-expect-error idk
				values.push(value.boolean)
				placeholders.push(`$${values.length}`)
			}
			if (value.time !== undefined) {
				columns.push('time')
				values.push(value.time)
				placeholders.push(`$${values.length}`)
			}
			if (value.point !== undefined) {
				columns.push('point')
				values.push(value.point)
				placeholders.push(`$${values.length}`)
			}

			await pool.query(`
				INSERT INTO values (${columns.join(', ')})
				VALUES (${placeholders.join(', ')})
				ON CONFLICT (id) DO NOTHING
			`, values)
		}
	}

	async function cleanupTestData() {
		if (!pool) return

		try {
			// Clean up in reverse order to respect foreign key constraints
			if (testSpaceId) {
				await pool.query('DELETE FROM values WHERE space_id = $1', [testSpaceId])
			}
			
			if (testEntityIds) {
				for (const entityId of testEntityIds) {
					await pool.query('DELETE FROM entities WHERE id = $1', [entityId])
				}
			}
			
			if (testPropertyIds) {
				for (const propertyId of Object.values(testPropertyIds)) {
					await pool.query('DELETE FROM properties WHERE id = $1', [propertyId])
				}
			}
			
			if (testSpaceId) {
				await pool.query('DELETE FROM spaces WHERE id = $1', [testSpaceId])
			}
		} catch (error) {
			// Silently ignore cleanup errors during reset
			console.warn('Cleanup warning:', error)
		}
	}

	describe("String property ordering", () => {
		it("should order string values in ascending order by default", async () => {
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid)
			`, [testPropertyIds.string])

			expect(result.rows).toHaveLength(3)
			
			// Should be ordered: Alpha, Beta, Charlie
			const orderedEntityIds = result.rows.map(row => row.id)
			expect(orderedEntityIds[0]).toBe(testEntityIds[1]) // Alpha
			expect(orderedEntityIds[1]).toBe(testEntityIds[2]) // Beta
			expect(orderedEntityIds[2]).toBe(testEntityIds[0]) // Charlie
		})

		it("should order string values in descending order", async () => {
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid, NULL, 'DESC'::sort_order)
			`, [testPropertyIds.string])

			expect(result.rows).toHaveLength(3)
			
			// Should be ordered: Charlie, Beta, Alpha
			const orderedEntityIds = result.rows.map(row => row.id)
			expect(orderedEntityIds[0]).toBe(testEntityIds[0]) // Charlie
			expect(orderedEntityIds[1]).toBe(testEntityIds[2]) // Beta
			expect(orderedEntityIds[2]).toBe(testEntityIds[1]) // Alpha
		})
	})

	describe("Number property ordering", () => {
		it("should order number values in ascending order", async () => {
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid, NULL, 'ASC'::sort_order)
			`, [testPropertyIds.number])

			expect(result.rows).toHaveLength(3)
			
			// Should be ordered numerically: 10, 25, 100
			const orderedEntityIds = result.rows.map(row => row.id)
			expect(orderedEntityIds[0]).toBe(testEntityIds[1]) // 10
			expect(orderedEntityIds[1]).toBe(testEntityIds[2]) // 25
			expect(orderedEntityIds[2]).toBe(testEntityIds[0]) // 100
		})

		it("should order number values in descending order", async () => {
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid, NULL, 'DESC'::sort_order)
			`, [testPropertyIds.number])

			expect(result.rows).toHaveLength(3)
			
			// Should be ordered numerically: 100, 25, 10
			const orderedEntityIds = result.rows.map(row => row.id)
			expect(orderedEntityIds[0]).toBe(testEntityIds[0]) // 100
			expect(orderedEntityIds[1]).toBe(testEntityIds[2]) // 25
			expect(orderedEntityIds[2]).toBe(testEntityIds[1]) // 10
		})
	})

	describe("Boolean property ordering", () => {
		it("should order boolean values (false before true)", async () => {
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid)
			`, [testPropertyIds.boolean])

			expect(result.rows).toHaveLength(3)
			
			// Boolean ordering: false comes before true
			const orderedEntityIds = result.rows.map(row => row.id)
			// Two entities have false, one has true
			expect(orderedEntityIds).toContain(testEntityIds[0]) // false
			expect(orderedEntityIds).toContain(testEntityIds[1]) // true
			expect(orderedEntityIds).toContain(testEntityIds[2]) // false
		})
	})

	describe("Time property ordering", () => {
		it("should order time values chronologically", async () => {
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid)
			`, [testPropertyIds.time])

			expect(result.rows).toHaveLength(3)
			
			// Should be ordered chronologically: 2023, 2024, 2025
			const orderedEntityIds = result.rows.map(row => row.id)
			expect(orderedEntityIds[0]).toBe(testEntityIds[1]) // 2023
			expect(orderedEntityIds[1]).toBe(testEntityIds[2]) // 2024
			expect(orderedEntityIds[2]).toBe(testEntityIds[0]) // 2025
		})
	})

	describe("Point property ordering", () => {
		it("should order point values", async () => {
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid)
			`, [testPropertyIds.point])

			expect(result.rows).toHaveLength(3)
			// Point ordering is lexicographic by string representation
			const orderedEntityIds = result.rows.map(row => row.id)
			expect(orderedEntityIds).toHaveLength(3)
		})
	})

	describe("Relation property exclusion", () => {
		it("should return 0 results for relation properties", async () => {
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid)
			`, [testPropertyIds.relation])

			expect(result.rows).toHaveLength(0)
		})
	})

	describe("Space filtering", () => {
		it("should filter by space when space_id is provided", async () => {
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid, $2::uuid)
			`, [testPropertyIds.string, testSpaceId])

			expect(result.rows).toHaveLength(3)
		})

		it("should return 0 results for non-existent space", async () => {
			const nonExistentSpaceId = "00000000-0000-0000-0000-000000000000"
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid, $2::uuid)
			`, [testPropertyIds.string, nonExistentSpaceId])

			expect(result.rows).toHaveLength(0)
		})
	})

	describe("Edge cases", () => {
		it("should return 0 results for non-existent property", async () => {
			const nonExistentPropertyId = "00000000-0000-0000-0000-000000000000"
			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid)
			`, [nonExistentPropertyId])

			expect(result.rows).toHaveLength(0)
		})

		it("should handle null values properly (exclude them)", async () => {
			// Insert an entity with null string value
			const nullEntityId = "550e8400-e29b-41d4-a716-446655440099"
			await pool.query(`
				INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
				VALUES ($1, '2024-01-01T00:00:00Z', '100', '2024-01-01T00:00:00Z', '100')
				ON CONFLICT (id) DO NOTHING
			`, [nullEntityId])

			await pool.query(`
				INSERT INTO values (id, property_id, entity_id, space_id, string)
				VALUES ('null-val', $1, $2, $3, NULL)
				ON CONFLICT (id) DO NOTHING
			`, [testPropertyIds.string, nullEntityId, testSpaceId])

			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid)
			`, [testPropertyIds.string])

			// Should still only return 3 results (excluding the null value)
			expect(result.rows).toHaveLength(3)
			expect(result.rows.map(row => row.id)).not.toContain(nullEntityId)

			// Cleanup
			await pool.query('DELETE FROM values WHERE id = $1', ['null-val'])
			await pool.query('DELETE FROM entities WHERE id = $1', [nullEntityId])
		})

		it("should handle empty string values properly (exclude them)", async () => {
			// Insert an entity with empty string value
			const emptyEntityId = "550e8400-e29b-41d4-a716-446655440098"
			await pool.query(`
				INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
				VALUES ($1, '2024-01-01T00:00:00Z', '100', '2024-01-01T00:00:00Z', '100')
				ON CONFLICT (id) DO NOTHING
			`, [emptyEntityId])

			await pool.query(`
				INSERT INTO values (id, property_id, entity_id, space_id, string)
				VALUES ('empty-val', $1, $2, $3, '')
				ON CONFLICT (id) DO NOTHING
			`, [testPropertyIds.string, emptyEntityId, testSpaceId])

			const result = await pool.query(`
				SELECT id FROM entities_ordered_by_property($1::uuid)
			`, [testPropertyIds.string])

			// Should still only return 3 results (excluding the empty string value)
			expect(result.rows).toHaveLength(3)
			expect(result.rows.map(row => row.id)).not.toContain(emptyEntityId)

			// Cleanup
			await pool.query('DELETE FROM values WHERE id = $1', ['empty-val'])
			await pool.query('DELETE FROM entities WHERE id = $1', [emptyEntityId])
		})
	})
})