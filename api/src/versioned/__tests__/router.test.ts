import {encodeEdit, type Op, randomId} from "@geoprotocol/grc-20"
import {Effect} from "effect"
import {Hono} from "hono"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {normalizeUuid} from "../../utils/uuid"
import {createVersionedRouter} from "../router"

// =============================================================================
// Test Setup
// =============================================================================

// Valid UUIDs for use in mock DB rows (these pass through normalizeUuid)
const PROP_1 = "00000000-0000-0000-0000-000000000a01"
const SPACE_1 = "00000000-0000-0000-0000-000000000b01"
const EDIT_1 = "00000000-0000-0000-0000-000000000c01"
const EDIT_2 = "00000000-0000-0000-0000-000000000c02"
const NAME_PROP = "00000000-0000-0000-0000-000000000d01"

/**
 * Create a minimal mock database that implements the query interface.
 */
function createMockDb() {
	return {
		execute: vi.fn(),
	}
}

/**
 * Create a minimal mock runtime for testing.
 */
function createMockRuntime() {
	return {
		runPromise: <A, E>(effect: Effect.Effect<A, E, never>) => Effect.runPromise(effect),
	}
}

/**
 * Set up test app with mock dependencies.
 */
function setupTestApp() {
	const db = createMockDb()
	const runtime = createMockRuntime()
	const router = createVersionedRouter(db as any, runtime as any)
	const app = new Hono()
	app.route("/versioned", router)
	return {app, db, runtime}
}

// =============================================================================
// GET /versioned/entities/:id Tests
// =============================================================================

describe("GET /versioned/entities/:id", () => {
	let app: Hono
	let db: ReturnType<typeof createMockDb>

	beforeEach(() => {
		const setup = setupTestApp()
		app = setup.app
		db = setup.db
	})

	describe("validation errors", () => {
		it("returns 400 when entityId is not a valid UUID", async () => {
			const res = await app.request("/versioned/entities/not-a-uuid?editId=00000000-0000-0000-0000-000000000001")

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("UUID")
		})

		it("returns 400 when editId is missing", async () => {
			const res = await app.request("/versioned/entities/00000000-0000-0000-0000-000000000001")

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("editId")
		})

		it("returns 400 when editId is not a valid UUID", async () => {
			const res = await app.request("/versioned/entities/00000000-0000-0000-0000-000000000001?editId=bad-id")

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("editId")
		})

		it("returns 400 when spaceId is provided but not a valid UUID", async () => {
			const res = await app.request(
				"/versioned/entities/00000000-0000-0000-0000-000000000001?editId=00000000-0000-0000-0000-000000000002&spaceId=invalid",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("spaceId")
		})
	})

	describe("not found errors", () => {
		it("returns 404 when edit is not found", async () => {
			// Mock: resolveVersionKey returns no rows
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(
				"/versioned/entities/00000000-0000-0000-0000-000000000001?editId=00000000-0000-0000-0000-000000000002",
			)

			expect(res.status).toBe(404)
			const body = await res.json()
			expect(body.error).toBe("Not found")
			expect(body.message).toContain("Edit")
		})
	})

	describe("successful responses", () => {
		it("returns entity snapshot when found", async () => {
			const entityId = "00000000-0000-0000-0000-000000000001"
			const editId = "00000000-0000-0000-0000-000000000002"

			// Mock: resolveVersionKey
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						version_key: "12345",
						name: "Test Edit",
						created_by_id: "aabbccdd-1122-3344-5566-778899001122",
					},
				],
			})

			// Mock: values query
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						entity_id: entityId,
						property_id: PROP_1,
						space_id: SPACE_1,
						text: "Test value",
					},
				],
			})

			// Mock: relations query
			db.execute.mockResolvedValueOnce({
				rows: [],
			})

			// Mock: blocks query (context edges)
			db.execute.mockResolvedValueOnce({
				rows: [],
			})

			// Mock: blocks query (relations fallback)
			db.execute.mockResolvedValueOnce({
				rows: [],
			})

			// Mock: getProfilesBySpaceIds (creator profile resolution)
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						entity_id: "11111111-2222-3333-4444-555555555555",
						space_id: "aabbccdd-1122-3344-5566-778899001122",
						space_address: "0x1234567890123456789012345678901234567890",
						entity_name: "Alice",
						avatar_url: "https://example.com/alice.png",
					},
				],
			})

			const res = await app.request(`/versioned/entities/${entityId}?editId=${editId}`)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.id).toBe(normalizeUuid(entityId))
			expect(body.editName).toBe("Test Edit")
			expect(body.createdById).toBe("aabbccdd112233445566778899001122")
			expect(body.createdBy).toEqual({
				entityId: "11111111222233334444555555555555",
				spaceId: "aabbccdd112233445566778899001122",
				name: "Alice",
				avatarUrl: "https://example.com/alice.png",
				address: "0x1234567890123456789012345678901234567890",
			})
			expect(body.values).toBeInstanceOf(Array)
			expect(body.relations).toBeInstanceOf(Array)
			expect(body.blocks).toBeInstanceOf(Array)
		})
	})

	describe("database errors", () => {
		it("returns 500 for database errors without leaking details", async () => {
			// Mock: database throws an error
			db.execute.mockRejectedValueOnce(new Error("Connection refused"))

			const res = await app.request(
				"/versioned/entities/00000000-0000-0000-0000-000000000001?editId=00000000-0000-0000-0000-000000000002",
			)

			expect(res.status).toBe(500)
			const body = await res.json()
			expect(body.error).toBe("Internal server error")
			expect(body.message).toBe("An unexpected error occurred")
			// Should NOT contain the actual error details
			expect(body.message).not.toContain("Connection refused")
		})
	})
})

