/**
 * Integration tests for the proposal diff endpoint using real GRC-20 edits.
 *
 * These tests exercise the full GRC-20 edit flow:
 * 1. Create GRC-20 edit with ops (createEntity, updateEntity, createRelation, etc.)
 * 2. Encode it with `encodeEditAuto`
 * 3. Write to `ipfs_cache` table
 * 4. Create proposal with `Publish` action pointing to that URI
 * 5. Call `/versioned/proposals/:id/diff` endpoint
 * 6. Verify the computed diff matches expected changes
 *
 * This validates that the proposal-diff.ts code correctly:
 * - Fetches edit blobs from `ipfs_cache`
 * - Decodes with `decodeEditAuto` from `@geoprotocol/grc-20`
 * - Extracts affected entities from ops
 * - Applies ops to base state to get proposed state
 * - Computes accurate diffs
 *
 * Prerequisites:
 * - PostgreSQL running (docker-compose up -d)
 * - DATABASE_URL environment variable set
 * - Migrations applied (bun run db:migrate)
 */

import {EditBuilder, encodeEdit, type Id, parseId, randomId} from "@geoprotocol/grc-20"
import {SystemIds} from "@graphprotocol/grc-20"
import {drizzle} from "drizzle-orm/node-postgres"
import {Hono} from "hono"
import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {runtime} from "../../services/runtime"
import {normalizeUuid} from "../../utils/uuid"
import {createVersionedRouter} from "../router"

/** Shorthand to normalize a UUID for response-body assertions. */
const n = normalizeUuid

// Skip integration tests if DATABASE_URL is not set
const DATABASE_URL = process.env.DATABASE_URL
const SKIP_INTEGRATION = !DATABASE_URL

// ============================================================================
// Test UUIDs - Using prefix 20000000-* for easy identification and cleanup
// ============================================================================

const uuid = {
	// Spaces
	space1: "20000000-0001-4000-8000-000000000001",

	// Existing entities in the database (for update/delete tests)
	entityExisting1: "20000000-0002-4000-8000-000000000001",
	entityExisting2: "20000000-0002-4000-8000-000000000002",
	entityExistingWithRelations: "20000000-0002-4000-8000-000000000003",
	entityToDelete: "20000000-0002-4000-8000-000000000004",

	// New entities (will be created via proposals)
	entityNew1: "20000000-0002-4000-8000-000000000010",
	entityNew2: "20000000-0002-4000-8000-000000000011",

	// Properties (custom test properties)
	propText: "20000000-0004-4000-8000-000000000001",
	propBool: "20000000-0004-4000-8000-000000000002",
	propInt: "20000000-0004-4000-8000-000000000003",
	propFloat: "20000000-0004-4000-8000-000000000004",
	propName: SystemIds.NAME_PROPERTY,

	// Relation types
	relTypeGeneric: "20000000-0005-4000-8000-000000000001",
	relTypeBlocks: SystemIds.BLOCKS,

	// Relations (for existing data)
	rel1: "20000000-0006-4000-8000-000000000001",
	rel2: "20000000-0006-4000-8000-000000000002",
	relToDelete: "20000000-0006-4000-8000-000000000003",
	relToUpdate: "20000000-0006-4000-8000-000000000004",

	// Edit IDs (versions)
	edit1: "20000000-0007-4000-8000-000000000001",

	// Proposals
	proposalCreateEntity: "20000000-0008-4000-8000-000000000001",
	proposalUpdateEntity: "20000000-0008-4000-8000-000000000002",
	proposalDeleteEntity: "20000000-0008-4000-8000-000000000003",
	proposalCreateRelation: "20000000-0008-4000-8000-000000000004",
	proposalMultipleOps: "20000000-0008-4000-8000-000000000005",
	proposalAllValueTypes: "20000000-0008-4000-8000-000000000006",
	proposalClosed: "20000000-0008-4000-8000-000000000007",
	proposalDeleteRelation: "20000000-0008-4000-8000-000000000008",
	proposalUpdateRelation: "20000000-0008-4000-8000-000000000009",

	// Blocks-related entities
	entityWithBlocks: "20000000-0002-4000-8000-000000000020",
	blockEntity1: "20000000-0002-4000-8000-000000000021",
	blocksRelation1: "20000000-0006-4000-8000-000000000010",
	blockTypesRelation1: "20000000-0006-4000-8000-000000000011",

	// GRC-20 system IDs for blocks
	typesProperty: SystemIds.TYPES_PROPERTY,
	textBlockType: SystemIds.TEXT_BLOCK,
	markdownContent: SystemIds.MARKDOWN_CONTENT,

	// Proposals (continued)
	proposalDeleteBlock: "20000000-0008-4000-8000-000000000010",
	proposalExecuted: "20000000-0008-4000-8000-000000000011",

	// Proposal actions
	actionCreateEntity: "20000000-000a-4000-8000-000000000001",
	actionUpdateEntity: "20000000-000a-4000-8000-000000000002",
	actionDeleteEntity: "20000000-000a-4000-8000-000000000003",
	actionCreateRelation: "20000000-000a-4000-8000-000000000004",
	actionMultipleOps: "20000000-000a-4000-8000-000000000005",
	actionAllValueTypes: "20000000-000a-4000-8000-000000000006",
	actionClosed: "20000000-000a-4000-8000-000000000007",
	actionDeleteRelation: "20000000-000a-4000-8000-000000000008",
	actionUpdateRelation: "20000000-000a-4000-8000-000000000009",
	actionDeleteBlock: "20000000-000a-4000-8000-000000000010",
	actionExecuted: "20000000-000a-4000-8000-000000000011",

	// Value IDs (for value_versions)
	val: (n: number) => `20000000-0009-4000-8000-${n.toString().padStart(12, "0")}`,
}

