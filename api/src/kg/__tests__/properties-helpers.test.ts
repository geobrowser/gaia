import { describe, expect, it, beforeAll, afterAll, beforeEach, afterEach } from "vitest"
import { drizzle } from "drizzle-orm/node-postgres"
import { Pool } from "pg"
import { sql } from "drizzle-orm"
import { entities, properties, values, relations, spaces } from "../../services/storage/schema"

describe("property helper functions", () => {
	let pool: Pool
	let db: ReturnType<typeof drizzle>
	
	// Test data IDs
	const testPropertyId = '550e8400-e29b-41d4-a716-446655440001'
	const testRelationPropertyId = '550e8400-e29b-41d4-a716-446655440002'
	const testEntityId = '550e8400-e29b-41d4-a716-446655440003'
	const testUnitId = '550e8400-e29b-41d4-a716-446655440004'
	const testRenderableTypeId = '550e8400-e29b-41d4-a716-446655440005'
	const testRelationValueTypeId = '550e8400-e29b-41d4-a716-446655440006'
	const testSpaceId = '550e8400-e29b-41d4-a716-446655440010'

	beforeAll(async () => {
		pool = new Pool({
			connectionString: process.env.DATABASE_URL
		})
		db = drizzle(pool, { 
			casing: "snake_case",
			schema: { entities, properties, values, relations, spaces }
		})
	})

	beforeEach(async () => {
		await setupTestData()
	})

	afterEach(async () => {
		await cleanupTestData()
	})

	afterAll(async () => {
		await pool?.end()
	})

	async function setupTestData() {
		// Insert test space first (required for values and relations)
		await db.insert(spaces).values({
			id: testSpaceId,
			type: 'Public',
			daoAddress: '0x123',
			spaceAddress: '0x456'
		}).onConflictDoNothing()

		// Insert test properties
		await db.insert(properties).values([
			{ id: testPropertyId, type: 'String' },
			{ id: testRelationPropertyId, type: 'Relation' }
		]).onConflictDoNothing()

		// Insert test entities
		await db.insert(entities).values([
			{ 
				id: testEntityId, 
				createdAt: '2023-01-01T00:00:00Z', 
				createdAtBlock: '1', 
				updatedAt: '2023-01-01T00:00:00Z', 
				updatedAtBlock: '1' 
			},
			{ 
				id: testUnitId, 
				createdAt: '2023-01-01T00:00:00Z', 
				createdAtBlock: '1', 
				updatedAt: '2023-01-01T00:00:00Z', 
				updatedAtBlock: '1' 
			},
			{ 
				id: testRenderableTypeId, 
				createdAt: '2023-01-01T00:00:00Z', 
				createdAtBlock: '1', 
				updatedAt: '2023-01-01T00:00:00Z', 
				updatedAtBlock: '1' 
			},
			{ 
				id: testRelationValueTypeId, 
				createdAt: '2023-01-01T00:00:00Z', 
				createdAtBlock: '1', 
				updatedAt: '2023-01-01T00:00:00Z', 
				updatedAtBlock: '1' 
			}
		]).onConflictDoNothing()

		// Insert test values
		await db.insert(values).values([
			{
				id: 'test-val-1',
				entityId: testPropertyId,
				propertyId: 'a126ca53-0c8e-48d5-b888-82c734c38935',
				spaceId: testSpaceId,
				string: 'Test Property Name'
			},
			{
				id: 'test-val-2',
				entityId: testPropertyId,
				propertyId: '9b1f76ff-9711-404c-861e-59dc3fa7d037',
				spaceId: testSpaceId,
				string: 'Test Property Description'
			},
			{
				id: 'test-val-3',
				entityId: testPropertyId,
				propertyId: '396f8c72-dfd0-4b57-91ea-09c1b9321b2f',
				spaceId: testSpaceId,
				string: 'Test Format'
			}
		]).onConflictDoNothing()

		// Insert test relations
		await db.insert(relations).values([
			{
				id: '550e8400-e29b-41d4-a716-446655440021',
				entityId: testPropertyId,
				typeId: '11b06581-20d3-41ea-b570-2ef4ee0a4ffd',
				fromEntityId: testPropertyId,
				toEntityId: testUnitId,
				spaceId: testSpaceId
			},
			{
				id: '550e8400-e29b-41d4-a716-446655440022',
				entityId: testPropertyId,
				typeId: '2316bbe1-c76f-4635-83f2-3e03b4f1fe46',
				fromEntityId: testPropertyId,
				toEntityId: testRenderableTypeId,
				spaceId: testSpaceId
			},
			{
				id: '550e8400-e29b-41d4-a716-446655440023',
				entityId: testRelationPropertyId,
				typeId: '9eea393f-17dd-4971-a62e-a603e8bfec20',
				fromEntityId: testRelationPropertyId,
				toEntityId: testRelationValueTypeId,
				spaceId: testSpaceId
			}
		]).onConflictDoNothing()
	}

	async function cleanupTestData() {
		// Delete in reverse order to respect foreign keys using Drizzle ORM
		await db.delete(relations)
		await db.delete(values)
		await db.delete(entities)
		await db.delete(properties)
		await db.delete(spaces)
	}

	describe("properties_name function", () => {
		it("should be callable and return string or null", async () => {
			const result = await db.execute(sql`
				SELECT properties_name(p) as name
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(typeof result.rows[0]?.name === 'string' || result.rows[0]?.name === null).toBe(true)
		})

		it("should return name when property has name value", async () => {
			const result = await db.execute(sql`
				SELECT properties_name(p) as name
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0]?.name).toBe('Test Property Name')
		})
	})

	describe("properties_description function", () => {
		it("should be callable and return string or null", async () => {
			const result = await db.execute(sql`
				SELECT properties_description(p) as description
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(typeof result.rows[0]?.description === 'string' || result.rows[0]?.description === null).toBe(true)
		})

		it("should return description when property has description value", async () => {
			const result = await db.execute(sql`
				SELECT properties_description(p) as description
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0]?.description).toBe('Test Property Description')
		})
	})

	describe("properties_renderable_type function", () => {
		it("should be callable and return UUID or null", async () => {
			const result = await db.execute(sql`
				SELECT properties_renderable_type(p) as renderable_type
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			// Should return UUID string or null
			const renderableType = result.rows[0]?.renderable_type
			expect(typeof renderableType === 'string' || renderableType === null).toBe(true)
		})

		it("should return renderable type for property with renderable type relation", async () => {
			const result = await db.execute(sql`
				SELECT properties_renderable_type(p) as renderable_type
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0]?.renderable_type).toBe(testRenderableTypeId)
		})

		it("should return null for system name property", async () => {
			// Insert system name property for this test
			await db.execute(sql`
				INSERT INTO properties (id, type) 
				VALUES ('a126ca53-0c8e-48d5-b888-82c734c38935', 'String')
				ON CONFLICT (id) DO NOTHING
			`)

			const result = await db.execute(sql`
				SELECT properties_renderable_type(p) as renderable_type
				FROM properties p 
				WHERE id = ${'a126ca53-0c8e-48d5-b888-82c734c38935'}
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0]?.renderable_type).toBeNull()
		})

		it("should return null for system description property", async () => {
			// Insert system description property for this test
			await db.execute(sql`
				INSERT INTO properties (id, type) 
				VALUES ('9b1f76ff-9711-404c-861e-59dc3fa7d037', 'String')
				ON CONFLICT (id) DO NOTHING
			`)

			const result = await db.execute(sql`
				SELECT properties_renderable_type(p) as renderable_type
				FROM properties p 
				WHERE id = ${'9b1f76ff-9711-404c-861e-59dc3fa7d037'}
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0]?.renderable_type).toBeNull()
		})
	})

	describe("properties_unit function", () => {
		it("should be callable and return UUID or null", async () => {
			const result = await db.execute(sql`
				SELECT properties_unit(p) as unit
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			// Should return UUID string or null
			const unit = result.rows[0]?.unit
			expect(typeof unit === 'string' || unit === null).toBe(true)
		})

		it("should return unit for property with unit relation", async () => {
			const result = await db.execute(sql`
				SELECT properties_unit(p) as unit
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0]?.unit).toBe(testUnitId)
		})
	})

	describe("properties_format function", () => {
		it("should be callable and return string or null", async () => {
			const result = await db.execute(sql`
				SELECT properties_format(p) as format
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(typeof result.rows[0]?.format === 'string' || result.rows[0]?.format === null).toBe(true)
		})

		it("should return format when property has format value", async () => {
			const result = await db.execute(sql`
				SELECT properties_format(p) as format
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0]?.format).toBe('Test Format')
		})
	})

	describe("properties_relation_value_types function", () => {
		it("should be callable and return entity objects", async () => {
			const result = await db.execute(sql`
				SELECT properties_relation_value_types(p) as value_types
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			// Should be callable without error
			expect(result.rows).toBeDefined()
		})

		it("should return relation value types for relation property", async () => {
			const result = await db.execute(sql`
				SELECT (properties_relation_value_types(p)).id as value_type_id
				FROM properties p 
				WHERE id = ${testRelationPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0]?.value_type_id).toBe(testRelationValueTypeId)
		})

		it("should return empty for system name property", async () => {
			// Insert system name property for this test
			await db.execute(sql`
				INSERT INTO properties (id, type) 
				VALUES ('a126ca53-0c8e-48d5-b888-82c734c38935', 'String')
				ON CONFLICT (id) DO NOTHING
			`)

			const result = await db.execute(sql`
				SELECT properties_relation_value_types(p) as value_types
				FROM properties p 
				WHERE id = ${'a126ca53-0c8e-48d5-b888-82c734c38935'}
			`)

			expect(result.rows).toHaveLength(0)
		})

		it("should return empty for system description property", async () => {
			// Insert system description property for this test
			await db.execute(sql`
				INSERT INTO properties (id, type) 
				VALUES ('9b1f76ff-9711-404c-861e-59dc3fa7d037', 'String')
				ON CONFLICT (id) DO NOTHING
			`)

			const result = await db.execute(sql`
				SELECT properties_relation_value_types(p) as value_types
				FROM properties p 
				WHERE id = ${'9b1f76ff-9711-404c-861e-59dc3fa7d037'}
			`)

			expect(result.rows).toHaveLength(0)
		})
	})

	describe("properties_relation_value_type_ids function", () => {
		it("should be callable and return array or null", async () => {
			const result = await db.execute(sql`
				SELECT properties_relation_value_type_ids(p) as value_type_ids
				FROM properties p 
				WHERE id = ${testPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(Array.isArray(result.rows[0]?.value_type_ids) || result.rows[0]?.value_type_ids === null).toBe(true)
		})

		it("should return relation value type ids for relation property", async () => {
			const result = await db.execute(sql`
				SELECT properties_relation_value_type_ids(p) as value_type_ids
				FROM properties p 
				WHERE id = ${testRelationPropertyId}
			`)

			expect(result.rows).toHaveLength(1)
			expect(Array.isArray(result.rows[0]?.value_type_ids)).toBe(true)
			expect(result.rows[0]?.value_type_ids).toContain(testRelationValueTypeId)
		})

		it("should return null for system name property", async () => {
			// Insert system name property for this test
			await db.execute(sql`
				INSERT INTO properties (id, type) 
				VALUES ('a126ca53-0c8e-48d5-b888-82c734c38935', 'String')
				ON CONFLICT (id) DO NOTHING
			`)

			const result = await db.execute(sql`
				SELECT properties_relation_value_type_ids(p) as value_type_ids
				FROM properties p 
				WHERE id = ${'a126ca53-0c8e-48d5-b888-82c734c38935'}
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0]?.value_type_ids).toBeNull()
		})

		it("should return null for system description property", async () => {
			// Insert system description property for this test
			await db.execute(sql`
				INSERT INTO properties (id, type) 
				VALUES ('9b1f76ff-9711-404c-861e-59dc3fa7d037', 'String')
				ON CONFLICT (id) DO NOTHING
			`)

			const result = await db.execute(sql`
				SELECT properties_relation_value_type_ids(p) as value_type_ids
				FROM properties p 
				WHERE id = ${'9b1f76ff-9711-404c-861e-59dc3fa7d037'}
			`)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0]?.value_type_ids).toBeNull()
		})
	})
})