// =============================================================================
// GET /versioned/entities/:id/versions Tests
// =============================================================================

describe("GET /versioned/entities/:id/versions", () => {
	let app: Hono
	let db: ReturnType<typeof createMockDb>

	beforeEach(() => {
		const setup = setupTestApp()
		app = setup.app
		db = setup.db
	})

	describe("validation errors", () => {
		it("returns 400 when entityId is not a valid UUID", async () => {
			const res = await app.request("/versioned/entities/bad-id/versions")

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
		})

		it("returns 400 when limit is invalid", async () => {
			const res = await app.request("/versioned/entities/00000000-0000-0000-0000-000000000001/versions?limit=-1")

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("limit")
		})

		it("returns 400 when offset is invalid", async () => {
			const res = await app.request("/versioned/entities/00000000-0000-0000-0000-000000000001/versions?offset=-5")

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("offset")
		})
	})

	describe("successful responses", () => {
		it("returns versions list with pagination", async () => {
			const entityId = "00000000-0000-0000-0000-000000000001"

			// Mock: getEntityVersions
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						edit_id: EDIT_1,
						block_number: "100",
						created_at: "2024-01-01T00:00:00Z",
						name: "First Edit",
						created_by_id: "aabbccdd-1122-3344-5566-778899001122",
					},
					{
						edit_id: EDIT_2,
						block_number: "200",
						created_at: "2024-01-02T00:00:00Z",
						name: null,
						created_by_id: null,
					},
				],
			})
			// Mock: getProfilesBySpaceIds (batch profile resolution)
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						entity_id: "11111111-2222-3333-4444-555555555555",
						space_id: "aabbccdd-1122-3344-5566-778899001122",
						space_address: "0x1234567890123456789012345678901234567890",
						entity_name: "Alice",
						avatar_url: "https://example.com/alice.png",
					},
				],
			})

			const res = await app.request(`/versioned/entities/${entityId}/versions`)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.versions).toBeInstanceOf(Array)
			expect(body.versions).toHaveLength(2)
			expect(body.versions[0]).toHaveProperty("editId")
			expect(body.versions[0]).toHaveProperty("blockNumber")
			expect(body.versions[0]).toHaveProperty("name")
			expect(body.versions[0].name).toBe("First Edit")
			expect(body.versions[0].createdById).toBe("aabbccdd112233445566778899001122")
			expect(body.versions[0].createdBy).toEqual({
				entityId: "11111111222233334444555555555555",
				spaceId: "aabbccdd112233445566778899001122",
				name: "Alice",
				avatarUrl: "https://example.com/alice.png",
				address: "0x1234567890123456789012345678901234567890",
			})
			expect(body.versions[1].name).toBeNull()
			expect(body.versions[1].createdById).toBeNull()
			expect(body.versions[1].createdBy).toBeNull()
		})

		it("respects limit parameter", async () => {
			const entityId = "00000000-0000-0000-0000-000000000001"

			db.execute.mockResolvedValueOnce({
				rows: [
					{
						edit_id: EDIT_1,
						block_number: "100",
						created_at: "2024-01-01T00:00:00Z",
						name: null,
						created_by_id: null,
					},
				],
			})

			const res = await app.request(`/versioned/entities/${entityId}/versions?limit=1`)

			expect(res.status).toBe(200)
			// Verify the query was called (limit is applied in SQL)
			expect(db.execute).toHaveBeenCalled()
		})
	})
})

