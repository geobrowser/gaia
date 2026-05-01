import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"

const TYPES_RELATION_ID = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"
const SPACE_TYPE_ID = "362c1dbd-dc64-44bb-a3c4-652f38a642d7"

describe("space helper functions", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({
			connectionString: process.env.DATABASE_URL,
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
				const result = await pool.query(
					`
					SELECT spaces_page(s) as page
					FROM spaces s 
					WHERE id = $1
				`,
					[spaceId],
				)

				expect(result.rows).toHaveLength(1)
				// Should be callable without error - the function can return null or an entity
				expect(result.rows[0]).toHaveProperty("page")
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
				ORDER BY e.created_at::numeric ASC, e.id ASC
				LIMIT 1
			`)

			if (result.rows.length > 0) {
				const spaceId = result.rows[0].space_id
				const expectedEntityId = result.rows[0].entity_id

				const pageResult = await pool.query(
					`
					SELECT spaces_page(s) as page
					FROM spaces s 
					WHERE id = $1
				`,
					[spaceId],
				)

				expect(pageResult.rows).toHaveLength(1)
				const page = pageResult.rows[0].page
				if (page && typeof page === "object" && "id" in page) {
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

				const pageResult = await pool.query(
					`
					SELECT spaces_page(s) as page
					FROM spaces s 
					WHERE id = $1
				`,
					[spaceId],
				)

				expect(pageResult.rows).toHaveLength(1)
				expect(pageResult.rows[0].page).toBeNull()
			}
		})

		it("should return the earliest created front page entity when multiple candidates exist", async () => {
			const spaceId = crypto.randomUUID()
			const earlierPageId = crypto.randomUUID()
			const laterPageId = crypto.randomUUID()
			const earlierRelationId = crypto.randomUUID()
			const laterRelationId = crypto.randomUUID()
			const entityIds = [earlierPageId, laterPageId, earlierRelationId, laterRelationId]

			try {
				await pool.query("INSERT INTO spaces (id, type, address) VALUES ($1, 'Personal', $2)", [
					spaceId,
					`0x${spaceId.replaceAll("-", "").slice(0, 40)}`,
				])
				await pool.query(
					`
					INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
					VALUES
						($1, '100', '1', '100', '1'),
						($2, '200', '2', '200', '2'),
						($3, '300', '3', '300', '3'),
						($4, '400', '4', '400', '4')
				`,
					entityIds,
				)
				await pool.query(
					`
					INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, verified)
					VALUES
						($1, $1, $2, $3, $4, $5, true),
						($6, $6, $2, $7, $4, $5, true)
				`,
					[
						laterRelationId,
						TYPES_RELATION_ID,
						laterPageId,
						SPACE_TYPE_ID,
						spaceId,
						earlierRelationId,
						earlierPageId,
					],
				)

				const result = await pool.query(
					`
					SELECT (spaces_page(s)).id AS page_id
					FROM spaces s
					WHERE id = $1
				`,
					[spaceId],
				)

				expect(result.rows).toHaveLength(1)
				expect(result.rows[0].page_id).toBe(earlierPageId)
			} finally {
				await pool.query("DELETE FROM relations WHERE id = ANY($1::uuid[])", [
					[earlierRelationId, laterRelationId],
				])
				await pool.query("DELETE FROM spaces WHERE id = $1", [spaceId])
				await pool.query("DELETE FROM entities WHERE id = ANY($1::uuid[])", [entityIds])
			}
		})
	})
})
