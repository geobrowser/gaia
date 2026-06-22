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
import {createVersionedV2Router} from "../v2"

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
			expect(Number(floatDiff?.after)).toBeCloseTo(Math.PI, 4)
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
	// 8. Closed Proposal Tests
	// ==========================================================================

	describe("Closed Proposal", () => {
		it("uses versioned base state for closed proposals", async () => {
			const res = await app.request(`/versioned/proposals/${uuid.proposalClosed}/diff?spaceId=${uuid.space1}`)
			expect(res.status).toBe(200)

			const body = await res.json()
			// Closed proposals should still return entities
			expect(body.entities).toBeDefined()
			expect(body.proposalStatus).toBe("closed")
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

	// ==========================================================================
	// 10. v2 enriched / context-aware proposal diff (/v2/versioned/...)
	// ==========================================================================

	describe("v2 enriched endpoints", () => {
		let v2App: Hono

		beforeAll(async () => {
			const db = drizzle(pool)
			v2App = new Hono()
			v2App.route("/v2/versioned", createVersionedV2Router(db as any, runtime))
			// Seed a NAME for propText so name resolution (enrichNames) is assertable.
			await pool.query(
				`INSERT INTO "values" (id, entity_id, property_id, space_id, text) VALUES ($1, $2, $3, $4, 'Text Property') ON CONFLICT DO NOTHING`,
				[uuid.val(900), n(uuid.propText), n(SystemIds.NAME_PROPERTY), n(uuid.space1)],
			)
		})

		it("returns the same set of changed entities as v1 (enrichment preserves the diff)", async () => {
			const [v1Res, v2Res] = await Promise.all([
				app.request(`/versioned/proposals/${uuid.proposalMultipleOps}/diff?spaceId=${uuid.space1}`),
				v2App.request(`/v2/versioned/proposals/${uuid.proposalMultipleOps}/diff?spaceId=${uuid.space1}`),
			])
			expect(v1Res.status).toBe(200)
			expect(v2Res.status).toBe(200)
			const v1 = await v1Res.json()
			const v2 = await v2Res.json()
			const ids = (b: {entities: {entityId: string}[]}) => b.entities.map((e) => e.entityId).sort()
			expect(ids(v2)).toEqual(ids(v1))
			expect(v2.pagination.totalEntities).toBe(v1.pagination.totalEntities)
		})

		it("returns the grouped/enriched shape with propertyName resolved", async () => {
			const res = await v2App.request(
				`/v2/versioned/proposals/${uuid.proposalCreateEntity}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			const entity = body.entities.find((e: {entityId: string}) => e.entityId === n(uuid.entityNew1))
			expect(entity).toBeDefined()
			expect(Array.isArray(entity.groupKeys)).toBe(true)
			expect(Array.isArray(entity.blocks)).toBe(true)
			const textChange = entity.values.find((v: {propertyId: string}) => v.propertyId === n(uuid.propText))
			expect(textChange).toBeDefined()
			expect(textChange).toHaveProperty("propertyName")
			expect(textChange.propertyName).toBe("Text Property")
		})

		it("relation changes carry typeName + toEntityName fields", async () => {
			const res = await v2App.request(
				`/v2/versioned/proposals/${uuid.proposalCreateRelation}/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			const withRel = body.entities.find((e: {relations: unknown[]}) => e.relations.length > 0)
			expect(withRel).toBeDefined()
			const rel = withRel.relations[0]
			expect(rel).toHaveProperty("typeName")
			expect(rel.after ?? rel.before).toHaveProperty("toEntityName")
		})

		it("grouped (multi-proposal) v2 endpoint returns enriched entities", async () => {
			const res = await v2App.request(
				`/v2/versioned/proposal-groups/diff?spaceId=${uuid.space1}&proposalIds=${uuid.proposalCreateEntity},${uuid.proposalCreateRelation}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.mode).toBe("active")
			expect(Array.isArray(body.entities)).toBe(true)
			expect(body.entities.length).toBeGreaterThan(0)
		})

		it("returns 404 for a non-existent proposal", async () => {
			const res = await v2App.request(
				`/v2/versioned/proposals/20000000-0008-4000-8000-000000000999/diff?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(404)
		})

		it("returns 400 for an invalid spaceId", async () => {
			const res = await v2App.request(
				`/v2/versioned/proposals/${uuid.proposalCreateEntity}/diff?spaceId=not-a-uuid`,
			)
			expect(res.status).toBe(400)
		})
	})

	// ==========================================================================
	// 11. v2 folding — cross-parent block folding + media-property filtering
	// ==========================================================================

	describe("v2 folding (blocks + media)", () => {
		let v2App: Hono
		const f = {
			space: "21000000-0001-4000-8000-000000000001",
			pageA: "21000000-0002-4000-8000-000000000001",
			blockA: "21000000-0002-4000-8000-000000000002",
			relBlocksA: "21000000-0006-4000-8000-000000000001",
			relTypesA: "21000000-0006-4000-8000-000000000002",
			propEditBlock: "21000000-0008-4000-8000-000000000001",
			actEditBlock: "21000000-000a-4000-8000-000000000001",
			pageB: "21000000-0002-4000-8000-000000000003",
			imgB: "21000000-0002-4000-8000-000000000004",
			relTypeAvatar: "21000000-0005-4000-8000-000000000001",
			relImgTypes: "21000000-0006-4000-8000-000000000003",
			relAvatar: "21000000-0006-4000-8000-000000000004",
			propAddAvatar: "21000000-0008-4000-8000-000000000002",
			actAddAvatar: "21000000-000a-4000-8000-000000000002",
			// Scenario C: data block whose config (View) is set on the reified BLOCKS relation entity.
			pageC: "21000000-0002-4000-8000-000000000005",
			dataBlockC: "21000000-0002-4000-8000-000000000006",
			relBlocksC: "21000000-0006-4000-8000-000000000005",
			relTypesC: "21000000-0006-4000-8000-000000000006",
			relViewConfig: "21000000-0006-4000-8000-000000000007",
			propSetConfig: "21000000-0008-4000-8000-000000000003",
			actSetConfig: "21000000-000a-4000-8000-000000000003",
			// Scenario V: video block whose url is edited.
			pageV: "21000000-0002-4000-8000-000000000007",
			videoV: "21000000-0002-4000-8000-000000000008",
			relBlocksV: "21000000-0006-4000-8000-000000000008",
			relTypesV: "21000000-0006-4000-8000-000000000009",
			propEditVideo: "21000000-0008-4000-8000-000000000004",
			actEditVideo: "21000000-000a-4000-8000-000000000004",
			// Scenario R: ranking block (a data-block subtype) whose name is edited.
			pageR: "21000000-0002-4000-8000-000000000009",
			rankR: "21000000-0002-4000-8000-00000000000a",
			relBlocksR: "21000000-0006-4000-8000-00000000000a",
			relTypesR: "21000000-0006-4000-8000-00000000000b",
			propEditRank: "21000000-0008-4000-8000-000000000005",
			actEditRank: "21000000-000a-4000-8000-000000000005",
			// Scenario U: existing (DB-typed) image whose IMAGE_URL is updated in the proposal.
			pageU: "21000000-0002-4000-8000-00000000000b",
			imgU: "21000000-0002-4000-8000-00000000000c",
			relImgUTypes: "21000000-0006-4000-8000-00000000000c",
			relCoverU: "21000000-0006-4000-8000-00000000000d",
			propUpdateMedia: "21000000-0008-4000-8000-000000000006",
			actUpdateMedia: "21000000-000a-4000-8000-000000000006",
			// Scenario M: one data block shared under TWO parents, each with its own config.
			pageM1: "21000000-0002-4000-8000-00000000000d",
			pageM2: "21000000-0002-4000-8000-00000000000e",
			sharedBlock: "21000000-0002-4000-8000-00000000000f",
			relBlocksM1: "21000000-0006-4000-8000-00000000000e",
			relBlocksM2: "21000000-0006-4000-8000-00000000000f",
			relTypesShared: "21000000-0006-4000-8000-000000000010",
			relViewM1: "21000000-0006-4000-8000-000000000011",
			relViewM2: "21000000-0006-4000-8000-000000000012",
			propMultiConfig: "21000000-0008-4000-8000-000000000007",
			actMultiConfig: "21000000-000a-4000-8000-000000000007",
			// Scenario X: proposal UNSETS an existing image's IMAGE_URL (reuses pageU/imgU).
			relCoverX: "21000000-0006-4000-8000-000000000013",
			propUnsetMedia: "21000000-0008-4000-8000-000000000008",
			actUnsetMedia: "21000000-000a-4000-8000-000000000008",
			// Scenario Y: one edit SETS then UNSETS a new image's IMAGE_URL (last write wins → no url).
			pageY: "21000000-0002-4000-8000-000000000010",
			imgY: "21000000-0002-4000-8000-000000000011",
			relImgYTypes: "21000000-0006-4000-8000-000000000014",
			relCoverY: "21000000-0006-4000-8000-000000000015",
			propSetUnsetMedia: "21000000-0008-4000-8000-000000000009",
			actSetUnsetMedia: "21000000-000a-4000-8000-000000000009",
		}
		const RANKING_BLOCK_TYPE = "150db6defe2344f0805afa57502e2c32"

		beforeAll(async () => {
			const db = drizzle(pool)
			v2App = new Hono()
			v2App.route("/v2/versioned", createVersionedV2Router(db as any, runtime))

			const now = Math.floor(Date.now() / 1000)
			const client = await pool.connect()
			try {
				await client.query("BEGIN")
				await client.query(
					`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xFoldTestSpace') ON CONFLICT DO NOTHING`,
					[f.space],
				)
				const val = (id: string, entity: string, prop: string, text: string) =>
					client.query(
						`INSERT INTO "values" (id, entity_id, property_id, space_id, text) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING`,
						[id, entity, prop, f.space, text],
					)
				const rel = (id: string, from: string, to: string, type: string) =>
					client.query(
						`INSERT INTO relations (id, entity_id, from_entity_id, to_entity_id, type_id, space_id) VALUES ($1, $1, $2, $3, $4, $5) ON CONFLICT DO NOTHING`,
						[id, from, to, type, f.space],
					)
				// Scenario A: page A owns a text block B (markdown "old text").
				await val(`${f.pageA}-name`, f.pageA, SystemIds.NAME_PROPERTY, "Page A")
				await val(`${f.blockA}-name`, f.blockA, SystemIds.NAME_PROPERTY, "Block A")
				await val(`${f.blockA}-md`, f.blockA, SystemIds.MARKDOWN_CONTENT, "old text")
				await rel(f.relBlocksA, f.pageA, f.blockA, SystemIds.BLOCKS)
				await rel(f.relTypesA, f.blockA, SystemIds.TEXT_BLOCK, SystemIds.TYPES_PROPERTY)
				// Scenario B: page B (name only); proposal will add an image + avatar relation.
				await val(`${f.pageB}-name`, f.pageB, SystemIds.NAME_PROPERTY, "Page B")
				// Scenario C: page C owns a data block; config lives on the reified BLOCKS relation.
				await val(`${f.pageC}-name`, f.pageC, SystemIds.NAME_PROPERTY, "Page C")
				await val(`${f.dataBlockC}-name`, f.dataBlockC, SystemIds.NAME_PROPERTY, "My Data Block")
				// The BLOCKS relation's reified entity id (entity_id) == relBlocksC → config carrier.
				await rel(f.relBlocksC, f.pageC, f.dataBlockC, SystemIds.BLOCKS)
				await rel(f.relTypesC, f.dataBlockC, SystemIds.DATA_BLOCK, SystemIds.TYPES_PROPERTY)
				// Scenario V: page V owns a video block (url "ipfs://oldvid").
				await val(`${f.pageV}-name`, f.pageV, SystemIds.NAME_PROPERTY, "Page V")
				await val(`${f.videoV}-url`, f.videoV, SystemIds.IMAGE_URL_PROPERTY, "ipfs://oldvid")
				await rel(f.relBlocksV, f.pageV, f.videoV, SystemIds.BLOCKS)
				await rel(f.relTypesV, f.videoV, SystemIds.VIDEO_BLOCK, SystemIds.TYPES_PROPERTY)
				// Scenario R: page R owns a ranking block (name "Old Rank").
				await val(`${f.pageR}-name`, f.pageR, SystemIds.NAME_PROPERTY, "Page R")
				await val(`${f.rankR}-name`, f.rankR, SystemIds.NAME_PROPERTY, "Old Rank")
				await rel(f.relBlocksR, f.pageR, f.rankR, SystemIds.BLOCKS)
				await rel(f.relTypesR, f.rankR, RANKING_BLOCK_TYPE, SystemIds.TYPES_PROPERTY)
				// Scenario U: page U + an EXISTING image entity (DB-typed, url "ipfs://oldcover").
				await val(`${f.pageU}-name`, f.pageU, SystemIds.NAME_PROPERTY, "Page U")
				await val(`${f.imgU}-url`, f.imgU, SystemIds.IMAGE_URL_PROPERTY, "ipfs://oldcover")
				await rel(f.relImgUTypes, f.imgU, SystemIds.IMAGE_TYPE, SystemIds.TYPES_PROPERTY)
				// Scenario M: a single data block embedded under two parents (distinct config carriers).
				await val(`${f.pageM1}-name`, f.pageM1, SystemIds.NAME_PROPERTY, "Parent 1")
				await val(`${f.pageM2}-name`, f.pageM2, SystemIds.NAME_PROPERTY, "Parent 2")
				await val(`${f.sharedBlock}-name`, f.sharedBlock, SystemIds.NAME_PROPERTY, "Shared Block")
				await rel(f.relBlocksM1, f.pageM1, f.sharedBlock, SystemIds.BLOCKS)
				await rel(f.relBlocksM2, f.pageM2, f.sharedBlock, SystemIds.BLOCKS)
				await rel(f.relTypesShared, f.sharedBlock, SystemIds.DATA_BLOCK, SystemIds.TYPES_PROPERTY)
				await client.query("COMMIT")
			} catch (e) {
				await client.query("ROLLBACK")
				throw e
			} finally {
				client.release()
			}

			const client2 = await pool.connect()
			try {
				// Proposal A: edit block A's markdown. Page A is NOT in the edit → orphan parent.
				await createProposalWithEdit(client2, {
					proposalId: f.propEditBlock,
					actionId: f.actEditBlock,
					spaceId: f.space,
					startTime: now - 1000,
					endTime: now + 86400,
					editBuilder: (editId) =>
						new EditBuilder(editId)
							.setName("Edit block text")
							.setCreatedNow()
							.updateEntity(uuidToId(f.blockA), (u) =>
								u.setText(uuidToId(SystemIds.MARKDOWN_CONTENT), "new text"),
							)
							.build(),
					contentUri: generateTestUri("fold-edit-block"),
				})
				// Proposal B: add an avatar image to page B (new IMAGE entity + relation).
				await createProposalWithEdit(client2, {
					proposalId: f.propAddAvatar,
					actionId: f.actAddAvatar,
					spaceId: f.space,
					startTime: now - 1000,
					endTime: now + 86400,
					editBuilder: (editId) =>
						new EditBuilder(editId)
							.setName("Add avatar")
							.setCreatedNow()
							.createEntity(uuidToId(f.imgB), (e) =>
								e.text(uuidToId(SystemIds.IMAGE_URL_PROPERTY), "ipfs://foldtestimg"),
							)
							.createRelationSimple(
								uuidToId(f.relImgTypes),
								uuidToId(f.imgB),
								uuidToId(SystemIds.IMAGE_TYPE),
								uuidToId(SystemIds.TYPES_PROPERTY),
							)
							.createRelationSimple(
								uuidToId(f.relAvatar),
								uuidToId(f.pageB),
								uuidToId(f.imgB),
								uuidToId(f.relTypeAvatar),
							)
							.build(),
					contentUri: generateTestUri("fold-add-avatar"),
				})
				// Proposal C: set a View config on the data block's reified BLOCKS relation entity.
				await createProposalWithEdit(client2, {
					proposalId: f.propSetConfig,
					actionId: f.actSetConfig,
					spaceId: f.space,
					startTime: now - 1000,
					endTime: now + 86400,
					editBuilder: (editId) =>
						new EditBuilder(editId)
							.setName("Set data block view")
							.setCreatedNow()
							.createRelationSimple(
								uuidToId(f.relViewConfig),
								uuidToId(f.relBlocksC), // from = reified BLOCKS relation entity
								uuidToId(SystemIds.GALLERY_VIEW),
								uuidToId(SystemIds.VIEW_PROPERTY),
							)
							.build(),
					contentUri: generateTestUri("fold-set-config"),
				})
				// Proposal V: edit the video block's url.
				await createProposalWithEdit(client2, {
					proposalId: f.propEditVideo,
					actionId: f.actEditVideo,
					spaceId: f.space,
					startTime: now - 1000,
					endTime: now + 86400,
					editBuilder: (editId) =>
						new EditBuilder(editId)
							.setName("Edit video url")
							.setCreatedNow()
							.updateEntity(uuidToId(f.videoV), (u) =>
								u.setText(uuidToId(SystemIds.IMAGE_URL_PROPERTY), "ipfs://newvid"),
							)
							.build(),
					contentUri: generateTestUri("fold-edit-video"),
				})
				// Proposal R: rename the ranking block.
				await createProposalWithEdit(client2, {
					proposalId: f.propEditRank,
					actionId: f.actEditRank,
					spaceId: f.space,
					startTime: now - 1000,
					endTime: now + 86400,
					editBuilder: (editId) =>
						new EditBuilder(editId)
							.setName("Rename ranking block")
							.setCreatedNow()
							.updateEntity(uuidToId(f.rankR), (u) =>
								u.setText(uuidToId(SystemIds.NAME_PROPERTY), "New Rank"),
							)
							.build(),
					contentUri: generateTestUri("fold-edit-rank"),
				})
				// Proposal U: update an existing image's url AND point a Cover relation at it.
				await createProposalWithEdit(client2, {
					proposalId: f.propUpdateMedia,
					actionId: f.actUpdateMedia,
					spaceId: f.space,
					startTime: now - 1000,
					endTime: now + 86400,
					editBuilder: (editId) =>
						new EditBuilder(editId)
							.setName("Update cover image url")
							.setCreatedNow()
							.updateEntity(uuidToId(f.imgU), (u) =>
								u.setText(uuidToId(SystemIds.IMAGE_URL_PROPERTY), "ipfs://newcover"),
							)
							.createRelationSimple(
								uuidToId(f.relCoverU),
								uuidToId(f.pageU),
								uuidToId(f.imgU),
								uuidToId(f.relTypeAvatar),
							)
							.build(),
					contentUri: generateTestUri("fold-update-media"),
				})
				// Proposal M: set DIFFERENT views on the shared block's two parent configs.
				await createProposalWithEdit(client2, {
					proposalId: f.propMultiConfig,
					actionId: f.actMultiConfig,
					spaceId: f.space,
					startTime: now - 1000,
					endTime: now + 86400,
					editBuilder: (editId) =>
						new EditBuilder(editId)
							.setName("Set per-parent views")
							.setCreatedNow()
							.createRelationSimple(
								uuidToId(f.relViewM1),
								uuidToId(f.relBlocksM1),
								uuidToId(SystemIds.GALLERY_VIEW),
								uuidToId(SystemIds.VIEW_PROPERTY),
							)
							.createRelationSimple(
								uuidToId(f.relViewM2),
								uuidToId(f.relBlocksM2),
								uuidToId(SystemIds.LIST_VIEW),
								uuidToId(SystemIds.VIEW_PROPERTY),
							)
							.build(),
					contentUri: generateTestUri("fold-multi-config"),
				})
				// Proposal X: unset the existing image's IMAGE_URL while pointing a Cover relation at it.
				await createProposalWithEdit(client2, {
					proposalId: f.propUnsetMedia,
					actionId: f.actUnsetMedia,
					spaceId: f.space,
					startTime: now - 1000,
					endTime: now + 86400,
					editBuilder: (editId) =>
						new EditBuilder(editId)
							.setName("Remove cover image url")
							.setCreatedNow()
							.updateEntity(uuidToId(f.imgU), (u) => u.unsetAll(uuidToId(SystemIds.IMAGE_URL_PROPERTY)))
							.createRelationSimple(
								uuidToId(f.relCoverX),
								uuidToId(f.pageU),
								uuidToId(f.imgU),
								uuidToId(f.relTypeAvatar),
							)
							.build(),
					contentUri: generateTestUri("fold-unset-media"),
				})
				// Proposal Y: within ONE edit, set then unset a new image's IMAGE_URL.
				// Last write wins → the proposed url must NOT be inlined onto the Cover relation.
				await createProposalWithEdit(client2, {
					proposalId: f.propSetUnsetMedia,
					actionId: f.actSetUnsetMedia,
					spaceId: f.space,
					startTime: now - 1000,
					endTime: now + 86400,
					editBuilder: (editId) =>
						new EditBuilder(editId)
							.setName("Add then remove cover url")
							.setCreatedNow()
							.createEntity(uuidToId(f.imgY), (e) =>
								e.text(uuidToId(SystemIds.IMAGE_URL_PROPERTY), "ipfs://shouldberemoved"),
							)
							.createRelationSimple(
								uuidToId(f.relImgYTypes),
								uuidToId(f.imgY),
								uuidToId(SystemIds.IMAGE_TYPE),
								uuidToId(SystemIds.TYPES_PROPERTY),
							)
							.updateEntity(uuidToId(f.imgY), (u) => u.unsetAll(uuidToId(SystemIds.IMAGE_URL_PROPERTY)))
							.createRelationSimple(
								uuidToId(f.relCoverY),
								uuidToId(f.pageY),
								uuidToId(f.imgY),
								uuidToId(f.relTypeAvatar),
							)
							.build(),
					contentUri: generateTestUri("fold-set-unset-media"),
				})
			} finally {
				client2.release()
			}
		})

		it("folds an edited block under its (unchanged) parent page; block is not top-level", async () => {
			const res = await v2App.request(`/v2/versioned/proposals/${f.propEditBlock}/diff?spaceId=${f.space}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			const ids = body.entities.map((e: {entityId: string}) => e.entityId)
			// Page A is the only root (resolved via BLOCKS backlink); block B is folded, not top-level.
			expect(ids).toContain(n(f.pageA))
			expect(ids).not.toContain(n(f.blockA))
			expect(body.pagination.totalEntities).toBe(1)
			const page = body.entities.find((e: {entityId: string}) => e.entityId === n(f.pageA))
			const block = page.blocks.find((b: {id: string}) => b.id === n(f.blockA))
			expect(block).toBeDefined()
			expect(block.type).toBe("textBlock")
			expect(block.before).toBe("old text")
			expect(block.after).toBe("new text")
		})

		it("drops a media-property entity and inlines its URL onto the parent relation", async () => {
			const res = await v2App.request(`/v2/versioned/proposals/${f.propAddAvatar}/diff?spaceId=${f.space}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			const ids = body.entities.map((e: {entityId: string}) => e.entityId)
			expect(ids).toContain(n(f.pageB))
			// The IMAGE entity is dropped from the top level; its URL is inlined instead.
			expect(ids).not.toContain(n(f.imgB))
			const page = body.entities.find((e: {entityId: string}) => e.entityId === n(f.pageB))
			const rel = page.relations.find((r: {after?: {toEntityId: string}}) => r.after?.toEntityId === n(f.imgB))
			expect(rel).toBeDefined()
			expect(rel.after.imageUrl).toBe("ipfs://foldtestimg")
		})

		it("folds data-block config (from the reified BLOCKS relation entity) into the block", async () => {
			const res = await v2App.request(`/v2/versioned/proposals/${f.propSetConfig}/diff?spaceId=${f.space}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			const ids = body.entities.map((e: {entityId: string}) => e.entityId)
			// Only the page is a root; the reified config entity is folded, not top-level.
			expect(ids).toContain(n(f.pageC))
			expect(ids).not.toContain(n(f.relBlocksC))
			const page = body.entities.find((e: {entityId: string}) => e.entityId === n(f.pageC))
			const block = page.blocks.find((b: {id: string}) => b.id === n(f.dataBlockC))
			expect(block).toBeDefined()
			expect(block.type).toBe("dataBlock")
			// The View config relation (authored on the reified entity) is folded onto the block.
			const viewRel = (block.relations ?? []).find(
				(r: {typeId: string; after?: {toEntityId: string}}) =>
					r.typeId === n(SystemIds.VIEW_PROPERTY) && r.after?.toEntityId === n(SystemIds.GALLERY_VIEW),
			)
			expect(viewRel).toBeDefined()
			expect(viewRel.changeType).toBe("ADD")
		})

		it("folds a video block (videoBlock type) under its parent", async () => {
			const res = await v2App.request(`/v2/versioned/proposals/${f.propEditVideo}/diff?spaceId=${f.space}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			const ids = body.entities.map((e: {entityId: string}) => e.entityId)
			expect(ids).toContain(n(f.pageV))
			expect(ids).not.toContain(n(f.videoV))
			const page = body.entities.find((e: {entityId: string}) => e.entityId === n(f.pageV))
			const block = page.blocks.find((b: {id: string}) => b.id === n(f.videoV))
			expect(block).toBeDefined()
			expect(block.type).toBe("videoBlock")
			expect(block.before).toBe("ipfs://oldvid")
			expect(block.after).toBe("ipfs://newvid")
		})

		it("folds a ranking block as a dataBlock", async () => {
			const res = await v2App.request(`/v2/versioned/proposals/${f.propEditRank}/diff?spaceId=${f.space}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			const page = body.entities.find((e: {entityId: string}) => e.entityId === n(f.pageR))
			expect(page).toBeDefined()
			const block = page.blocks.find((b: {id: string}) => b.id === n(f.rankR))
			expect(block).toBeDefined()
			expect(block.type).toBe("dataBlock")
			expect(block.before).toBe("Old Rank")
			expect(block.after).toBe("New Rank")
		})

		it("paginates over root entities (cursor crosses a page boundary)", async () => {
			// proposalMultipleOps changes 2 root entities; limit=1 forces two pages.
			const r1 = await v2App.request(
				`/v2/versioned/proposals/${uuid.proposalMultipleOps}/diff?spaceId=${uuid.space1}&limit=1`,
			)
			expect(r1.status).toBe(200)
			const b1 = await r1.json()
			expect(b1.entities.length).toBe(1)
			expect(b1.pagination.totalEntities).toBe(2)
			expect(b1.pagination.hasMore).toBe(true)
			expect(b1.pagination.cursor).toBeTruthy()

			const r2 = await v2App.request(
				`/v2/versioned/proposals/${uuid.proposalMultipleOps}/diff?spaceId=${uuid.space1}&limit=1&cursor=${encodeURIComponent(b1.pagination.cursor)}`,
			)
			expect(r2.status).toBe(200)
			const b2 = await r2.json()
			expect(b2.entities.length).toBe(1)
			expect(b2.pagination.hasMore).toBe(false)
			// The two pages cover distinct roots.
			const all = new Set([...b1.entities, ...b2.entities].map((e: {entityId: string}) => e.entityId))
			expect(all.size).toBe(2)
		})

		it("inlines the proposed (updated) URL for an existing DB-typed media entity, not the stale base URL", async () => {
			const res = await v2App.request(`/v2/versioned/proposals/${f.propUpdateMedia}/diff?spaceId=${f.space}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			const ids = body.entities.map((e: {entityId: string}) => e.entityId)
			expect(ids).toContain(n(f.pageU))
			expect(ids).not.toContain(n(f.imgU))
			const page = body.entities.find((e: {entityId: string}) => e.entityId === n(f.pageU))
			const rel = page.relations.find((r: {after?: {toEntityId: string}}) => r.after?.toEntityId === n(f.imgU))
			expect(rel).toBeDefined()
			// The edit updated the image's IMAGE_URL; the proposed URL must win over the base-version URL.
			expect(rel.after.imageUrl).toBe("ipfs://newcover")
		})

		it("keeps per-parent config when one data block is shared under two parents", async () => {
			const res = await v2App.request(`/v2/versioned/proposals/${f.propMultiConfig}/diff?spaceId=${f.space}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			const viewOf = (parentId: string) => {
				const page = body.entities.find((e: {entityId: string}) => e.entityId === n(parentId))
				const block = page?.blocks.find((b: {id: string}) => b.id === n(f.sharedBlock))
				const viewRel = (block?.relations ?? []).find(
					(r: {typeId: string}) => r.typeId === n(SystemIds.VIEW_PROPERTY),
				)
				return viewRel?.after?.toEntityId
			}
			// Each parent's fold must carry its OWN config — not a single block-keyed collision.
			expect(viewOf(f.pageM1)).toBe(n(SystemIds.GALLERY_VIEW))
			expect(viewOf(f.pageM2)).toBe(n(SystemIds.LIST_VIEW))
		})

		it("removes a relation's inlined media URL when the proposal unsets it", async () => {
			const res = await v2App.request(`/v2/versioned/proposals/${f.propUnsetMedia}/diff?spaceId=${f.space}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			const page = body.entities.find((e: {entityId: string}) => e.entityId === n(f.pageU))
			expect(page).toBeDefined()
			const rel = page.relations.find((r: {after?: {toEntityId: string}}) => r.after?.toEntityId === n(f.imgU))
			expect(rel).toBeDefined()
			// The proposal unset IMAGE_URL → no stale base-version URL should remain inlined.
			expect(rel.after.imageUrl ?? null).toBeNull()
			expect(rel.after.videoUrl ?? null).toBeNull()
		})

		it("does not inline a proposed media URL that the same edit later unsets (last write wins)", async () => {
			const res = await v2App.request(`/v2/versioned/proposals/${f.propSetUnsetMedia}/diff?spaceId=${f.space}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			const page = body.entities.find((e: {entityId: string}) => e.entityId === n(f.pageY))
			expect(page).toBeDefined()
			const rel = page.relations.find((r: {after?: {toEntityId: string}}) => r.after?.toEntityId === n(f.imgY))
			expect(rel).toBeDefined()
			// The edit set IMAGE_URL then unset it; the proposed url must not be inlined.
			expect(rel.after.imageUrl ?? null).toBeNull()
			expect(rel.after.videoUrl ?? null).toBeNull()
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
					.updateEntity(uuidToId(uuid.entityExisting2), (u) => u.setFloat64(uuidToId(uuid.propFloat), Math.E))
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
							.float64(uuidToId(uuid.propFloat), Math.PI),
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

	// Create proposal
	if (executedAt) {
		await client.query(
			`INSERT INTO proposals (id, space_id, proposed_by, voting_mode, start_time, end_time, quorum, threshold, executed_at, created_at, created_at_block)
			 VALUES ($1, $2, $3, 'Fast', $4, $5, 1, 1, $6, '2024-01-01T00:00:00Z', '2000') ON CONFLICT DO NOTHING`,
			[proposalId, spaceId, uuid.entityExisting1, startTime, endTime, executedAt],
		)
	} else {
		await client.query(
			`INSERT INTO proposals (id, space_id, proposed_by, voting_mode, start_time, end_time, quorum, threshold, created_at, created_at_block)
			 VALUES ($1, $2, $3, 'Fast', $4, $5, 1, 1, '2024-01-01T00:00:00Z', '2000') ON CONFLICT DO NOTHING`,
			[proposalId, spaceId, uuid.entityExisting1, startTime, endTime],
		)
	}

	// Create proposal action (Publish)
	await client.query(
		`INSERT INTO proposal_actions (id, proposal_id, action_type, content_uri)
		 VALUES ($1, $2, 'Publish', $3) ON CONFLICT DO NOTHING`,
		[actionId, proposalId, contentUri],
	)
}

async function cleanupTestData(pool: Pool): Promise<void> {
	const client = await pool.connect()

	try {
		await client.query("BEGIN")

		// Delete in reverse order of foreign key dependencies.
		// Covers both fixture prefixes used in this file: 20000000-* and 21000000-*.
		await client.query(`DELETE FROM proposal_actions WHERE proposal_id::text ~ '^2[01]000000-'`)
		await client.query(`DELETE FROM proposals WHERE id::text ~ '^2[01]000000-'`)
		await client.query(`DELETE FROM relations WHERE from_entity_id::text ~ '^2[01]000000-'`)
		await client.query(`DELETE FROM relation_versions WHERE from_entity_id::text ~ '^2[01]000000-'`)
		await client.query(`DELETE FROM "values" WHERE entity_id::text ~ '^2[01]000000-'`)
		await client.query(`DELETE FROM value_versions WHERE entity_id::text ~ '^2[01]000000-'`)
		await client.query(`DELETE FROM edit_versions WHERE edit_id::text ~ '^2[01]000000-'`)
		await client.query(`DELETE FROM entities WHERE id::text ~ '^2[01]000000-'`)
		await client.query(`DELETE FROM spaces WHERE id::text ~ '^2[01]000000-'`)
		await client.query(`DELETE FROM ipfs_cache WHERE uri LIKE 'ipfs://bafkreitest%'`)

		await client.query("COMMIT")
	} catch (error) {
		await client.query("ROLLBACK")
		console.warn("Cleanup error:", error)
	} finally {
		client.release()
	}
}