// =============================================================================
// GET /versioned/entities/:id/diff Tests
// =============================================================================

describe("GET /versioned/entities/:id/diff", () => {
	let app: Hono
	let db: ReturnType<typeof createMockDb>

	beforeEach(() => {
		const setup = setupTestApp()
		app = setup.app
		db = setup.db
	})

	describe("validation errors", () => {
		it("returns 400 when fromEditId is missing", async () => {
			const res = await app.request(
				"/versioned/entities/00000000-0000-0000-0000-000000000001/diff?toEditId=00000000-0000-0000-0000-000000000002&spaceId=00000000-0000-0000-0000-000000000003",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("fromEditId")
		})

		it("returns 400 when toEditId is missing", async () => {
			const res = await app.request(
				"/versioned/entities/00000000-0000-0000-0000-000000000001/diff?fromEditId=00000000-0000-0000-0000-000000000002&spaceId=00000000-0000-0000-0000-000000000003",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("toEditId")
		})

		it("returns 400 when spaceId is missing", async () => {
			const res = await app.request(
				"/versioned/entities/00000000-0000-0000-0000-000000000001/diff?fromEditId=00000000-0000-0000-0000-000000000002&toEditId=00000000-0000-0000-0000-000000000003",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("spaceId")
		})
	})

	describe("not found errors", () => {
		it("returns 404 when fromEditId is not found", async () => {
			// Mock: first resolveVersionKey returns null
			db.execute.mockResolvedValueOnce({rows: []})
			// Mock: second resolveVersionKey returns a result
			db.execute.mockResolvedValueOnce({
				rows: [{version_key: "100", name: null, created_by_id: null}],
			})

			const res = await app.request(
				"/versioned/entities/00000000-0000-0000-0000-000000000001/diff?fromEditId=00000000-0000-0000-0000-000000000002&toEditId=00000000-0000-0000-0000-000000000003&spaceId=00000000-0000-0000-0000-000000000004",
			)

			expect(res.status).toBe(404)
			const body = await res.json()
			expect(body.message).toContain("Edit")
		})

		it("returns 404 when toEditId is not found", async () => {
			// Mock: first resolveVersionKey returns a result
			db.execute.mockResolvedValueOnce({
				rows: [{version_key: "100", name: null, created_by_id: null}],
			})
			// Mock: second resolveVersionKey returns null
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(
				"/versioned/entities/00000000-0000-0000-0000-000000000001/diff?fromEditId=00000000-0000-0000-0000-000000000002&toEditId=00000000-0000-0000-0000-000000000003&spaceId=00000000-0000-0000-0000-000000000004",
			)

			expect(res.status).toBe(404)
			const body = await res.json()
			expect(body.message).toContain("Edit")
		})
	})

	describe("successful responses", () => {
		it("returns entity diff", async () => {
			const entityId = "00000000-0000-0000-0000-000000000001"

			// Mock: resolveVersionKey (from)
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						version_key: "100",
						name: "From Edit",
						created_by_id: "aabbccdd-0000-0000-0000-000000000001",
					},
				],
			})
			// Mock: resolveVersionKey (to)
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						version_key: "200",
						name: "To Edit",
						created_by_id: "aabbccdd-0000-0000-0000-000000000002",
					},
				],
			})

			// Mock: getGroupedEntitySnapshotAtVersion (from) - values
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						entity_id: entityId,
						property_id: NAME_PROP,
						space_id: SPACE_1,
						text: "Old Name",
					},
				],
			})
			// Mock: from - relations
			db.execute.mockResolvedValueOnce({rows: []})
			// Mock: from - context edges
			db.execute.mockResolvedValueOnce({rows: []})
			// Mock: from - relations fallback
			db.execute.mockResolvedValueOnce({rows: []})

			// Mock: getGroupedEntitySnapshotAtVersion (to) - values
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						entity_id: entityId,
						property_id: NAME_PROP,
						space_id: SPACE_1,
						text: "New Name",
					},
				],
			})
			// Mock: to - relations
			db.execute.mockResolvedValueOnce({rows: []})
			// Mock: to - context edges
			db.execute.mockResolvedValueOnce({rows: []})
			// Mock: to - relations fallback
			db.execute.mockResolvedValueOnce({rows: []})

			// Mock: getProfilesBySpaceIds (creator profile resolution for both from/to)
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						entity_id: "aaaaaaaa-0000-0000-0000-000000000001",
						space_id: "aabbccdd-0000-0000-0000-000000000001",
						space_address: "0x1111111111111111111111111111111111111111",
						entity_name: "Author One",
						avatar_url: null,
					},
					{
						entity_id: "aaaaaaaa-0000-0000-0000-000000000002",
						space_id: "aabbccdd-0000-0000-0000-000000000002",
						space_address: "0x2222222222222222222222222222222222222222",
						entity_name: "Author Two",
						avatar_url: "https://example.com/two.png",
					},
				],
			})

			const res = await app.request(
				`/versioned/entities/${entityId}/diff?fromEditId=00000000-0000-0000-0000-000000000002&toEditId=00000000-0000-0000-0000-000000000003&spaceId=00000000-0000-0000-0000-000000000004`,
			)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.entityId).toBe(normalizeUuid(entityId))
			expect(body.fromEditName).toBe("From Edit")
			expect(body.fromCreatedById).toBe("aabbccdd000000000000000000000001")
			expect(body.fromCreatedBy).toEqual({
				entityId: "aaaaaaaa000000000000000000000001",
				spaceId: "aabbccdd000000000000000000000001",
				name: "Author One",
				avatarUrl: null,
				address: "0x1111111111111111111111111111111111111111",
			})
			expect(body.toEditName).toBe("To Edit")
			expect(body.toCreatedById).toBe("aabbccdd000000000000000000000002")
			expect(body.toCreatedBy).toEqual({
				entityId: "aaaaaaaa000000000000000000000002",
				spaceId: "aabbccdd000000000000000000000002",
				name: "Author Two",
				avatarUrl: "https://example.com/two.png",
				address: "0x2222222222222222222222222222222222222222",
			})
			expect(body.values).toBeInstanceOf(Array)
			expect(body.relations).toBeInstanceOf(Array)
			expect(body.blocks).toBeInstanceOf(Array)
		})
	})
})

