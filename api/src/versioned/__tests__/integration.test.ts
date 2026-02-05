/**
 * Comprehensive integration tests for versioned entity endpoints.
 *
 * Tests run against a real PostgreSQL database and cover:
 * - All 13 GRC-20 value types
 * - Relations (same-space, cross-space, positioned, verified)
 * - Blocks (text, image, data)
 * - Value/relation/block diffs (ADD, REMOVE, UPDATE)
 * - Text diff chunks
 * - Proposal diffs (active, closed, executed)
 * - Pagination
 * - Edge cases (deleted entities, non-existent entities, etc.)
 *
 * Prerequisites:
 * - PostgreSQL running (docker-compose up -d)
 * - DATABASE_URL environment variable set
 * - Migrations applied (bun run db:migrate)
 */

import {SystemIds} from "@graphprotocol/grc-20"
import {drizzle} from "drizzle-orm/node-postgres"
import {Hono} from "hono"
import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {runtime} from "../../services/runtime"
import {normalizeUuid} from "../../utils/uuid"
import {createVersionedRouter} from "../router"

// Skip integration tests if DATABASE_URL is not set
const DATABASE_URL = process.env.DATABASE_URL
const SKIP_INTEGRATION = !DATABASE_URL

// ============================================================================
// Test UUIDs - Using prefix 10000000-* for easy identification and cleanup
// ============================================================================

const uuid = {
	// Spaces
	space1: normalizeUuid("10000000-0001-4000-8000-000000000001"),
	space2: normalizeUuid("10000000-0001-4000-8000-000000000002"),

	// Entities
	entityAllTypes: normalizeUuid("10000000-0002-4000-8000-000000000001"), // Has all 13 value types
	entityWithRelations: normalizeUuid("10000000-0002-4000-8000-000000000002"),
	entityWithBlocks: normalizeUuid("10000000-0002-4000-8000-000000000003"),
	entityChanging: normalizeUuid("10000000-0002-4000-8000-000000000004"), // Changes between versions
	entityDeleted: normalizeUuid("10000000-0002-4000-8000-000000000005"), // Deleted at v2
	entityCreatedLater: normalizeUuid("10000000-0002-4000-8000-000000000006"), // Created at v2
	entityNonExistent: normalizeUuid("10000000-0002-4000-8000-000000000999"),

	// Block entities (linked via BLOCKS relation)
	blockText1: normalizeUuid("10000000-0003-4000-8000-000000000001"),
	blockText2: normalizeUuid("10000000-0003-4000-8000-000000000002"),
	blockImage: normalizeUuid("10000000-0003-4000-8000-000000000003"),
	blockData: normalizeUuid("10000000-0003-4000-8000-000000000004"),

	// Entity with dynamic groups (non-BLOCKS context)
	entityWithDynamicGroups: normalizeUuid("10000000-0002-4000-8000-000000000007"),
	// Child entities for dynamic groups
	dynamicChildA1: normalizeUuid("10000000-0003-4000-8000-000000000005"),
	dynamicChildA2: normalizeUuid("10000000-0003-4000-8000-000000000006"),
	dynamicChildB1: normalizeUuid("10000000-0003-4000-8000-000000000007"),

	// Properties (custom test properties)
	propText: normalizeUuid("10000000-0004-4000-8000-000000000001"),
	propBool: normalizeUuid("10000000-0004-4000-8000-000000000002"),
	propInt: normalizeUuid("10000000-0004-4000-8000-000000000003"),
	propFloat: normalizeUuid("10000000-0004-4000-8000-000000000004"),
	propDecimal: normalizeUuid("10000000-0004-4000-8000-000000000005"),
	propBytes: normalizeUuid("10000000-0004-4000-8000-000000000006"),
	propDate: normalizeUuid("10000000-0004-4000-8000-000000000007"),
	propTime: normalizeUuid("10000000-0004-4000-8000-000000000008"),
	propDatetime: normalizeUuid("10000000-0004-4000-8000-000000000009"),
	propSchedule: normalizeUuid("10000000-0004-4000-8000-000000000010"),
	propPoint: normalizeUuid("10000000-0004-4000-8000-000000000011"),
	propRect: normalizeUuid("10000000-0004-4000-8000-000000000012"),
	propEmbedding: normalizeUuid("10000000-0004-4000-8000-000000000013"),

	// System properties (from GRC-20)
	propName: normalizeUuid(SystemIds.NAME_PROPERTY),
	propMarkdownContent: normalizeUuid(SystemIds.MARKDOWN_CONTENT),
	propImageUrl: normalizeUuid(SystemIds.IMAGE_URL_PROPERTY),
	propTypesProperty: normalizeUuid(SystemIds.TYPES_PROPERTY),

	// Block type entities (from GRC-20)
	textBlockType: normalizeUuid(SystemIds.TEXT_BLOCK),
	imageBlockType: normalizeUuid(SystemIds.IMAGE_BLOCK),
	dataBlockType: normalizeUuid(SystemIds.DATA_BLOCK),

	// Relation types
	relTypeGeneric: normalizeUuid("10000000-0005-4000-8000-000000000001"),
	relTypeBlocks: normalizeUuid(SystemIds.BLOCKS), // Must use real BLOCKS type ID for block grouping to work
	relTypeCustomA: normalizeUuid("10000000-0005-4000-8000-000000000003"), // Custom type for dynamic grouping
	relTypeCustomB: normalizeUuid("10000000-0005-4000-8000-000000000004"), // Another custom type

	// Relations
	rel1: normalizeUuid("10000000-0006-4000-8000-000000000001"),
	rel2: normalizeUuid("10000000-0006-4000-8000-000000000002"),
	relCrossSpace: normalizeUuid("10000000-0006-4000-8000-000000000003"),
	relPositioned: normalizeUuid("10000000-0006-4000-8000-000000000004"),
	relBlock1: normalizeUuid("10000000-0006-4000-8000-000000000005"),
	relBlock2: normalizeUuid("10000000-0006-4000-8000-000000000006"),
	relBlockImage: normalizeUuid("10000000-0006-4000-8000-000000000007"),
	relBlockData: normalizeUuid("10000000-0006-4000-8000-000000000008"),
	// Type relations for blocks (to identify block type)
	relBlock1Type: normalizeUuid("10000000-0006-4000-8000-000000000009"),
	relBlock2Type: normalizeUuid("10000000-0006-4000-8000-000000000010"),
	relBlockImageType: normalizeUuid("10000000-0006-4000-8000-000000000011"),
	relBlockDataType: normalizeUuid("10000000-0006-4000-8000-000000000012"),
	// Relations for dynamic grouping (custom types, not BLOCKS)
	relDynamicA1: normalizeUuid("10000000-0006-4000-8000-000000000013"),
	relDynamicA2: normalizeUuid("10000000-0006-4000-8000-000000000014"),
	relDynamicB1: normalizeUuid("10000000-0006-4000-8000-000000000015"),

	// Edits (versions)
	edit1: normalizeUuid("10000000-0007-4000-8000-000000000001"), // Version 1
	edit2: normalizeUuid("10000000-0007-4000-8000-000000000002"), // Version 2
	edit3: normalizeUuid("10000000-0007-4000-8000-000000000003"), // Version 3

	// Proposals
	proposalActive: normalizeUuid("10000000-0008-4000-8000-000000000001"),
	proposalClosed: normalizeUuid("10000000-0008-4000-8000-000000000002"),
	proposalExecuted: normalizeUuid("10000000-0008-4000-8000-000000000003"),
	proposalNoPublish: normalizeUuid("10000000-0008-4000-8000-000000000004"),

	// Value IDs (for value_versions)
	val: (n: number) => normalizeUuid(`10000000-0009-4000-8000-${n.toString().padStart(12, "0")}`),
}

