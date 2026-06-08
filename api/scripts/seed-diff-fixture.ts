/**
 * Seed a minimal diff fixture for verifying the v2 endpoint locally.
 *
 * Scenario:
 *   edit1 (version_key=100) — creates `image` entity:
 *     - TYPES_PROPERTY relation → IMAGE_TYPE
 *     - IMAGE_URL_PROPERTY value = "https://example.com/hero.jpg"
 *   edit2 (version_key=200) — creates `page` entity:
 *     - "cover" relation (custom relation type) → image entity
 *
 * Diff page entity from edit1 → edit2 should show the cover relation as added.
 * v2 enrichment should inline `imageUrl` on that relation's `after`.
 */

import {randomUUID} from "node:crypto"
import {Pool} from "pg"

const DATABASE_URL = process.env.DATABASE_URL
if (!DATABASE_URL) {
	console.error("DATABASE_URL is required")
	process.exit(1)
}

// System property/type IDs from grc-20 SDK
const IMAGE_TYPE = "ba4e4146-0010-499d-a0a3-caaa7f579d0e"
const IMAGE_URL_PROPERTY = "8a743832-c094-4a62-b665-0c3cc2f9c7bc"
const TYPES_PROPERTY = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"

// Fixture IDs — predictable, prefix 30000000 for easy cleanup
const SPACE = "30000000-0000-4000-8000-000000000001"
const PAGE = "30000000-0000-4000-8000-000000000100"
const IMAGE = "30000000-0000-4000-8000-000000000200"
const COVER_REL_TYPE = "30000000-0000-4000-8000-000000000300"
const EDIT1 = "30000000-0000-4000-8000-000000000a01"
const EDIT2 = "30000000-0000-4000-8000-000000000a02"
const VKEY1 = 100
const VKEY2 = 200

const IMAGE_URL = "https://example.com/hero.jpg"
const ENTITY_TIMES = {
	created_at: "2026-05-25T00:00:00Z",
	created_at_block: "1000",
	updated_at: "2026-05-25T00:00:00Z",
	updated_at_block: "1001",
}

async function main() {
	const pool = new Pool({connectionString: DATABASE_URL})
	const client = await pool.connect()
	try {
		await client.query("BEGIN")

		// Space
		await client.query(`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xDiffFixture') ON CONFLICT DO NOTHING`, [SPACE])

		// Entities
		for (const id of [PAGE, IMAGE, COVER_REL_TYPE]) {
			await client.query(
				`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
				 VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING`,
				[id, ENTITY_TIMES.created_at, ENTITY_TIMES.created_at_block, ENTITY_TIMES.updated_at, ENTITY_TIMES.updated_at_block],
			)
		}

		// Edits
		await client.query(
			`INSERT INTO edit_versions (edit_id, block_number, sequence, version_key, created_at, name)
			 VALUES ($1, 1000, 0, $2, '2026-05-25T00:00:00Z', 'Create image entity') ON CONFLICT DO NOTHING`,
			[EDIT1, VKEY1],
		)
		await client.query(
			`INSERT INTO edit_versions (edit_id, block_number, sequence, version_key, created_at, name)
			 VALUES ($1, 1001, 0, $2, '2026-05-25T00:00:01Z', 'Create page with cover image') ON CONFLICT DO NOTHING`,
			[EDIT2, VKEY2],
		)

		// Versioned: image entity at edit1
		const imageTypeRelVerId = randomUUID()
		const imageTypeRelId = randomUUID()
		await client.query(
			`INSERT INTO relation_versions
			 (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, NULL)
			 ON CONFLICT DO NOTHING`,
			[imageTypeRelVerId, imageTypeRelId, IMAGE, TYPES_PROPERTY, IMAGE_TYPE, SPACE, VKEY1],
		)

		const imageUrlValueId = randomUUID()
		await client.query(
			`INSERT INTO value_versions
			 (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, $6)
			 ON CONFLICT DO NOTHING`,
			[imageUrlValueId, IMAGE, IMAGE_URL_PROPERTY, SPACE, VKEY1, IMAGE_URL],
		)

		// Versioned: page entity at edit2 — cover relation → image
		const coverRelVerId = randomUUID()
		const coverRelId = randomUUID()
		await client.query(
			`INSERT INTO relation_versions
			 (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, NULL)
			 ON CONFLICT DO NOTHING`,
			[coverRelVerId, coverRelId, PAGE, COVER_REL_TYPE, IMAGE, SPACE, VKEY2],
		)

		// Live state — mirrors latest version
		await client.query(
			`INSERT INTO relations
			 (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
			 VALUES ($1, $2, $3, $2, $4, $5, false)
			 ON CONFLICT DO NOTHING`,
			[imageTypeRelId, IMAGE, TYPES_PROPERTY, IMAGE_TYPE, SPACE],
		)
		await client.query(
			`INSERT INTO relations
			 (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
			 VALUES ($1, $2, $3, $2, $4, $5, false)
			 ON CONFLICT DO NOTHING`,
			[coverRelId, PAGE, COVER_REL_TYPE, IMAGE, SPACE],
		)
		await client.query(
			`INSERT INTO "values"
			 (id, property_id, entity_id, space_id, text)
			 VALUES ($1, $2, $3, $4, $5)
			 ON CONFLICT DO NOTHING`,
			[`val-${IMAGE}-${IMAGE_URL_PROPERTY}`, IMAGE_URL_PROPERTY, IMAGE, SPACE, IMAGE_URL],
		)

		await client.query("COMMIT")
		console.log("✅ Seed complete")
		console.log("")
		console.log("To smoke-test v2 endpoint:")
		console.log("")
		console.log(`  curl -s "http://localhost:3000/v2/versioned/entities/${PAGE}/diff?fromEditId=${EDIT1}&toEditId=${EDIT2}&spaceId=${SPACE}" | jq`)
		console.log("")
		console.log("Expected: relation pointing at image entity carries `imageUrl` on `after`.")
	} catch (err) {
		await client.query("ROLLBACK")
		console.error("Seed failed:", err)
		process.exit(1)
	} finally {
		client.release()
		await pool.end()
	}
}

main()