// =============================================================================
// GET /versioned/proposals/:id/diff Tests
// =============================================================================

describe("GET /versioned/proposals/:id/diff", () => {
	let app: Hono
	let db: ReturnType<typeof createMockDb>

	beforeEach(() => {
		const setup = setupTestApp()
		app = setup.app
		db = setup.db
	})

	describe("validation errors", () => {
		it("returns 400 when proposalId is not a valid UUID", async () => {
			const res = await app.request(
				"/versioned/proposals/not-a-uuid/diff?spaceId=00000000-0000-0000-0000-000000000001",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("UUID")
		})

		it("returns 400 when spaceId is missing", async () => {
			const res = await app.request("/versioned/proposals/00000000-0000-0000-0000-000000000001/diff")

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("spaceId")
		})

		it("returns 400 when spaceId is not a valid UUID", async () => {
			const res = await app.request(
				"/versioned/proposals/00000000-0000-0000-0000-000000000001/diff?spaceId=invalid",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("spaceId")
		})

		it("returns 400 when limit is invalid", async () => {
			const res = await app.request(
				"/versioned/proposals/00000000-0000-0000-0000-000000000001/diff?spaceId=00000000-0000-0000-0000-000000000002&limit=-1",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("limit")
		})

		it("returns 400 when limit is not a number", async () => {
			const res = await app.request(
				"/versioned/proposals/00000000-0000-0000-0000-000000000001/diff?spaceId=00000000-0000-0000-0000-000000000002&limit=abc",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("limit")
		})
	})

	describe("not found errors", () => {
		it("returns 404 when proposal is not found", async () => {
			// Mock: getProposalWithPublishAction returns no rows
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(
				"/versioned/proposals/00000000-0000-0000-0000-000000000001/diff?spaceId=00000000-0000-0000-0000-000000000002",
			)

			expect(res.status).toBe(404)
			const body = await res.json()
			expect(body.error).toBe("Not found")
			expect(body.message).toContain("Proposal")
		})

		it("returns 404 when edit blob is not cached (without leaking URI)", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)

			// Mock: getProposalWithPublishAction returns proposal with content_uri
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(), // Active proposal
						executed_at: null,
						content_uri: "ipfs://QmTest123SensitiveUri",
					},
				],
			})

			// Mock: getIpfsCacheData returns no data
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}`)

			expect(res.status).toBe(404)
			const body = await res.json()
			expect(body.error).toBe("Not found")
			expect(body.message).toContain("Edit blob not cached")
			// Verify IPFS URI is NOT leaked in the error message
			expect(body.message).not.toContain("ipfs://")
			expect(body.message).not.toContain("QmTest123")
		})

		it("returns 422 when edit blob failed GRC-20 validation (is_errored=true)", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)

			// Mock: getProposalWithPublishAction returns proposal with content_uri
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: "ipfs://QmTestErroredBlob",
					},
				],
			})

			// Mock: getIpfsCacheData returns row with is_errored=true
			db.execute.mockResolvedValueOnce({rows: [{data: null, is_errored: true}]})

			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}`)

			expect(res.status).toBe(422)
			const body = await res.json()
			expect(body.error).toBe("Unprocessable")
			expect(body.message).toContain("GRC-20 validation")
			// Verify IPFS URI is NOT leaked
			expect(body.message).not.toContain("ipfs://")
			expect(body.message).not.toContain("QmTestErroredBlob")
		})

		it("returns 404 when ipfs_cache row exists but data is null and not errored", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)

			// Mock: getProposalWithPublishAction returns proposal with content_uri
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: "ipfs://QmTestNullData",
					},
				],
			})

			// Mock: getIpfsCacheData returns row with data=null, is_errored=false
			db.execute.mockResolvedValueOnce({rows: [{data: null, is_errored: false}]})

			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}`)

			expect(res.status).toBe(404)
			const body = await res.json()
			expect(body.error).toBe("Not found")
			expect(body.message).toContain("Edit blob not cached")
		})

		it("returns 400 when spaceId does not match proposal's space", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const proposalSpaceId = "00000000-0000-0000-0000-000000000002"
			const wrongSpaceId = "00000000-0000-0000-0000-000000000099" // Different from proposal's space
			const now = Math.floor(Date.now() / 1000)

			// Mock: getProposalWithPublishAction returns proposal with different space_id
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: proposalSpaceId, // Proposal belongs to this space
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: null,
					},
				],
			})

			// Request with wrong spaceId
			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${wrongSpaceId}`)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("spaceId does not match")
		})

		it("returns 400 when cursor is malformed", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)
			const invalidCursor = "not-a-valid-base64-cursor!!!"

			// Mock: getProposalWithPublishAction returns proposal with content_uri
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: "ipfs://QmTest123",
					},
				],
			})

			// Mock: getIpfsCacheData returns valid blob
			// Create a minimal valid edit blob that decodes to empty ops
			const mockEditBlob = Buffer.from('{"version":1,"ops":[]}')
			db.execute.mockResolvedValueOnce({rows: [{data: mockEditBlob}]})

			const res = await app.request(
				`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}&cursor=${invalidCursor}`,
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("Invalid pagination cursor")
		})
	})

	describe("successful responses", () => {
		it("returns empty diff when proposal has no publish action", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)

			// Mock: getProposalWithPublishAction returns proposal without content_uri
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: null, // No publish action
					},
				],
			})

			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}`)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposalId).toBe(normalizeUuid(proposalId))
			expect(body.spaceId).toBe(normalizeUuid(spaceId))
			expect(body.proposalStatus).toBe("active")
			expect(body.entities).toEqual([])
			expect(body.pagination).toEqual({
				cursor: null,
				hasMore: false,
				totalEntities: 0,
			})
		})

		it("returns proposal status as active when end_time is in future", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)

			// Mock: proposal with end_time in future
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(), // Future
						executed_at: null,
						content_uri: null,
					},
				],
			})

			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}`)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposalStatus).toBe("active")
		})

		it("returns proposal status as closed when end_time is in past", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)

			// Mock: proposal with end_time in past
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 2000).toString(),
						end_time: (now - 1000).toString(), // Past
						executed_at: null,
						content_uri: null,
					},
				],
			})

			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}`)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposalStatus).toBe("closed")
		})

		it("returns proposal status as executed when executed_at is set", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)

			// Mock: executed proposal
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 2000).toString(),
						end_time: (now - 1000).toString(),
						executed_at: (now - 500).toString(), // Executed
						content_uri: null,
					},
				],
			})

			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}`)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposalStatus).toBe("executed")
		})

		it("uses executedAt (not endTime) as base state for executed proposals", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)
			const executedAt = now - 500
			const endTime = now + 1000 // Fast path: executed before endTime

			// Build a real encoded edit with a createEntity op
			const grcEntityId = randomId()
			const grcPropertyId = randomId()
			const ops: Op[] = [
				{
					type: "createEntity",
					id: grcEntityId,
					values: [
						{
							property: grcPropertyId,
							value: {type: "text", value: "Hello"},
						},
					],
				},
			]
			const editBlob = Buffer.from(
				encodeEdit({
					id: randomId(),
					name: "test",
					ops,
					authors: [],
					createdAt: BigInt(now) * 1000n,
				}),
			)

			// Mock 1: getProposalWithPublishAction — executed proposal with content_uri
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 2000).toString(),
						end_time: endTime.toString(),
						executed_at: executedAt.toString(),
						content_uri: "ipfs://QmTestBlob",
					},
				],
			})

			// Mock 2: getIpfsCacheData
			db.execute.mockResolvedValueOnce({rows: [{data: editBlob}]})

			// Mock 3: resolveVersionKeyBeforeTimestamp(executedAt)
			// Returns a version key so we take the batchGetVersionedSnapshots path
			db.execute.mockResolvedValueOnce({rows: [{version_key: "100"}]})

			// Mock 4: batchGetVersionedSnapshots — values query (empty base = no pre-existing values)
			db.execute.mockResolvedValueOnce({rows: []})

			// Mock 5: batchGetVersionedSnapshots — relations query (empty base)
			db.execute.mockResolvedValueOnce({rows: []})

			// Mock 6: batchGetBlockRelationsForEntities — no block relations
			db.execute.mockResolvedValueOnce({rows: []})

			// Mock 7: batchGetEntityNames (enrichment)
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}`)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposalStatus).toBe("executed")
			// The diff should be non-empty because the proposal adds a value to an empty base
			expect(body.entities.length).toBe(1)
			expect(body.pagination.totalEntities).toBe(1)

			// Verify the version resolution used executedAt, not endTime.
			// Mock call 3 (index 2) is resolveVersionKeyBeforeTimestamp.
			const resolveCall = db.execute.mock.calls[2]
			expect(resolveCall).toBeDefined()
			const resolveQuery = resolveCall![0]
			// Drizzle sql tagged templates store params in queryChunks or similar.
			// Check that the bound parameter is executedAt, not endTime.
			const allParams = JSON.stringify(resolveCall)
			expect(allParams).toContain(executedAt.toString())
			expect(allParams).not.toContain(endTime.toString())
		})

		it("respects limit parameter", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)

			// Mock: proposal without content_uri (simplest path)
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: null,
					},
				],
			})

			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}&limit=10`)

			expect(res.status).toBe(200)
			// Verify the query was called (limit would be applied during processing)
			expect(db.execute).toHaveBeenCalled()
		})

		it("accepts cursor parameter for pagination", async () => {
			const proposalId = "00000000-0000-0000-0000-000000000001"
			const spaceId = "00000000-0000-0000-0000-000000000002"
			const now = Math.floor(Date.now() / 1000)
			// Base64 encoded cursor: {"entityIndex":10}
			const cursor = Buffer.from(JSON.stringify({entityIndex: 10})).toString("base64")

			// Mock: proposal without content_uri
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: proposalId,
						space_id: spaceId,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: null,
					},
				],
			})

			const res = await app.request(`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}&cursor=${cursor}`)

			// Should accept the cursor without error (even if no entities to paginate)
			expect(res.status).toBe(200)
		})
	})

	describe("database errors", () => {
		it("returns 500 for database errors without leaking details", async () => {
			// Mock: database throws an error
			db.execute.mockRejectedValueOnce(new Error("Connection refused"))

			const res = await app.request(
				"/versioned/proposals/00000000-0000-0000-0000-000000000001/diff?spaceId=00000000-0000-0000-0000-000000000002",
			)

			expect(res.status).toBe(500)
			const body = await res.json()
			expect(body.error).toBe("Internal server error")
			expect(body.message).toBe("An unexpected error occurred")
			expect(body.message).not.toContain("Connection refused")
		})
	})
})