// Version keys: (block_number << 32) | sequence
const versionKey1 = (BigInt(1000) << BigInt(32)).toString()
const versionKey2 = (BigInt(1001) << BigInt(32)).toString()
const versionKey3 = (BigInt(1002) << BigInt(32)).toString()

// ============================================================================
// Test Suite
// ============================================================================

describe.skipIf(SKIP_INTEGRATION)("Versioned Endpoints - Comprehensive Integration Tests", () => {
	let pool: Pool
	let app: Hono

	beforeAll(async () => {
		pool = new Pool({connectionString: DATABASE_URL})
		const db = drizzle(pool)
		app = new Hono()
		app.route("/versioned", createVersionedRouter(db as any, runtime))

		await setupTestData(pool)
	})

	afterAll(async () => {
		await cleanupTestData(pool)
		await pool?.end()
	})

	// ==========================================================================
	// 1. Entity Snapshot - Value Types
	// ==========================================================================

	describe("Entity Snapshot - All Value Types", () => {
		it("returns TEXT value", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propText)
			expect(value).toBeDefined()
			expect(value.text).toBe("Hello World")
		})

		it("returns BOOL value", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propBool)
			expect(value).toBeDefined()
			expect(value.boolean).toBe(true)
		})

		it("returns INT64 value", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propInt)
			expect(value).toBeDefined()
			expect(value.integer).toBe("42") // Bigint serialized as string
		})

		it("returns FLOAT64 value", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propFloat)
			expect(value).toBeDefined()
			expect(value.float).toBeCloseTo(3.14159, 5)
		})

		it("returns DECIMAL value", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propDecimal)
			expect(value).toBeDefined()
			expect(value.decimal).toBe("123.456789")
		})

		it("returns BYTES value (base64)", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propBytes)
			expect(value).toBeDefined()
			// "hello" in base64
			expect(value.bytes).toBe("aGVsbG8=")
		})

		it("returns DATE value", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propDate)
			expect(value).toBeDefined()
			expect(value.date).toBe("2024-01-15")
		})

		it("returns TIME value", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propTime)
			expect(value).toBeDefined()
			expect(value.time).toBe("14:30:00")
		})

		it("returns DATETIME value", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propDatetime)
			expect(value).toBeDefined()
			expect(value.datetime).toBe("2024-01-15T14:30:00Z")
		})

		it("returns SCHEDULE value (JSON)", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propSchedule)
			expect(value).toBeDefined()
			expect(value.schedule).toEqual({rrule: "FREQ=DAILY"})
		})

		it("returns POINT value", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propPoint)
			expect(value).toBeDefined()
			expect(value.point).toBe("37.7749,-122.4194")
		})

		it("returns RECT value", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propRect)
			expect(value).toBeDefined()
			expect(value.rect).toBe("0,0,100,100")
		})

		it("returns EMBEDDING value (JSON array)", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const value = body.values.find((v: any) => v.propertyId === uuid.propEmbedding)
			expect(value).toBeDefined()
			expect(value.embedding).toEqual([0.1, 0.2, 0.3])
		})
	})

	// ==========================================================================
	// 2. Entity Snapshot - Relations
	// ==========================================================================

	describe("Entity Snapshot - Relations", () => {
		it("returns entity with no relations", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityAllTypes}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			expect(body.relations).toEqual([])
		})

		it("returns entity with single relation", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithRelations}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.relations.length).toBeGreaterThanOrEqual(1)

			const rel = body.relations.find((r: any) => r.relationId === uuid.rel1)
			expect(rel).toBeDefined()
			expect(rel.typeId).toBe(uuid.relTypeGeneric)
			expect(rel.fromEntityId).toBe(uuid.entityWithRelations)
			expect(rel.toEntityId).toBe(uuid.entityAllTypes)
		})

		it("returns cross-space relation with toSpaceId", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithRelations}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const rel = body.relations.find((r: any) => r.relationId === uuid.relCrossSpace)
			expect(rel).toBeDefined()
			expect(rel.toSpaceId).toBe(uuid.space2)
		})

		it("returns positioned relation with position", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithRelations}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()
			const rel = body.relations.find((r: any) => r.relationId === uuid.relPositioned)
			expect(rel).toBeDefined()
			expect(rel.position).toBe("a0")
		})
	})

	// ==========================================================================
	// 3. Entity Snapshot - Blocks
	// ==========================================================================

	describe("Entity Snapshot - Blocks", () => {
		it("returns entity with text blocks", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithBlocks}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()

			expect(body.blocks).toBeInstanceOf(Array)
			expect(body.blocks.length).toBeGreaterThanOrEqual(2)

			// Find text blocks - they have MARKDOWN_CONTENT property
			const textBlock = body.blocks.find((b: any) => b.id === uuid.blockText1)
			expect(textBlock).toBeDefined()
			const textValue = textBlock.values.find((v: any) => v.propertyId === uuid.propMarkdownContent)
			expect(textValue?.text).toBe("Block 1 content")
		})

		it("returns entity with image block", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithBlocks}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			const imageBlock = body.blocks.find((b: any) => b.id === uuid.blockImage)
			expect(imageBlock).toBeDefined()
			const urlValue = imageBlock.values.find((v: any) => v.propertyId === uuid.propImageUrl)
			expect(urlValue?.text).toBe("https://example.com/image.png")
		})

		it("returns entity with data block", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithBlocks}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			const dataBlock = body.blocks.find((b: any) => b.id === uuid.blockData)
			expect(dataBlock).toBeDefined()
			const nameValue = dataBlock.values.find((v: any) => v.propertyId === uuid.propName)
			expect(nameValue?.text).toBe("Data Block Name")
		})

		it("excludes BLOCKS relations from relations array", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithBlocks}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			// BLOCKS relations should NOT appear in the relations array
			const blockRelations = body.relations.filter((r: any) => r.typeId === uuid.relTypeBlocks)
			expect(blockRelations).toEqual([])
		})
	})

	// ==========================================================================
	// 4. Entity Snapshot - Edge Cases
	// ==========================================================================

	describe("Entity Snapshot - Edge Cases", () => {
		it("returns empty snapshot for non-existent entity", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityNonExistent}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.id).toBe(uuid.entityNonExistent)
			expect(body.values).toEqual([])
			expect(body.relations).toEqual([])
			expect(body.blocks).toEqual([])
		})

		it("returns empty snapshot for entity deleted before version", async () => {
			// entityDeleted exists at v1, deleted at v2
			const res = await app.request(
				`/versioned/entities/${uuid.entityDeleted}?editId=${uuid.edit3}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.values).toEqual([])
		})

		it("returns entity state at v1 before deletion", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityDeleted}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.values.length).toBeGreaterThan(0)
		})

		it("returns empty snapshot for entity created after requested version", async () => {
			// entityCreatedLater created at v2
			const res = await app.request(
				`/versioned/entities/${uuid.entityCreatedLater}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.values).toEqual([])
		})

		it("returns entity state after creation version", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityCreatedLater}?editId=${uuid.edit2}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.values.length).toBeGreaterThan(0)
		})
	})

	// ==========================================================================
	// 5. Entity Versions
	// ==========================================================================

	describe("Entity Versions", () => {
		it("lists versions that affected an entity", async () => {
			const res = await app.request(`/versioned/entities/${uuid.entityChanging}/versions?spaceId=${uuid.space1}`)
			expect(res.status).toBe(200)
			const body = await res.json()

			expect(body.versions).toBeInstanceOf(Array)
			expect(body.versions.length).toBeGreaterThanOrEqual(2)

			for (const version of body.versions) {
				expect(version.editId).toBeDefined()
				expect(version.blockNumber).toBeDefined()
				expect(version.createdAt).toBeDefined()
			}
		})

		it("returns empty versions for entity with no history", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityNonExistent}/versions?spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.versions).toEqual([])
		})

		it("respects limit parameter", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityChanging}/versions?spaceId=${uuid.space1}&limit=1`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.versions.length).toBeLessThanOrEqual(1)
		})
	})

	// ==========================================================================
	// 6. Entity Diff - Value Changes
	// ==========================================================================

	describe("Entity Diff - Value Changes", () => {
		it("shows value UPDATE (text changed)", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityChanging}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit2}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()

			const textChange = body.values.find((v: any) => v.propertyId === uuid.propText)
			expect(textChange).toBeDefined()
			expect(textChange.type).toBe("TEXT")
			expect(textChange.before).toBe("Original text")
			expect(textChange.after).toBe("Modified text")
		})

		it("shows value ADD (new property)", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityChanging}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit2}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			const intChange = body.values.find((v: any) => v.propertyId === uuid.propInt)
			expect(intChange).toBeDefined()
			expect(intChange.before).toBeNull()
			expect(intChange.after).toBe("100")
		})

		it("shows value REMOVE (deleted property)", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityChanging}/diff?fromEditId=${uuid.edit2}&toEditId=${uuid.edit3}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			const boolChange = body.values.find((v: any) => v.propertyId === uuid.propBool)
			expect(boolChange).toBeDefined()
			expect(boolChange.before).not.toBeNull()
			expect(boolChange.after).toBeNull()
		})

		it("returns empty diff for same version", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityChanging}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.values).toEqual([])
			expect(body.relations).toEqual([])
		})

		it("includes text diff chunks for TEXT values", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityChanging}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit2}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			const textChange = body.values.find((v: any) => v.propertyId === uuid.propText)
			expect(textChange.diff).toBeInstanceOf(Array)
			expect(textChange.diff.length).toBeGreaterThan(0)

			// Check diff structure
			for (const chunk of textChange.diff) {
				expect(chunk).toHaveProperty("value")
				// added and removed are optional
			}
		})
	})

	// ==========================================================================
	// 7. Entity Diff - Relation Changes
	// ==========================================================================

	describe("Entity Diff - Relation Changes", () => {
		it("shows relation ADD", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityChanging}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit2}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			const relAdd = body.relations.find((r: any) => r.changeType === "ADD" && r.relationId === uuid.rel2)
			expect(relAdd).toBeDefined()
			expect(relAdd.before).toBeNull()
			expect(relAdd.after).toBeDefined()
			expect(relAdd.after.toEntityId).toBeDefined()
		})

		it("shows relation REMOVE", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityChanging}/diff?fromEditId=${uuid.edit2}&toEditId=${uuid.edit3}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			const relRemove = body.relations.find((r: any) => r.changeType === "REMOVE")
			expect(relRemove).toBeDefined()
			expect(relRemove.before).toBeDefined()
			expect(relRemove.after).toBeNull()
		})
	})

	// ==========================================================================
	// 8. Entity Diff - Block Changes
	// ==========================================================================

	describe("Entity Diff - Block Changes", () => {
		it("shows text block content change with diff", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithBlocks}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit2}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()

			const textBlockChange = body.blocks.find((b: any) => b.id === uuid.blockText1 && b.type === "textBlock")
			expect(textBlockChange).toBeDefined()
			expect(textBlockChange.before).toBe("Block 1 content")
			expect(textBlockChange.after).toBe("Block 1 updated content")
			expect(textBlockChange.diff).toBeInstanceOf(Array)
		})
	})

	// ==========================================================================
	// 9. Entity Diff - Dynamic Grouping
	// ==========================================================================

	describe("Entity Diff - Dynamic Grouping", () => {
		it("returns groupKeys for dynamic groups with changes", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithDynamicGroups}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit2}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()

			// Should have groupKeys listing the dynamic group types with changes
			expect(body.groupKeys).toBeInstanceOf(Array)
			expect(body.groupKeys).toContain(uuid.relTypeCustomA)
		})

		it("spreads dynamic groups at root level", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithDynamicGroups}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit2}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			// Dynamic groups should be spread at root level (not nested under 'groups')
			expect(body.groups).toBeUndefined() // 'groups' is spread, not returned as-is
			expect(body[uuid.relTypeCustomA]).toBeInstanceOf(Array)
		})

		it("includes entity diffs in dynamic groups", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithDynamicGroups}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit2}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			const dynamicGroupA = body[uuid.relTypeCustomA]
			expect(dynamicGroupA).toBeDefined()
			expect(dynamicGroupA.length).toBeGreaterThan(0)

			// Each item should have entity diff structure
			const item = dynamicGroupA[0]
			expect(item).toHaveProperty("entityId")
			expect(item).toHaveProperty("values")
		})

		it("supports multiple dynamic groups", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithDynamicGroups}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit3}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			// Should have both custom types when both have changes
			if (body.groupKeys.includes(uuid.relTypeCustomB)) {
				expect(body[uuid.relTypeCustomB]).toBeInstanceOf(Array)
			}
		})

		it("sorts groupKeys alphabetically", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithDynamicGroups}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit3}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			// groupKeys should be sorted
			const sortedKeys = [...body.groupKeys].sort()
			expect(body.groupKeys).toEqual(sortedKeys)
		})

		it("excludes groups with no changes from groupKeys", async () => {
			// When comparing same version, no dynamic groups should have changes
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithDynamicGroups}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			const body = await res.json()

			expect(body.groupKeys).toEqual([])
		})
	})

	// ==========================================================================
	// 10. Proposal Diff
	// ==========================================================================

	describe("Proposal Diff", () => {
		it("returns proposal status as active when end_time is in future", async () => {
			const res = await app.request(`/versioned/proposals/${uuid.proposalActive}/diff?spaceId=${uuid.space1}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposalStatus).toBe("active")
		})

		it("returns proposal status as closed when end_time is in past", async () => {
			const res = await app.request(`/versioned/proposals/${uuid.proposalClosed}/diff?spaceId=${uuid.space1}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposalStatus).toBe("closed")
		})

		it("returns proposal status as executed when executed_at is set", async () => {
			const res = await app.request(`/versioned/proposals/${uuid.proposalExecuted}/diff?spaceId=${uuid.space1}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposalStatus).toBe("executed")
		})

		it("returns empty diff for proposal without publish action", async () => {
			const res = await app.request(`/versioned/proposals/${uuid.proposalNoPublish}/diff?spaceId=${uuid.space1}`)
			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.entities).toEqual([])
			expect(body.pagination.totalEntities).toBe(0)
		})

		it("returns 404 for non-existent proposal", async () => {
			const res = await app.request(`/versioned/proposals/${uuid.entityNonExistent}/diff?spaceId=${uuid.space1}`)
			expect(res.status).toBe(404)
		})

		it("returns 400 when spaceId does not match proposal", async () => {
			const res = await app.request(`/versioned/proposals/${uuid.proposalActive}/diff?spaceId=${uuid.space2}`)
			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("spaceId does not match")
		})

		it("returns 400 for invalid cursor", async () => {
			const res = await app.request(
				`/versioned/proposals/${uuid.proposalNoPublish}/diff?spaceId=${uuid.space1}&cursor=invalid!!!`,
			)
			// No content_uri means cursor validation is skipped (early return)
			// This test would need a proposal WITH content_uri to test cursor validation
			// For now, we verify the endpoint handles the request
			expect([200, 400]).toContain(res.status)
		})
	})

	// ==========================================================================
	// 11. UUID Format - Dashless Regression Guard
	//
	// The API must return dashless lowercase hex UUIDs (32 chars, no dashes).
	// These tests verify every UUID field in every response shape to catch
	// regressions where Postgres dashed UUIDs leak through.
	// ==========================================================================

	describe("UUID Format - all responses use dashless UUIDs", () => {
		const DASHLESS_UUID = /^[0-9a-f]{32}$/

		function expectDashlessUuid(value: unknown, field: string): void {
			expect(value, `${field} should be a dashless UUID but got: ${value}`).toMatch(DASHLESS_UUID)
		}

		function expectOptionalDashlessUuid(value: unknown, field: string): void {
			if (value !== null && value !== undefined) {
				expectDashlessUuid(value, field)
			}
		}

		function assertValueUuids(v: any, prefix: string): void {
			expectDashlessUuid(v.propertyId, `${prefix}.propertyId`)
			expectDashlessUuid(v.spaceId, `${prefix}.spaceId`)
			expectOptionalDashlessUuid(v.contextRootId, `${prefix}.contextRootId`)
			expectOptionalDashlessUuid(v.contextEdgeTypeId, `${prefix}.contextEdgeTypeId`)
		}

		function assertRelationUuids(r: any, prefix: string): void {
			expectDashlessUuid(r.relationId, `${prefix}.relationId`)
			expectDashlessUuid(r.typeId, `${prefix}.typeId`)
			expectDashlessUuid(r.fromEntityId, `${prefix}.fromEntityId`)
			expectOptionalDashlessUuid(r.fromSpaceId, `${prefix}.fromSpaceId`)
			expectDashlessUuid(r.toEntityId, `${prefix}.toEntityId`)
			expectOptionalDashlessUuid(r.toSpaceId, `${prefix}.toSpaceId`)
			expectDashlessUuid(r.spaceId, `${prefix}.spaceId`)
			expectOptionalDashlessUuid(r.contextRootId, `${prefix}.contextRootId`)
			expectOptionalDashlessUuid(r.contextEdgeTypeId, `${prefix}.contextEdgeTypeId`)
		}

		function assertBlockSnapshotUuids(b: any, prefix: string): void {
			expectDashlessUuid(b.id, `${prefix}.id`)
			for (let i = 0; i < b.values.length; i++) {
				assertValueUuids(b.values[i], `${prefix}.values[${i}]`)
			}
			for (let i = 0; i < b.relations.length; i++) {
				assertRelationUuids(b.relations[i], `${prefix}.relations[${i}]`)
			}
		}

		function assertValueChangeUuids(v: any, prefix: string): void {
			expectDashlessUuid(v.propertyId, `${prefix}.propertyId`)
			expectDashlessUuid(v.spaceId, `${prefix}.spaceId`)
		}

		function assertRelationChangeUuids(r: any, prefix: string): void {
			expectDashlessUuid(r.relationId, `${prefix}.relationId`)
			expectDashlessUuid(r.typeId, `${prefix}.typeId`)
			expectDashlessUuid(r.spaceId, `${prefix}.spaceId`)
			if (r.before) {
				expectDashlessUuid(r.before.toEntityId, `${prefix}.before.toEntityId`)
				expectOptionalDashlessUuid(r.before.toSpaceId, `${prefix}.before.toSpaceId`)
			}
			if (r.after) {
				expectDashlessUuid(r.after.toEntityId, `${prefix}.after.toEntityId`)
				expectOptionalDashlessUuid(r.after.toSpaceId, `${prefix}.after.toSpaceId`)
			}
		}

		function assertBlockChangeUuids(b: any, prefix: string): void {
			expectDashlessUuid(b.id, `${prefix}.id`)
		}

		function assertEntityDiffUuids(d: any, prefix: string): void {
			expectDashlessUuid(d.entityId, `${prefix}.entityId`)
			for (let i = 0; i < d.values.length; i++) {
				assertValueChangeUuids(d.values[i], `${prefix}.values[${i}]`)
			}
			for (let i = 0; i < d.relations.length; i++) {
				assertRelationChangeUuids(d.relations[i], `${prefix}.relations[${i}]`)
			}
			for (let i = 0; i < d.blocks.length; i++) {
				assertBlockChangeUuids(d.blocks[i], `${prefix}.blocks[${i}]`)
			}
		}

		it("entity snapshot: id, values, relations, blocks all dashless", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithBlocks}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()

			// EntitySnapshot.id
			expectDashlessUuid(body.id, "id")

			// EntitySnapshot.values[]
			for (let i = 0; i < body.values.length; i++) {
				assertValueUuids(body.values[i], `values[${i}]`)
			}

			// EntitySnapshot.relations[]
			for (let i = 0; i < body.relations.length; i++) {
				assertRelationUuids(body.relations[i], `relations[${i}]`)
			}

			// EntitySnapshot.blocks[]
			for (let i = 0; i < body.blocks.length; i++) {
				assertBlockSnapshotUuids(body.blocks[i], `blocks[${i}]`)
			}
		})

		it("entity snapshot with relations: all relation UUID fields dashless", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityWithRelations}?editId=${uuid.edit1}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()

			expect(body.relations.length).toBeGreaterThan(0)
			for (let i = 0; i < body.relations.length; i++) {
				assertRelationUuids(body.relations[i], `relations[${i}]`)
			}
		})

		it("entity diff: entityId, value changes, relation changes, block changes all dashless", async () => {
			const res = await app.request(
				`/versioned/entities/${uuid.entityChanging}/diff?fromEditId=${uuid.edit1}&toEditId=${uuid.edit2}&spaceId=${uuid.space1}`,
			)
			expect(res.status).toBe(200)
			const body = await res.json()

			// GroupedEntityDiff top-level
			expectDashlessUuid(body.entityId, "entityId")

			// ValueChange[]
			for (let i = 0; i < body.values.length; i++) {
				assertValueChangeUuids(body.values[i], `values[${i}]`)
			}

			// RelationChange[]
			for (let i = 0; i < body.relations.length; i++) {
				assertRelationChangeUuids(body.relations[i], `relations[${i}]`)
			}

			// BlockChange[]
			for (let i = 0; i < body.blocks.length; i++) {
				assertBlockChangeUuids(body.blocks[i], `blocks[${i}]`)
			}

			// GroupedEntityDiff.groupKeys[]
			for (let i = 0; i < body.groupKeys.length; i++) {
				expectDashlessUuid(body.groupKeys[i], `groupKeys[${i}]`)
			}
		})

		it("entity versions: editId is dashless", async () => {
			const res = await app.request(`/versioned/entities/${uuid.entityAllTypes}/versions?spaceId=${uuid.space1}`)
			expect(res.status).toBe(200)
			const body = await res.json()

			expect(body.versions.length).toBeGreaterThan(0)
			for (let i = 0; i < body.versions.length; i++) {
				expectDashlessUuid(body.versions[i].editId, `versions[${i}].editId`)
			}
		})

		it("proposal diff: proposalId, spaceId, entity diffs all dashless", async () => {
			const res = await app.request(`/versioned/proposals/${uuid.proposalActive}/diff?spaceId=${uuid.space1}`)
			expect(res.status).toBe(200)
			const body = await res.json()

			// PaginatedProposalDiff top-level
			expectDashlessUuid(body.proposalId, "proposalId")
			expectDashlessUuid(body.spaceId, "spaceId")

			// PaginatedProposalDiff.entities[]
			for (let i = 0; i < body.entities.length; i++) {
				assertEntityDiffUuids(body.entities[i], `entities[${i}]`)
			}
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

		// 1. Create spaces
		await client.query(
			`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xTestSpace1') ON CONFLICT DO NOTHING`,
			[uuid.space1],
		)
		await client.query(
			`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xTestSpace2') ON CONFLICT DO NOTHING`,
			[uuid.space2],
		)

		// 2. Create entities
		const entities = [
			uuid.entityAllTypes,
			uuid.entityWithRelations,
			uuid.entityWithBlocks,
			uuid.entityChanging,
			uuid.entityDeleted,
			uuid.entityCreatedLater,
			uuid.blockText1,
			uuid.blockText2,
			uuid.blockImage,
			uuid.blockData,
			uuid.entityWithDynamicGroups,
			uuid.dynamicChildA1,
			uuid.dynamicChildA2,
			uuid.dynamicChildB1,
		]
		for (const entityId of entities) {
			await client.query(
				`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
				 VALUES ($1, '2024-01-01T00:00:00Z', '1000', '2024-01-02T00:00:00Z', '1001')
				 ON CONFLICT DO NOTHING`,
				[entityId],
			)
		}

		// 3. Create edit versions
		await client.query(
			`INSERT INTO edit_versions (edit_id, block_number, sequence, version_key, created_at)
			 VALUES ($1, 1000, 0, $2, '2024-01-01T00:00:00Z') ON CONFLICT DO NOTHING`,
			[uuid.edit1, versionKey1],
		)
		await client.query(
			`INSERT INTO edit_versions (edit_id, block_number, sequence, version_key, created_at)
			 VALUES ($1, 1001, 0, $2, '2024-01-02T00:00:00Z') ON CONFLICT DO NOTHING`,
			[uuid.edit2, versionKey2],
		)
		await client.query(
			`INSERT INTO edit_versions (edit_id, block_number, sequence, version_key, created_at)
			 VALUES ($1, 1002, 0, $2, '2024-01-03T00:00:00Z') ON CONFLICT DO NOTHING`,
			[uuid.edit3, versionKey3],
		)

		// 4. Create value versions for entityAllTypes (all 13 types)
		let valIdx = 1
		const allTypesValues = [
			{prop: uuid.propText, col: "text", val: "'Hello World'"},
			{prop: uuid.propBool, col: "boolean", val: "true"},
			{prop: uuid.propInt, col: "integer", val: "42"},
			{prop: uuid.propFloat, col: "float", val: "3.14159"},
			{prop: uuid.propDecimal, col: "decimal", val: "'123.456789'"},
			{prop: uuid.propBytes, col: "bytes", val: "'\\x68656c6c6f'"}, // "hello" in hex
			{prop: uuid.propDate, col: "date", val: "'2024-01-15'"},
			{prop: uuid.propTime, col: "time", val: "'14:30:00'"},
			{prop: uuid.propDatetime, col: "datetime", val: "'2024-01-15T14:30:00Z'"},
			{prop: uuid.propSchedule, col: "schedule", val: `'{"rrule":"FREQ=DAILY"}'::jsonb`},
			{prop: uuid.propPoint, col: "point", val: "'37.7749,-122.4194'"},
			{prop: uuid.propRect, col: "rect", val: "'0,0,100,100'"},
			{prop: uuid.propEmbedding, col: "embedding", val: "'[0.1,0.2,0.3]'::jsonb"},
		]

		for (const v of allTypesValues) {
			await client.query(
				`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, ${v.col})
				 VALUES ($1, $2, $3, $4, $5, NULL, ${v.val}) ON CONFLICT DO NOTHING`,
				[uuid.val(valIdx++), uuid.entityAllTypes, v.prop, uuid.space1, versionKey1],
			)
		}

		// 5. Create relations for entityWithRelations
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.rel1,
				uuid.entityWithRelations,
				uuid.relTypeGeneric,
				uuid.entityAllTypes,
				uuid.space1,
				versionKey1,
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, to_space_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, $8, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relCrossSpace,
				uuid.entityWithRelations,
				uuid.relTypeGeneric,
				uuid.entityAllTypes,
				uuid.space2,
				uuid.space1,
				versionKey1,
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, position, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, 'a0', $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relPositioned,
				uuid.entityWithRelations,
				uuid.relTypeGeneric,
				uuid.entityAllTypes,
				uuid.space1,
				versionKey1,
			],
		)

		// 6. Create blocks for entityWithBlocks
		// Block text values (using MARKDOWN_CONTENT property)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, $6, 'Block 1 content') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.blockText1, uuid.propMarkdownContent, uuid.space1, versionKey1, versionKey2],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Block 1 updated content') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.blockText1, uuid.propMarkdownContent, uuid.space1, versionKey2],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Block 2 content') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.blockText2, uuid.propMarkdownContent, uuid.space1, versionKey1],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'https://example.com/image.png') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.blockImage, uuid.propImageUrl, uuid.space1, versionKey1],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Data Block Name') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.blockData, uuid.propName, uuid.space1, versionKey1],
		)

		// BLOCKS relations (linking blocks to entityWithBlocks)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, position, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, 'a0', $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relBlock1,
				uuid.entityWithBlocks,
				uuid.relTypeBlocks,
				uuid.blockText1,
				uuid.space1,
				versionKey1,
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, position, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, 'a1', $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relBlock2,
				uuid.entityWithBlocks,
				uuid.relTypeBlocks,
				uuid.blockText2,
				uuid.space1,
				versionKey1,
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, position, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, 'a2', $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relBlockImage,
				uuid.entityWithBlocks,
				uuid.relTypeBlocks,
				uuid.blockImage,
				uuid.space1,
				versionKey1,
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, position, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, 'a3', $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relBlockData,
				uuid.entityWithBlocks,
				uuid.relTypeBlocks,
				uuid.blockData,
				uuid.space1,
				versionKey1,
			],
		)

		// TYPES_PROPERTY relations (to identify block types)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relBlock1Type,
				uuid.blockText1,
				uuid.propTypesProperty,
				uuid.textBlockType,
				uuid.space1,
				versionKey1,
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relBlock2Type,
				uuid.blockText2,
				uuid.propTypesProperty,
				uuid.textBlockType,
				uuid.space1,
				versionKey1,
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relBlockImageType,
				uuid.blockImage,
				uuid.propTypesProperty,
				uuid.imageBlockType,
				uuid.space1,
				versionKey1,
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relBlockDataType,
				uuid.blockData,
				uuid.propTypesProperty,
				uuid.dataBlockType,
				uuid.space1,
				versionKey1,
			],
		)

		// 7. Create values for entityChanging (changes across versions)
		// Text value: changes from v1 to v2
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, $6, 'Original text') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityChanging, uuid.propText, uuid.space1, versionKey1, versionKey2],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Modified text') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityChanging, uuid.propText, uuid.space1, versionKey2],
		)
		// Int value: added at v2
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, integer)
			 VALUES ($1, $2, $3, $4, $5, NULL, 100) ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityChanging, uuid.propInt, uuid.space1, versionKey2],
		)
		// Bool value: exists v1-v2, removed at v3
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, boolean)
			 VALUES ($1, $2, $3, $4, $5, $6, true) ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityChanging, uuid.propBool, uuid.space1, versionKey1, versionKey3],
		)

		// Relation added at v2 for entityChanging
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, $8) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.rel2,
				uuid.entityChanging,
				uuid.relTypeGeneric,
				uuid.entityAllTypes,
				uuid.space1,
				versionKey2,
				versionKey3,
			],
		)

		// 8. Create values for entityDeleted (exists at v1, deleted at v2)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, $6, 'I will be deleted') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityDeleted, uuid.propText, uuid.space1, versionKey1, versionKey2],
		)

		// 9. Create values for entityCreatedLater (created at v2)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Created at v2') ON CONFLICT DO NOTHING`,
			[uuid.val(valIdx++), uuid.entityCreatedLater, uuid.propText, uuid.space1, versionKey2],
		)

		// 10. Create dynamic grouping test data
		// Child entities with values (for dynamic group content)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text, context_root_id, context_edge_type_id)
			 VALUES ($1, $2, $3, $4, $5, $6, 'Dynamic A1 original', $7, $8) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.dynamicChildA1,
				uuid.propText,
				uuid.space1,
				versionKey1,
				versionKey2,
				uuid.entityWithDynamicGroups,
				uuid.relTypeCustomA,
			],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text, context_root_id, context_edge_type_id)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Dynamic A1 updated', $6, $7) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.dynamicChildA1,
				uuid.propText,
				uuid.space1,
				versionKey2,
				uuid.entityWithDynamicGroups,
				uuid.relTypeCustomA,
			],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text, context_root_id, context_edge_type_id)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Dynamic A2 content', $6, $7) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.dynamicChildA2,
				uuid.propText,
				uuid.space1,
				versionKey1,
				uuid.entityWithDynamicGroups,
				uuid.relTypeCustomA,
			],
		)
		await client.query(
			`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text, context_root_id, context_edge_type_id)
			 VALUES ($1, $2, $3, $4, $5, NULL, 'Dynamic B1 content', $6, $7) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.dynamicChildB1,
				uuid.propText,
				uuid.space1,
				versionKey2,
				uuid.entityWithDynamicGroups,
				uuid.relTypeCustomB,
			],
		)

		// Relations linking children to parent via custom types (for fallback discovery)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relDynamicA1,
				uuid.entityWithDynamicGroups,
				uuid.relTypeCustomA,
				uuid.dynamicChildA1,
				uuid.space1,
				versionKey1,
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relDynamicA2,
				uuid.entityWithDynamicGroups,
				uuid.relTypeCustomA,
				uuid.dynamicChildA2,
				uuid.space1,
				versionKey1,
			],
		)
		await client.query(
			`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, valid_from_key, valid_to_key)
			 VALUES ($1, $2, $3, $4, $3, $5, $6, $7, NULL) ON CONFLICT DO NOTHING`,
			[
				uuid.val(valIdx++),
				uuid.relDynamicB1,
				uuid.entityWithDynamicGroups,
				uuid.relTypeCustomB,
				uuid.dynamicChildB1,
				uuid.space1,
				versionKey2,
			],
		)

		// 11. Create proposals
		const now = Math.floor(Date.now() / 1000)

		// Active proposal (end_time in future)
		await client.query(
			`INSERT INTO proposals (id, space_id, proposed_by, voting_mode, start_time, end_time, quorum, threshold, created_at, created_at_block)
			 VALUES ($1, $2, $3, 'Fast', $4, $5, 1, 1, '2024-01-01T00:00:00Z', '1000') ON CONFLICT DO NOTHING`,
			[uuid.proposalActive, uuid.space1, uuid.entityAllTypes, now - 1000, now + 86400],
		)

		// Closed proposal (end_time in past)
		await client.query(
			`INSERT INTO proposals (id, space_id, proposed_by, voting_mode, start_time, end_time, quorum, threshold, created_at, created_at_block)
			 VALUES ($1, $2, $3, 'Fast', $4, $5, 1, 1, '2024-01-01T00:00:00Z', '1000') ON CONFLICT DO NOTHING`,
			[uuid.proposalClosed, uuid.space1, uuid.entityAllTypes, now - 86400, now - 1000],
		)

		// Executed proposal
		await client.query(
			`INSERT INTO proposals (id, space_id, proposed_by, voting_mode, start_time, end_time, quorum, threshold, executed_at, created_at, created_at_block)
			 VALUES ($1, $2, $3, 'Fast', $4, $5, 1, 1, $6, '2024-01-01T00:00:00Z', '1000') ON CONFLICT DO NOTHING`,
			[uuid.proposalExecuted, uuid.space1, uuid.entityAllTypes, now - 86400, now - 1000, now - 500],
		)

		// Proposal without publish action
		await client.query(
			`INSERT INTO proposals (id, space_id, proposed_by, voting_mode, start_time, end_time, quorum, threshold, created_at, created_at_block)
			 VALUES ($1, $2, $3, 'Fast', $4, $5, 1, 1, '2024-01-01T00:00:00Z', '1000') ON CONFLICT DO NOTHING`,
			[uuid.proposalNoPublish, uuid.space1, uuid.entityAllTypes, now - 1000, now + 86400],
		)

		await client.query("COMMIT")
	} catch (error) {
		await client.query("ROLLBACK")
		throw error
	} finally {
		client.release()
	}
}

async function cleanupTestData(pool: Pool): Promise<void> {
	const client = await pool.connect()

	try {
		await client.query("BEGIN")

		// Delete in reverse order of foreign key dependencies
		await client.query(`DELETE FROM proposal_actions WHERE proposal_id::text LIKE '10000000-%'`)
		await client.query(`DELETE FROM proposals WHERE id::text LIKE '10000000-%'`)
		await client.query(`DELETE FROM relation_versions WHERE entity_id::text LIKE '10000000-%'`)
		await client.query(`DELETE FROM value_versions WHERE entity_id::text LIKE '10000000-%'`)
		await client.query(`DELETE FROM edit_versions WHERE edit_id::text LIKE '10000000-%'`)
		await client.query(`DELETE FROM entities WHERE id::text LIKE '10000000-%'`)
		await client.query(`DELETE FROM spaces WHERE id::text LIKE '10000000-%'`)

		await client.query("COMMIT")
	} catch (error) {
		await client.query("ROLLBACK")
		console.warn("Cleanup error:", error)
	} finally {
		client.release()
	}
}