// Version key: (block_number << 32) | sequence
const versionKey1 = (BigInt(2000) << BigInt(32)).toString()

// Helper to convert UUID string to Id (branded 16-byte Uint8Array)
function uuidToId(uuidStr: string): Id {
	const id = parseId(uuidStr)
	if (!id) {
		throw new Error(`Invalid UUID: ${uuidStr}`)
	}
	return id
}

// Helper to generate IPFS-like URI
function generateTestUri(suffix: string): string {
	return `ipfs://bafkreitest${suffix}`
}

// ============================================================================
// Test Suite
// ============================================================================

describe.skipIf(SKIP_INTEGRATION)("Proposal Diff - Full GRC-20 Edit Flow", () => {
	let pool: Pool
	let app: Hono

	beforeAll(async () => {
		pool = new Pool({connectionString: DATABASE_URL})
		const db = drizzle(pool)
		app = new Hono()
		app.route("/versioned", createVersionedRouter(db as any, runtime))

		try {
			await setupTestData(pool)
		} catch (error) {
			console.error("Setup failed:", error)
			throw error
		}
	})

	afterAll(async () => {
		await cleanupTestData(pool)
		await pool?.end()
	})

	// ==========================================================================
	// 1. CreateEntity Op Tests
	// ==========================================================================

	describe("CreateEntity Op", () => {
		it("returns ADD diff for new entity with text value", async () => {
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalCreateEntity}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)

			const body = await res.json()
			expect(body.entities).toBeDefined()
			expect(body.entities.length).toBeGreaterThan(0)

			// Find the diff for our new entity
			const entityDiff = body.entities.find((d: any) => d.entityId === n(uuid.entityNew1))
			expect(entityDiff).toBeDefined()

			// Should have ADD for the text value (before=null, after has value)
			const textValueDiff = entityDiff.values.find((v: any) => v.propertyId === n(uuid.propText))
			expect(textValueDiff).toBeDefined()
			expect(textValueDiff.type).toBe("TEXT")
			expect(textValueDiff.before).toBeNull()
			expect(textValueDiff.after).toBe("New Entity Title")
		})

		it("returns ADD diff for new entity with multiple values", async () => {
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalAllValueTypes}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)

			const body = await res.json()
			const entityDiff = body.entities.find((d: any) => d.entityId === n(uuid.entityNew2))
			expect(entityDiff).toBeDefined()

			// Should have multiple value diffs
			expect(entityDiff.values.length).toBeGreaterThanOrEqual(4)

			// Check text value (ADD: before=null, after has value)
			const textDiff = entityDiff.values.find((v: any) => v.propertyId === n(uuid.propText))
			expect(textDiff?.type).toBe("TEXT")
			expect(textDiff?.before).toBeNull()
			expect(textDiff?.after).toBe("Entity with all types")

			// Check bool value
			const boolDiff = entityDiff.values.find((v: any) => v.propertyId === n(uuid.propBool))
			expect(boolDiff?.type).toBe("BOOL")
			expect(boolDiff?.before).toBeNull()
			expect(boolDiff?.after).toBe("true")

			// Check int value
			const intDiff = entityDiff.values.find((v: any) => v.propertyId === n(uuid.propInt))
			expect(intDiff?.type).toBe("INT64")
			expect(intDiff?.before).toBeNull()
			expect(intDiff?.after).toBe("42")

			// Check float value
			const floatDiff = entityDiff.values.find((v: any) => v.propertyId === n(uuid.propFloat))
			expect(floatDiff?.type).toBe("FLOAT64")
			expect(floatDiff?.before).toBeNull()
			expect(Number(floatDiff?.after)).toBeCloseTo(3.14159, 4)
		})
	})

	// ==========================================================================
	// 2. UpdateEntity Op Tests
	// ==========================================================================

	describe("UpdateEntity Op", () => {
		it("returns UPDATE diff when modifying existing value", async () => {
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalUpdateEntity}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)

			const body = await res.json()
			const entityDiff = body.entities.find((d: any) => d.entityId === n(uuid.entityExisting1))
			expect(entityDiff).toBeDefined()

			// Should have UPDATE for the text value (both before and after have values)
			const textValueDiff = entityDiff.values.find((v: any) => v.propertyId === n(uuid.propText))
			expect(textValueDiff).toBeDefined()
			expect(textValueDiff.type).toBe("TEXT")
			expect(textValueDiff.before).toBe("Original Text")
			expect(textValueDiff.after).toBe("Updated Text")
		})

		it("returns ADD diff when setting new property on existing entity", async () => {
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalUpdateEntity}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)

			const body = await res.json()
			const entityDiff = body.entities.find((d: any) => d.entityId === n(uuid.entityExisting1))

			// Should have ADD for the new bool value (entity didn't have it before)
			const boolValueDiff = entityDiff.values.find((v: any) => v.propertyId === n(uuid.propBool))
			expect(boolValueDiff).toBeDefined()
			expect(boolValueDiff.type).toBe("BOOL")
			expect(boolValueDiff.before).toBeNull()
			expect(boolValueDiff.after).toBe("true")
		})
	})

	// ==========================================================================
	// 3. DeleteEntity Op Tests
	// ==========================================================================

	describe("DeleteEntity Op", () => {
		it("returns REMOVE diff for all values on deleted entity", async () => {
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalDeleteEntity}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)

			const body = await res.json()
			const entityDiff = body.entities.find((d: any) => d.entityId === n(uuid.entityToDelete))
			expect(entityDiff).toBeDefined()

			// Should have REMOVE for all existing values (before has value, after=null)
			const textValueDiff = entityDiff.values.find((v: any) => v.propertyId === n(uuid.propText))
			expect(textValueDiff).toBeDefined()
			expect(textValueDiff.type).toBe("TEXT")
			expect(textValueDiff.before).toBe("Entity to be deleted")
			expect(textValueDiff.after).toBeNull()
		})
	})

	// ==========================================================================
	// 4. CreateRelation Op Tests
	// ==========================================================================

	describe("CreateRelation Op", () => {
		it("returns ADD relation diff for new relation", async () => {
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalCreateRelation}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)

			const body = await res.json()
			// Find the entity that has the relation
			const entityDiff = body.entities.find((d: any) => d.entityId === n(uuid.entityExisting1))
			expect(entityDiff).toBeDefined()

			// Should have ADD for the new relation
			// Structure: { relationId, typeId, spaceId, changeType, before, after: {toEntityId, toSpaceId, position} }
			const relationDiff = entityDiff.relations.find((r: any) => r.after?.toEntityId === n(uuid.entityExisting2))
			expect(relationDiff).toBeDefined()
			expect(relationDiff.changeType).toBe("ADD")
			expect(relationDiff.before).toBeNull()
			expect(relationDiff.typeId).toBe(n(uuid.relTypeGeneric))
			expect(relationDiff.after?.toEntityId).toBe(n(uuid.entityExisting2))
		})
	})

	// ==========================================================================
	// 5. DeleteRelation Op Tests
	// ==========================================================================

	describe("DeleteRelation Op", () => {
		it("returns REMOVE relation diff for deleted relation (requires relation lookup)", async () => {
			// This test verifies the new batchLookupRelationEntities() function.
			// A deleteRelation op only contains the relation ID, not the from_entity_id.
			// The proposal-diff code must look up the from_entity_id to know which
			// entity is affected by the relation deletion.
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalDeleteRelation}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)

			const body = await res.json()

			// The diff should contain the entity that has the relation being deleted
			const entityDiff = body.entities.find((d: any) => d.entityId === n(uuid.entityExistingWithRelations))
			expect(entityDiff).toBeDefined()

			// Should have REMOVE for the deleted relation
			const relationDiff = entityDiff.relations.find((r: any) => r.relationId === n(uuid.relToDelete))
			expect(relationDiff).toBeDefined()
			expect(relationDiff.changeType).toBe("REMOVE")
			expect(relationDiff.before).not.toBeNull()
			expect(relationDiff.before?.toEntityId).toBe(n(uuid.entityExisting1))
			expect(relationDiff.after).toBeNull()
		})
	})

	// ==========================================================================
	// 6. UpdateRelation Op Tests
	// ==========================================================================

	describe("UpdateRelation Op", () => {
		it("returns UPDATE relation diff for modified relation (requires relation lookup)", async () => {
			// This test verifies that updateRelation ops correctly look up the from_entity_id
			// and show the position change in the diff.
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalUpdateRelation}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)

			const body = await res.json()

			// The diff should contain the entity that has the relation being updated
			const entityDiff = body.entities.find((d: any) => d.entityId === n(uuid.entityExistingWithRelations))
			expect(entityDiff).toBeDefined()

			// Should have UPDATE for the modified relation
			const relationDiff = entityDiff.relations.find((r: any) => r.relationId === n(uuid.relToUpdate))
			expect(relationDiff).toBeDefined()
			expect(relationDiff.changeType).toBe("UPDATE")
			expect(relationDiff.before?.position).toBe("b0")
			expect(relationDiff.after?.position).toBe("z9") // New position set in the proposal
		})
	})

	// ==========================================================================
	// 7. Block Relation Tests
	// ==========================================================================

	describe("Block Relations", () => {
		it("returns REMOVE block diff when deleting a BLOCKS relation", async () => {
			// Regression test: proposal diffs should include blocks.
			// Previously, blocks were missing because batch snapshot fetching
			// skipped block discovery/population.
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalDeleteBlock}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)

			const body = await res.json()

			// The diff should contain the parent entity that owns the BLOCKS relation
			const entityDiff = body.entities.find((d: any) => d.entityId === n(uuid.entityWithBlocks))
			expect(entityDiff).toBeDefined()

			// Should have a REMOVE block diff (the block existed in base state, removed in proposed).
			// Block diffs use the format: { id, type, before, after, diff? }
			// For a removed text block: before = markdown content string, after = null
			expect(entityDiff.blocks).toBeDefined()
			expect(entityDiff.blocks.length).toBeGreaterThan(0)

			const blockDiff = entityDiff.blocks.find((b: any) => b.id === n(uuid.blockEntity1))
			expect(blockDiff).toBeDefined()
			expect(blockDiff.type).toBe("textBlock")
			expect(blockDiff.before).toBe("Block text content")
			expect(blockDiff.after).toBeNull()
		})
	})

	// ==========================================================================
	// 8. Multiple Operations Tests
	// ==========================================================================

	describe("Multiple Operations", () => {
		it("returns diffs for all affected entities from multiple ops", async () => {
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalMultipleOps}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)

			const body = await res.json()

			// Should have diffs for both entities affected by the multiple ops
			expect(body.entities.length).toBeGreaterThanOrEqual(2)

			// Check first entity (existing1 is updated)
			const diff1 = body.entities.find((d: any) => d.entityId === n(uuid.entityExisting1))
			expect(diff1).toBeDefined()

			// Check second entity (existing2 is updated)
			const diff2 = body.entities.find((d: any) => d.entityId === n(uuid.entityExisting2))
			expect(diff2).toBeDefined()
		})
	})

	// ==========================================================================
	// 8. Proposal Status Branches
	//
	// `getProposalStatus()` (proposal-diff.ts) decides the base-state source:
	//   - active            → live tables
	//   - closed (not exec) → versioned tables at end_time
	//   - executed          → versioned tables just before executed_at
	//
	// Most other tests in this suite exercise the `active` branch implicitly
	// via end_time-in-the-future fixtures. These tests cover the remaining
	// branches and assert the reported `proposalStatus`.
	// ==========================================================================

	describe("Proposal Status Branches", () => {
		it("returns proposalStatus=active for an open proposal", async () => {
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalCreateEntity}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposalStatus).toBe("active")
		})

		it("uses versioned base state for closed (not executed) proposals", async () => {
			const res = await app.request(`/versioned/proposals/${uuid.proposalClosed}/diff?spaceId=${uuid.space1}`)
			expect(res.status).toBe(200)

			const body = await res.json()
			// Closed proposals should still return entities
			expect(body.entities).toBeDefined()
			expect(body.proposalStatus).toBe("closed")
		})

		it("uses versioned base state just before executed_at for executed proposals", async () => {
			const res = await app.request(`/versioned/proposals/${uuid.proposalExecuted}/diff?spaceId=${uuid.space1}`)
			expect(res.status).toBe(200)

			const body = await res.json()
			expect(body.entities).toBeDefined()
			expect(body.proposalStatus).toBe("executed")
		})
	})

	// ==========================================================================
	// 9. Error Cases
	// ==========================================================================

	describe("Error Cases", () => {
		it("returns 400 for proposal with non-matching spaceId", async () => {
			const wrongSpaceId = "30000000-0001-4000-8000-000000000099"
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalCreateEntity}/diff?spaceId=${wrongSpaceId}`,
			)
			expect(res.status).toBe(400)

			const body = await res.json()
			// Error response has {error: "Invalid parameter", message: "...space..."}
			expect(body.message).toContain("space")
		})

		it("returns 404 for non-existent proposal", async () => {
			const nonExistentId = "20000000-0008-4000-8000-000000000999"
			const res = await app.request(`/versioned/proposals/${nonExistentId}/diff?spaceId=${uuid.space1}`)
			expect(res.status).toBe(404)
		})
	})
})

// ============================================================================
// Test Data Setup
// ============================================================================

async function setupTestData(pool: Pool): Promise<void> {
	const client = await pool.connect()

	try {
		await client.query("BEGIN")

		// 1. Create space
		await client.query(
			`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xTestSpace20000000') ON CONFLICT DO NOTHING`,
			[uuid.space1],
		)

		// 2. Create edit version (for existing entities)
		// version_key = (block_number << 32) | sequence
		const blockNumber1 = BigInt(2000)
		const sequence1 = BigInt(0)
		await client.query(
			`INSERT INTO edit_versions (edit_id, block_number, sequence, version_key, created_at)
			 VALUES ($1, $2, $3, $4, '2024-01-01T00:00:00Z') ON CONFLICT DO NOTHING`,
			[uuid.edit1, blockNumber1.toString(), sequence1.toString(), versionKey1],
		)

		// 3. Create existing entities with values
		// We need to insert into BOTH:
		// - `values` table (live state for active proposals)
		// - `value_versions` table (versioned state for closed proposals)
		let valIdx = 1

		// Entity 1 - will be updated (live state)
		await client.query(
			`INSERT INTO "values" (id, entity_id, property_id, space_id, text)
			 VALUES ($1, $2, $3, $4, 'Original Text') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx), uuid.entityExisting1, uuid.propText, uuid.space1],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Original Text') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityExisting1, uuid.propText, uuid.space1, versionKey1],
		)

		// Entity 2 - exists for relations (live state)
		await client.query(
			`INSERT INTO "values" (id, entity_id, property_id, space_id, text)
			 VALUES ($1, $2, $3, $4, 'Entity Two') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx), uuid.entityExisting2, uuid.propText, uuid.space1],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Entity Two') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityExisting2, uuid.propText, uuid.space1, versionKey1],
		)

		// Entity with relations - has an existing relation (live state)
		await client.query(
			`INSERT INTO "values" (id, entity_id, property_id, space_id, text)
			 VALUES ($1, $2, $3, $4, 'Entity with relations') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx), uuid.entityExistingWithRelations, uuid.propText, uuid.space1],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Entity with relations') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityExistingWithRelations, uuid.propText, uuid.space1, versionKey1],
		)

		// Create an existing relation that will be deleted by a proposal
		// This relation goes from entityExistingWithRelations -> entityExisting1
		await client.query(
			`INSERT INTO relations (id, entity_id, from_entity_id, to_entity_id, type_id, space_id, position)
			 VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING`,
			[
				uuid.relToDelete,
				uuid.entityExistingWithRelations,
				uuid.entityExistingWithRelations,
				uuid.entityExisting1,
				uuid.relTypeGeneric,
				uuid.space1,
				"a0",
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, from_entity_id, to_entity_id, type_id, space_id, valid_from_key, valid_to_key, position)
			 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9) ON CONFLICT DO NOTHING`,
			[
				uuid.rel1,
				uuid.relToDelete,
				uuid.entityExistingWithRelations,
				uuid.entityExistingWithRelations,
				uuid.entityExisting1,
				uuid.relTypeGeneric,
				uuid.space1,
				versionKey1,
				"a0",
			],
		)

		// Create another relation that will be updated (position changed) by a proposal
		// This relation also goes from entityExistingWithRelations -> entityExisting2
		await client.query(
			`INSERT INTO relations (id, entity_id, from_entity_id, to_entity_id, type_id, space_id, position)
			 VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING`,
			[
				uuid.relToUpdate,
				uuid.entityExistingWithRelations,
				uuid.entityExistingWithRelations,
				uuid.entityExisting2,
				uuid.relTypeGeneric,
				uuid.space1,
				"b0",
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, from_entity_id, to_entity_id, type_id, space_id, valid_from_key, valid_to_key, position)
			 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9) ON CONFLICT DO NOTHING`,
			[
				uuid.rel2,
				uuid.relToUpdate,
				uuid.entityExistingWithRelations,
				uuid.entityExistingWithRelations,
				uuid.entityExisting2,
				uuid.relTypeGeneric,
				uuid.space1,
				versionKey1,
				"b0",
			],
		)

		// Entity with blocks - a page-like entity that has a BLOCKS relation to a text block.
		// Used by the "Block Relations" test to verify proposal diffs correctly show blocks.
		await client.query(
			`INSERT INTO "values" (id, entity_id, property_id, space_id, text)
			 VALUES ($1, $2, $3, $4, 'Page with blocks') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityWithBlocks, uuid.propText, uuid.space1],
		)

		// Block entity - a text block with MARKDOWN_CONTENT value
		await client.query(
			`INSERT INTO "values" (id, entity_id, property_id, space_id, text)
			 VALUES ($1, $2, $3, $4, 'Block text content') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.blockEntity1, uuid.markdownContent, uuid.space1],
		)

		// TYPES relation on the block: blockEntity1 -> TEXT_BLOCK (identifies this as a text block)
		await client.query(
			`INSERT INTO relations (id, entity_id, from_entity_id, to_entity_id, type_id, space_id)
			 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING`,
			[
				uuid.blockTypesRelation1,
				uuid.blockEntity1,
				uuid.blockEntity1,
				uuid.textBlockType,
				uuid.typesProperty,
				uuid.space1,
			],
		)

		// BLOCKS relation: entityWithBlocks -> blockEntity1 (live table)
		await client.query(
			`INSERT INTO relations (id, entity_id, from_entity_id, to_entity_id, type_id, space_id, position)
			 VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING`,
			[
				uuid.blocksRelation1,
				uuid.entityWithBlocks,
				uuid.entityWithBlocks,
				uuid.blockEntity1,
				uuid.relTypeBlocks,
				uuid.space1,
				"a0",
			],
		)

		// Entity to delete (live state)
		await client.query(
			`INSERT INTO "values" (id, entity_id, property_id, space_id, text)
			 VALUES ($1, $2, $3, $4, 'Entity to be deleted') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx), uuid.entityToDelete, uuid.propText, uuid.space1],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Entity to be deleted') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityToDelete, uuid.propText, uuid.space1, versionKey1],
		)

		// 4. Create proposals and their edits
		const now = Math.floor(Date.now() / 1000)

		// Proposal 1: CreateEntity
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalCreateEntity,
			actionId: uuid.actionCreateEntity,
			spaceId: uuid.space1,
			startTime: now - 1000,
			endTime: now + 86400,
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("Create New Entity")
					.setCreatedNow()
					.createEntity(uuidToId(uuid.entityNew1), (e) => e.text(uuidToId(uuid.propText), "New Entity Title"))
				return builder.build()
			},
			contentUri: generateTestUri("create-entity"),
		})

		// Proposal 2: UpdateEntity
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalUpdateEntity,
			actionId: uuid.actionUpdateEntity,
			spaceId: uuid.space1,
			startTime: now - 1000,
			endTime: now + 86400,
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("Update Entity")
					.setCreatedNow()
					.updateEntity(uuidToId(uuid.entityExisting1), (u) =>
						u.setText(uuidToId(uuid.propText), "Updated Text").setBool(uuidToId(uuid.propBool), true),
					)
				return builder.build()
			},
			contentUri: generateTestUri("update-entity"),
		})

		// Proposal 3: DeleteEntity
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalDeleteEntity,
			actionId: uuid.actionDeleteEntity,
			spaceId: uuid.space1,
			startTime: now - 1000,
			endTime: now + 86400,
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("Delete Entity")
					.setCreatedNow()
					.deleteEntity(uuidToId(uuid.entityToDelete))
				return builder.build()
			},
			contentUri: generateTestUri("delete-entity"),
		})

		// Proposal 4: CreateRelation
		const relationId = randomId()
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalCreateRelation,
			actionId: uuid.actionCreateRelation,
			spaceId: uuid.space1,
			startTime: now - 1000,
			endTime: now + 86400,
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("Create Relation")
					.setCreatedNow()
					.createRelationSimple(
						relationId,
						uuidToId(uuid.entityExisting1),
						uuidToId(uuid.entityExisting2),
						uuidToId(uuid.relTypeGeneric),
					)
				return builder.build()
			},
			contentUri: generateTestUri("create-relation"),
		})

		// Proposal 5: Multiple Operations
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalMultipleOps,
			actionId: uuid.actionMultipleOps,
			spaceId: uuid.space1,
			startTime: now - 1000,
			endTime: now + 86400,
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("Multiple Operations")
					.setCreatedNow()
					.updateEntity(uuidToId(uuid.entityExisting1), (u) =>
						u.setInt64(uuidToId(uuid.propInt), BigInt(100)),
					)
					.updateEntity(uuidToId(uuid.entityExisting2), (u) =>
						u.setFloat64(uuidToId(uuid.propFloat), 2.71828),
					)
				return builder.build()
			},
			contentUri: generateTestUri("multiple-ops"),
		})

		// Proposal 6: All Value Types
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalAllValueTypes,
			actionId: uuid.actionAllValueTypes,
			spaceId: uuid.space1,
			startTime: now - 1000,
			endTime: now + 86400,
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("All Value Types")
					.setCreatedNow()
					.createEntity(uuidToId(uuid.entityNew2), (e) =>
						e
							.text(uuidToId(uuid.propText), "Entity with all types")
							.bool(uuidToId(uuid.propBool), true)
							.int64(uuidToId(uuid.propInt), BigInt(42))
							.float64(uuidToId(uuid.propFloat), 3.14159),
					)
				return builder.build()
			},
			contentUri: generateTestUri("all-value-types"),
		})

		// Proposal 7: Closed Proposal
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalClosed,
			actionId: uuid.actionClosed,
			spaceId: uuid.space1,
			startTime: now - 86400,
			endTime: now - 1000, // Ended in the past
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("Closed Proposal Edit")
					.setCreatedNow()
					.updateEntity(uuidToId(uuid.entityExisting1), (u) =>
						u.setText(uuidToId(uuid.propText), "From closed proposal"),
					)
				return builder.build()
			},
			contentUri: generateTestUri("closed-proposal"),
		})

		// Proposal 8: DeleteRelation
		// This tests that deleteRelation ops correctly look up the from_entity_id
		// to include the affected entity in the diff
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalDeleteRelation,
			actionId: uuid.actionDeleteRelation,
			spaceId: uuid.space1,
			startTime: now - 1000,
			endTime: now + 86400,
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("Delete Relation")
					.setCreatedNow()
					.deleteRelation(uuidToId(uuid.relToDelete))
				return builder.build()
			},
			contentUri: generateTestUri("delete-relation"),
		})

		// Proposal 9: UpdateRelation
		// This tests that updateRelation ops correctly look up the from_entity_id
		// and that the position change is reflected in the diff
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalUpdateRelation,
			actionId: uuid.actionUpdateRelation,
			spaceId: uuid.space1,
			startTime: now - 1000,
			endTime: now + 86400,
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("Update Relation")
					.setCreatedNow()
					.updateRelation(uuidToId(uuid.relToUpdate), (r) => r.setPosition("z9"))
				return builder.build()
			},
			contentUri: generateTestUri("update-relation"),
		})

		// Proposal 10: Delete BLOCKS relation
		// Regression test: verifies that proposal diffs correctly populate blocks
		// on base state snapshots, so the diff shows the removed block with its content.
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalDeleteBlock,
			actionId: uuid.actionDeleteBlock,
			spaceId: uuid.space1,
			startTime: now - 1000,
			endTime: now + 86400,
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("Delete Block Relation")
					.setCreatedNow()
					.deleteRelation(uuidToId(uuid.blocksRelation1))
				return builder.build()
			},
			contentUri: generateTestUri("delete-block"),
		})

		// Proposal 11: Executed Proposal — `executedAt` is set, so getProposalStatus
		// returns "executed" regardless of where now sits relative to endTime.
		// The diff should be computed against versioned state just before executedAt.
		await createProposalWithEdit(client, {
			proposalId: uuid.proposalExecuted,
			actionId: uuid.actionExecuted,
			spaceId: uuid.space1,
			startTime: now - 86400,
			endTime: now - 1000,
			executedAt: now - 500,
			editBuilder: (editId) => {
				const builder = new EditBuilder(editId)
					.setName("Executed Proposal Edit")
					.setCreatedNow()
					.updateEntity(uuidToId(uuid.entityExisting1), (u) =>
						u.setText(uuidToId(uuid.propText), "From executed proposal"),
					)
				return builder.build()
			},
			contentUri: generateTestUri("executed-proposal"),
		})

		await client.query("COMMIT")
	} catch (error) {
		await client.query("ROLLBACK")
		throw error
	} finally {
		client.release()
	}
}