// =============================================================================
// GET /versioned/proposal-groups/diff Tests
// =============================================================================

describe("GET /versioned/proposal-groups/diff", () => {
	let app: Hono
	let db: ReturnType<typeof createMockDb>

	beforeEach(() => {
		const setup = setupTestApp()
		app = setup.app
		db = setup.db
	})

	const SPACE_1 = "00000000-0000-0000-0000-000000000b01"
	const PROPOSAL_1 = "00000000-0000-0000-0000-000000000001"
	const PROPOSAL_2 = "00000000-0000-0000-0000-000000000002"

	describe("validation errors", () => {
		it("returns 400 when spaceId is missing", async () => {
			const res = await app.request(`/versioned/proposal-groups/diff?proposalIds=${PROPOSAL_1},${PROPOSAL_2}`)
			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("spaceId")
		})

		it("returns 400 when proposalIds is missing", async () => {
			const res = await app.request(`/versioned/proposal-groups/diff?spaceId=${SPACE_1}`)
			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("proposalIds")
		})

		it("returns 400 when fewer than 2 proposal IDs are provided", async () => {
			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=${SPACE_1}&proposalIds=${PROPOSAL_1}`,
			)
			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("at least 2")
		})

		it("returns 400 when proposalIds contains invalid UUID", async () => {
			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=${SPACE_1}&proposalIds=${PROPOSAL_1},not-a-uuid`,
			)
			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("Invalid UUID")
		})

		it("returns 400 when spaceId is not a valid UUID", async () => {
			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=bad&proposalIds=${PROPOSAL_1},${PROPOSAL_2}`,
			)
			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("spaceId")
		})

		it("returns 400 when limit is invalid", async () => {
			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=${SPACE_1}&proposalIds=${PROPOSAL_1},${PROPOSAL_2}&limit=-1`,
			)
			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("limit")
		})

		it("returns 400 for duplicate proposal IDs", async () => {
			// Mock: batch load returns both (since validation happens after load)
			const now = Math.floor(Date.now() / 1000)
			// No DB call needed — duplicates are caught before any DB query

			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=${SPACE_1}&proposalIds=${PROPOSAL_1},${PROPOSAL_1}`,
			)
			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("Duplicate")
		})
	})

	describe("proposal lookup errors", () => {
		it("returns 404 when one proposal is not found", async () => {
			// Mock: batch query returns only one of the two proposals
			const now = Math.floor(Date.now() / 1000)
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: PROPOSAL_1,
						space_id: SPACE_1,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: "ipfs://QmTest1",
					},
					// PROPOSAL_2 is missing
				],
			})

			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=${SPACE_1}&proposalIds=${PROPOSAL_1},${PROPOSAL_2}`,
			)
			expect(res.status).toBe(404)
			const body = await res.json()
			expect(body.error).toBe("Not found")
		})

		it("returns 422 when a proposal has no Publish action", async () => {
			const now = Math.floor(Date.now() / 1000)
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: PROPOSAL_1,
						space_id: SPACE_1,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: "ipfs://QmTest1",
					},
					{
						proposal_id: PROPOSAL_2,
						space_id: SPACE_1,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: null, // No Publish action
					},
				],
			})

			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=${SPACE_1}&proposalIds=${PROPOSAL_1},${PROPOSAL_2}`,
			)
			expect(res.status).toBe(422)
			const body = await res.json()
			expect(body.message).toContain("Publish action")
		})

		it("returns 400 for mixed active and historical proposals", async () => {
			const now = Math.floor(Date.now() / 1000)
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: PROPOSAL_1,
						space_id: SPACE_1,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(), // Active (future end)
						executed_at: null,
						content_uri: "ipfs://QmTest1",
					},
					{
						proposal_id: PROPOSAL_2,
						space_id: SPACE_1,
						start_time: (now - 2000).toString(),
						end_time: (now - 1000).toString(), // Closed (past end)
						executed_at: null,
						content_uri: "ipfs://QmTest2",
					},
				],
			})

			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=${SPACE_1}&proposalIds=${PROPOSAL_1},${PROPOSAL_2}`,
			)
			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("mix")
		})

		it("returns 400 when a proposal belongs to a different space", async () => {
			const now = Math.floor(Date.now() / 1000)
			const OTHER_SPACE = "00000000-0000-0000-0000-000000000099"
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: PROPOSAL_1,
						space_id: SPACE_1,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: "ipfs://QmTest1",
					},
					{
						proposal_id: PROPOSAL_2,
						space_id: OTHER_SPACE, // Wrong space
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: "ipfs://QmTest2",
					},
				],
			})

			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=${SPACE_1}&proposalIds=${PROPOSAL_1},${PROPOSAL_2}`,
			)
			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("do not belong")
		})
	})

	describe("successful responses", () => {
		it("returns grouped diff with correct mode and proposalIds for active proposals", async () => {
			const now = Math.floor(Date.now() / 1000)

			// Mock 1: batchGetProposalsWithPublishActions
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						proposal_id: PROPOSAL_1,
						space_id: SPACE_1,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: "ipfs://QmTest1",
					},
					{
						proposal_id: PROPOSAL_2,
						space_id: SPACE_1,
						start_time: (now - 1000).toString(),
						end_time: (now + 1000).toString(),
						executed_at: null,
						content_uri: "ipfs://QmTest2",
					},
				],
			})

			// Mock 2-3: getIpfsCacheData for each proposal (valid GRC-20 edits with no ops)
			const emptyEditBlob = Buffer.from(
				encodeEdit({
					id: randomId(),
					name: "empty",
					ops: [],
					authors: [],
					createdAt: BigInt(now) * 1000n,
				}),
			)
			db.execute.mockResolvedValueOnce({rows: [{data: emptyEditBlob, is_errored: false}]})
			db.execute.mockResolvedValueOnce({rows: [{data: emptyEditBlob, is_errored: false}]})

			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=${SPACE_1}&proposalIds=${PROPOSAL_1},${PROPOSAL_2}`,
			)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.mode).toBe("active")
			expect(body.proposalIds).toHaveLength(2)
			expect(body.spaceId).toBe(normalizeUuid(SPACE_1))
			expect(body.entities).toEqual([])
			expect(body.pagination).toEqual({
				cursor: null,
				hasMore: false,
				totalEntities: 0,
			})
		})
	})

	describe("database errors", () => {
		it("returns 500 for database errors without leaking details", async () => {
			db.execute.mockRejectedValueOnce(new Error("Connection refused"))

			const res = await app.request(
				`/versioned/proposal-groups/diff?spaceId=${SPACE_1}&proposalIds=${PROPOSAL_1},${PROPOSAL_2}`,
			)

			expect(res.status).toBe(500)
			const body = await res.json()
			expect(body.error).toBe("Internal server error")
			expect(body.message).not.toContain("Connection refused")
		})
	})
})
