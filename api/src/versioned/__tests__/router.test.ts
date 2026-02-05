import {Effect} from "effect"
import {Hono} from "hono"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {createVersionedRouter} from "../router"

// =============================================================================
// Test Setup
// =============================================================================

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
				rows: [{version_key: "12345"}],
			})

			// Mock: values query
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						entity_id: entityId,
						property_id: "prop-1",
						space_id: "space-1",
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

			const res = await app.request(`/versioned/entities/${entityId}?editId=${editId}`)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.id).toBe(entityId)
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
						edit_id: "edit-1",
						block_number: "100",
						created_at: "2024-01-01T00:00:00Z",
					},
					{
						edit_id: "edit-2",
						block_number: "200",
						created_at: "2024-01-02T00:00:00Z",
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
		})

		it("respects limit parameter", async () => {
			const entityId = "00000000-0000-0000-0000-000000000001"

			db.execute.mockResolvedValueOnce({
				rows: [
					{
						edit_id: "edit-1",
						block_number: "100",
						created_at: "2024-01-01T00:00:00Z",
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
			db.execute.mockResolvedValueOnce({rows: [{version_key: "100"}]})

			const res = await app.request(
				"/versioned/entities/00000000-0000-0000-0000-000000000001/diff?fromEditId=00000000-0000-0000-0000-000000000002&toEditId=00000000-0000-0000-0000-000000000003&spaceId=00000000-0000-0000-0000-000000000004",
			)

			expect(res.status).toBe(404)
			const body = await res.json()
			expect(body.message).toContain("Edit")
		})

		it("returns 404 when toEditId is not found", async () => {
			// Mock: first resolveVersionKey returns a result
			db.execute.mockResolvedValueOnce({rows: [{version_key: "100"}]})
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
			db.execute.mockResolvedValueOnce({rows: [{version_key: "100"}]})
			// Mock: resolveVersionKey (to)
			db.execute.mockResolvedValueOnce({rows: [{version_key: "200"}]})

			// Mock: getEntitySnapshotAtVersion (from) - values
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						entity_id: entityId,
						property_id: "name-prop",
						space_id: "space-1",
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

			// Mock: getEntitySnapshotAtVersion (to) - values
			db.execute.mockResolvedValueOnce({
				rows: [
					{
						entity_id: entityId,
						property_id: "name-prop",
						space_id: "space-1",
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

			const res = await app.request(
				`/versioned/entities/${entityId}/diff?fromEditId=00000000-0000-0000-0000-000000000002&toEditId=00000000-0000-0000-0000-000000000003&spaceId=00000000-0000-0000-0000-000000000004`,
			)

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.entityId).toBe(entityId)
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
			expect(body.proposalId).toBe(proposalId)
			expect(body.spaceId).toBe(spaceId)
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

			const res = await app.request(
				`/versioned/proposals/${proposalId}/diff?spaceId=${spaceId}&cursor=${cursor}`,
			)

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