interface CreateProposalOptions {
	proposalId: string
	actionId: string
	spaceId: string
	startTime: number
	endTime: number
	editBuilder: (editId: Id) => ReturnType<EditBuilder["build"]>
	contentUri: string
	executedAt?: number
}

async function createProposalWithEdit(client: any, options: CreateProposalOptions): Promise<void> {
	const {proposalId, actionId, spaceId, startTime, endTime, editBuilder, contentUri, executedAt} = options

	// Generate a random edit ID
	const editId = randomId()

	// Build the edit
	const edit = editBuilder(editId)

	// Encode the edit to binary (using sync encodeEdit instead of async encodeEditAuto)
	const encoded = encodeEdit(edit)

	// Insert into ipfs_cache
	await client.query(
		`INSERT INTO ipfs_cache (uri, data, block, space, name, is_errored)
		 VALUES ($1, $2, '2000', $3, $4, false) ON CONFLICT (uri) DO NOTHING`,
		[contentUri, Buffer.from(encoded), spaceId, edit.name],
	)

	// Create proposal (V2 identity row + version 1 row).
	// `proposals_current` view joins these on current_version = proposal_version.
	if (executedAt) {
		await client.query(
			`INSERT INTO proposals (id, space_id, proposed_by, executed_at, created_at, created_at_block, current_version)
			 VALUES ($1, $2, $3, $4, '2024-01-01T00:00:00Z', '2000', 1) ON CONFLICT DO NOTHING`,
			[proposalId, spaceId, uuid.entityExisting1, executedAt],
		)
	} else {
		await client.query(
			`INSERT INTO proposals (id, space_id, proposed_by, created_at, created_at_block, current_version)
			 VALUES ($1, $2, $3, '2024-01-01T00:00:00Z', '2000', 1) ON CONFLICT DO NOTHING`,
			[proposalId, spaceId, uuid.entityExisting1],
		)
	}

	await client.query(
		`INSERT INTO proposal_versions (
			proposal_id, proposal_version, voting_mode, start_time, end_time,
			quorum, threshold,
			partial_percentage_support_threshold, universal_percentage_support_threshold,
			flat_support_threshold,
			version_created_at, version_created_at_block
		 )
		 VALUES ($1, 1, 'Fast', $2, $3, 1, 1, 0, 0, 1, '2024-01-01T00:00:00Z', '2000') ON CONFLICT DO NOTHING`,
		[proposalId, startTime, endTime],
	)

	// Create proposal action (Publish) — version-scoped on (proposal_id, proposal_version, index).
	// `actionId` is retained in the option signature for caller stability but is no longer a column.
	void actionId
	await client.query(
		`INSERT INTO proposal_actions (proposal_id, proposal_version, index, action_type, content_uri)
		 VALUES ($1, 1, 0, 'Publish', $2) ON CONFLICT DO NOTHING`,
		[proposalId, contentUri],
	)
}

