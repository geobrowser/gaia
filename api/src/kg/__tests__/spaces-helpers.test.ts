import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"

const SYSTEM_TYPES_ID = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"
const SYSTEM_SPACE_TYPE_ID = "362c1dbd-dc64-44bb-a3c4-652f38a642d7"

const TOPIC_WITH_LEGACY_SPACE = "00000000-cafe-4caf-8afe-000000000001"
const TOPIC_ONLY_SPACE = "00000000-cafe-4caf-8afe-000000000002"
const NULL_TOPIC_LEGACY_SPACE = "00000000-cafe-4caf-8afe-000000000003"
const NULL_TOPIC_EMPTY_SPACE = "00000000-cafe-4caf-8afe-000000000004"

const TOPIC_ENTITY = "00000000-feed-4fee-8eed-000000000001"
const TOPIC_ONLY_ENTITY = "00000000-feed-4fee-8eed-000000000002"
const LEGACY_ENTITY = "00000000-feed-4fee-8eed-000000000003"
const NULL_TOPIC_LEGACY_ENTITY = "00000000-feed-4fee-8eed-000000000004"
const LEGACY_RELATION_ENTITY = "00000000-feed-4fee-8eed-000000000005"
const NULL_TOPIC_LEGACY_RELATION_ENTITY = "00000000-feed-4fee-8eed-000000000006"

const LEGACY_RELATION = "00000000-f00d-4f00-800d-000000000001"
const NULL_TOPIC_LEGACY_RELATION = "00000000-f00d-4f00-800d-000000000002"

const TEST_SPACE_IDS = [TOPIC_WITH_LEGACY_SPACE, TOPIC_ONLY_SPACE, NULL_TOPIC_LEGACY_SPACE, NULL_TOPIC_EMPTY_SPACE]
const TEST_ENTITY_IDS = [
	TOPIC_ENTITY,
	TOPIC_ONLY_ENTITY,
	LEGACY_ENTITY,
	NULL_TOPIC_LEGACY_ENTITY,
	LEGACY_RELATION_ENTITY,
	NULL_TOPIC_LEGACY_RELATION_ENTITY,
]
const TEST_RELATION_IDS = [LEGACY_RELATION, NULL_TOPIC_LEGACY_RELATION]

async function cleanupFixtures(pool: Pool) {
	await pool.query(
		`
			DELETE FROM relations
			WHERE id = ANY($1::uuid[])
			   OR space_id = ANY($2::uuid[])
			   OR from_entity_id = ANY($3::uuid[])
			   OR entity_id = ANY($3::uuid[])
		`,
		[TEST_RELATION_IDS, TEST_SPACE_IDS, TEST_ENTITY_IDS],
	)
	await pool.query(`DELETE FROM spaces WHERE id = ANY($1::uuid[])`, [TEST_SPACE_IDS])
	await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [TEST_ENTITY_IDS])
}

async function seedFixtures(pool: Pool) {
	await pool.query(
		`
			INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			SELECT id, '0', '0', '0', '0'
			FROM unnest($1::uuid[]) AS t(id)
			ON CONFLICT (id) DO NOTHING
		`,
		[TEST_ENTITY_IDS],
	)

	await pool.query(
		`
			INSERT INTO spaces (id, type, address, topic_id) VALUES
				($1, 'DAO', '0xSpaceTopicWithLegacy', $5),
				($2, 'DAO', '0xSpaceTopicOnly', $6),
				($3, 'DAO', '0xSpaceNullTopicLegacy', NULL),
				($4, 'DAO', '0xSpaceNullTopicEmpty', NULL)
		`,
		[
			TOPIC_WITH_LEGACY_SPACE,
			TOPIC_ONLY_SPACE,
			NULL_TOPIC_LEGACY_SPACE,
			NULL_TOPIC_EMPTY_SPACE,
			TOPIC_ENTITY,
			TOPIC_ONLY_ENTITY,
		],
	)

	await pool.query(
		`
			INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) VALUES
				($1, $3, $5, $6, $7, $8, true),
				($2, $4, $5, $9, $7, $10, true)
		`,
		[
			LEGACY_RELATION,
			NULL_TOPIC_LEGACY_RELATION,
			LEGACY_RELATION_ENTITY,
			NULL_TOPIC_LEGACY_RELATION_ENTITY,
			SYSTEM_TYPES_ID,
			LEGACY_ENTITY,
			SYSTEM_SPACE_TYPE_ID,
			TOPIC_WITH_LEGACY_SPACE,
			NULL_TOPIC_LEGACY_ENTITY,
			NULL_TOPIC_LEGACY_SPACE,
		],
	)
}

describe("space helper functions", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({
			connectionString: process.env.DATABASE_URL,
		})

		await cleanupFixtures(pool)
		await seedFixtures(pool)
	})

	afterAll(async () => {
		if (pool) {
			await cleanupFixtures(pool)
			await pool.end()
		}
	})

	describe("spaces_page function", () => {
		it("returns the topic entity when topic_id is set and legacy relation disagrees", async () => {
			const result = await pool.query(
				`
					SELECT (spaces_page(s)).id::text as page_id
					FROM spaces s
					WHERE id = $1
				`,
				[TOPIC_WITH_LEGACY_SPACE],
			)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0].page_id).toBe(TOPIC_ENTITY)
			expect(result.rows[0].page_id).not.toBe(LEGACY_ENTITY)
		})

		it("returns the topic entity when topic_id is set and no legacy relation exists", async () => {
			const result = await pool.query(
				`
					SELECT (spaces_page(s)).id::text as page_id
					FROM spaces s
					WHERE id = $1
				`,
				[TOPIC_ONLY_SPACE],
			)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0].page_id).toBe(TOPIC_ONLY_ENTITY)
		})

		it("falls back to the legacy front-page relation when topic_id is null", async () => {
			const result = await pool.query(
				`
					SELECT (spaces_page(s)).id::text as page_id
					FROM spaces s
					WHERE id = $1
				`,
				[NULL_TOPIC_LEGACY_SPACE],
			)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0].page_id).toBe(NULL_TOPIC_LEGACY_ENTITY)
		})

		it("returns null when topic_id is null and no legacy front-page relation exists", async () => {
			const result = await pool.query(
				`
					SELECT (spaces_page(s)).id::text as page_id
					FROM spaces s
					WHERE id = $1
				`,
				[NULL_TOPIC_EMPTY_SPACE],
			)

			expect(result.rows).toHaveLength(1)
			expect(result.rows[0].page_id).toBeNull()
		})
	})
})
