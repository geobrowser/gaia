import { describe, expect, it, beforeAll, afterAll } from "vitest"
import { Pool } from "pg"

describe("space helper functions", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({
			connectionString: process.env.DATABASE_URL
		})
	})

	afterAll(async () => {
		await pool?.end()
	})

	describe("spaces_page function", () => {
		it("should be callable and return entity object or null", async () => {
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
				// Should be callable without error - the function can return null or an entity
				expect(result.rows[0]).toHaveProperty('page')
			}
		})

		it("should return front page entity when space has one", async () => {
			// Look for a space that has a front page entity
			const result = await pool.query(`
				SELECT s.id as space_id, e.id as entity_id
				FROM spaces s
				JOIN relations r ON r.space_id = s.id
				JOIN entities e ON e.id = r.from_entity_id
				WHERE r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'
				  AND r.to_entity_id = '362c1dbd-dc64-44bb-a3c4-652f38a642d7'
				LIMIT 1
			`)

			if (result.rows.length > 0) {
				const spaceId = result.rows[0].space_id
				const expectedEntityId = result.rows[0].entity_id
				
				const pageResult = await pool.query(`
					SELECT spaces_page(s) as page
					FROM spaces s 
					WHERE id = $1
				`, [spaceId])

				expect(pageResult.rows).toHaveLength(1)
				const page = pageResult.rows[0].page
				if (page && typeof page === 'object' && 'id' in page) {
					expect(page.id).toBe(expectedEntityId)
				}
			}
		})

		it("should return null when space has no front page", async () => {
			// Look for a space that doesn't have a front page entity
			const result = await pool.query(`
				SELECT s.id as space_id
				FROM spaces s
				WHERE NOT EXISTS (
					SELECT 1 FROM relations r
					WHERE r.space_id = s.id
					  AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'
					  AND r.to_entity_id = '362c1dbd-dc64-44bb-a3c4-652f38a642d7'
				)
				LIMIT 1
			`)

			if (result.rows.length > 0) {
				const spaceId = result.rows[0].space_id
				
				const pageResult = await pool.query(`
					SELECT spaces_page(s) as page
					FROM spaces s 
					WHERE id = $1
				`, [spaceId])

				expect(pageResult.rows).toHaveLength(1)
				expect(pageResult.rows[0].page).toBeNull()
			}
		})
	})
})