async function cleanupTestData(pool: Pool): Promise<void> {
	const client = await pool.connect()

	try {
		await client.query("BEGIN")

		// Delete in reverse order of foreign key dependencies
		await client.query(`DELETE FROM proposal_actions WHERE proposal_id::text LIKE '20000000-%'`)
		await client.query(`DELETE FROM proposal_versions WHERE proposal_id::text LIKE '20000000-%'`)
		await client.query(`DELETE FROM proposals WHERE id::text LIKE '20000000-%'`)
		await client.query(`DELETE FROM relations WHERE from_entity_id::text LIKE '20000000-%'`)
		await client.query(`DELETE FROM relation_versions WHERE from_entity_id::text LIKE '20000000-%'`)
		await client.query(`DELETE FROM "values" WHERE entity_id::text LIKE '20000000-%'`)
		await client.query(`DELETE FROM value_versions WHERE entity_id::text LIKE '20000000-%'`)
		await client.query(`DELETE FROM edit_versions WHERE edit_id::text LIKE '20000000-%'`)
		await client.query(`DELETE FROM entities WHERE id::text LIKE '20000000-%'`)
		await client.query(`DELETE FROM spaces WHERE id::text LIKE '20000000-%'`)
		await client.query(`DELETE FROM ipfs_cache WHERE uri LIKE 'ipfs://bafkreitest%'`)

		await client.query("COMMIT")
	} catch (error) {
		await client.query("ROLLBACK")
		console.warn("Cleanup error:", error)
	} finally {
		client.release()
	}
}
