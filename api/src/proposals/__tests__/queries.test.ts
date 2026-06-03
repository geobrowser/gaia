/**
 * Tests for proposal queries including status filtering and ordering.
 *
 * These tests verify:
 * 1. Unit tests for helper functions (cursor parsing, validation)
 * 2. Integration tests for list endpoint with mock database
 * 3. Parity tests ensuring SQL status logic matches TypeScript computeProposalStatus
 */

import {Effect} from "effect"
import {Hono} from "hono"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {createProposalsRouter} from "../router"
import {computeProposalStatus} from "../status"
import type {ProposalWithVotes} from "../types"

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
	// biome-ignore lint/suspicious/noExplicitAny: test mock
	const router = createProposalsRouter(db as any, runtime as any)
	const app = new Hono()
	app.route("/proposals", router)
	return {app, db, runtime}
}

/**
 * Create a test proposal row as returned from the database.
 */
function makeDbProposalRow(overrides: Partial<Record<string, unknown>> = {}) {
	const now = Math.floor(Date.now() / 1000)
	return {
		id: "550e8400-e29b-41d4-a716-446655440000",
		space_id: "660e8400-e29b-41d4-a716-446655440000",
		name: "Test Proposal",
		proposed_by: "770e8400-e29b-41d4-a716-446655440000",
		voting_mode: "Fast",
		start_time: String(now - 3600),
		end_time: String(now + 3600),
		quorum: "10",
		threshold: "10",
		executed_at: null,
		created_at: new Date().toISOString(),
		yes_count: "0",
		no_count: "0",
		abstain_count: "0",
		actions_json: [],
		...overrides,
	}
}

/**
 * Create a test proposal for computeProposalStatus.
 */
function makeProposal(overrides: Partial<ProposalWithVotes> = {}): ProposalWithVotes {
	const now = BigInt(Math.floor(Date.now() / 1000))
	return {
		id: "test-proposal-id",
		spaceId: "test-space-id",
		name: "Test Proposal",
		proposedBy: "test-proposer-id",
		votingMode: "Fast",
		startTime: now - 3600n,
		endTime: now + 3600n,
		quorum: 10n,
		threshold: 10n,
		executedAt: null,
		yesCount: 0n,
		noCount: 0n,
		abstainCount: 0n,
		votes: [],
		actions: [],
		...overrides,
	}
}

// =============================================================================
// Router Validation Tests
// =============================================================================

describe("GET /proposals/space/:spaceId/status", () => {
	let app: Hono
	let db: ReturnType<typeof createMockDb>

	beforeEach(() => {
		const setup = setupTestApp()
		app = setup.app
		db = setup.db
	})

	describe("parameter validation", () => {
		it("returns 400 when spaceId is not a valid UUID", async () => {
			const res = await app.request("/proposals/space/not-a-uuid/status")

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.error).toBe("Invalid parameter")
			expect(body.message).toContain("UUID")
		})

		it("returns 400 when limit is not a number", async () => {
			const res = await app.request("/proposals/space/550e8400-e29b-41d4-a716-446655440000/status?limit=abc")

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("Limit")
		})

		it("returns 400 when limit is out of range", async () => {
			const res = await app.request("/proposals/space/550e8400-e29b-41d4-a716-446655440000/status?limit=101")

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("Limit")
		})

		it("returns 400 for invalid status values", async () => {
			const res = await app.request(
				"/proposals/space/550e8400-e29b-41d4-a716-446655440000/status?status=INVALID,BADSTATUS",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("Invalid statuses")
			expect(body.message).toContain("INVALID")
			expect(body.message).toContain("BADSTATUS")
			expect(body.message).toContain("Valid:")
		})

		it("returns 400 for invalid orderBy value", async () => {
			const res = await app.request(
				"/proposals/space/550e8400-e29b-41d4-a716-446655440000/status?orderBy=invalid_column",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("Invalid orderBy")
			expect(body.message).toContain("created_at")
		})

		it("returns 400 for invalid orderDirection value", async () => {
			const res = await app.request(
				"/proposals/space/550e8400-e29b-41d4-a716-446655440000/status?orderDirection=sideways",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("Invalid orderDirection")
			expect(body.message).toContain("asc")
			expect(body.message).toContain("desc")
		})

		it("accepts valid status values (case insensitive)", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(
				"/proposals/space/550e8400-e29b-41d4-a716-446655440000/status?status=proposed,EXECUTABLE,Accepted",
			)

			expect(res.status).toBe(200)
		})

		it("accepts valid orderBy values", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(
				"/proposals/space/550e8400-e29b-41d4-a716-446655440000/status?orderBy=end_time",
			)

			expect(res.status).toBe(200)
		})

		it("accepts valid orderDirection values", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(
				"/proposals/space/550e8400-e29b-41d4-a716-446655440000/status?orderDirection=asc",
			)

			expect(res.status).toBe(200)
		})
	})

	describe("successful responses", () => {
		it("returns proposals list with default ordering", async () => {
			const row = makeDbProposalRow()
			db.execute.mockResolvedValueOnce({rows: [row]})

			const res = await app.request("/proposals/space/660e8400-e29b-41d4-a716-446655440000/status")

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposals).toBeInstanceOf(Array)
			expect(body.proposals).toHaveLength(1)
			expect(body.nextCursor).toBeNull()
		})

		it("maps SET_TOPIC and UNSET_TOPIC actions in list responses", async () => {
			const row = makeDbProposalRow({
				actions_json: [
					{
						action_type: "SetTopic",
						target_id: "880e8400-e29b-41d4-a716-446655440000",
						content_uri: null,
						content_id: null,
						quorum: null,
						fast_threshold: null,
						slow_threshold: null,
						duration: null,
					},
					{
						action_type: "UnsetTopic",
						target_id: "990e8400-e29b-41d4-a716-446655440000",
						content_uri: null,
						content_id: null,
						quorum: null,
						fast_threshold: null,
						slow_threshold: null,
						duration: null,
					},
				],
			})
			db.execute.mockResolvedValueOnce({rows: [row]})

			const res = await app.request("/proposals/space/660e8400-e29b-41d4-a716-446655440000/status")

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposals[0].actions).toEqual([
				{
					actionType: "SET_TOPIC",
					targetTopicId: "880e8400-e29b-41d4-a716-446655440000",
				},
				{
					actionType: "UNSET_TOPIC",
					targetTopicId: "990e8400-e29b-41d4-a716-446655440000",
				},
			])
		})

		it("maps UNSET_TOPIC with null target_id to UNKNOWN", async () => {
			const row = makeDbProposalRow({
				actions_json: [
					{
						action_type: "UnsetTopic",
						target_id: null,
						content_uri: null,
						content_id: null,
						quorum: null,
						fast_threshold: null,
						slow_threshold: null,
						duration: null,
					},
				],
			})
			db.execute.mockResolvedValueOnce({rows: [row]})

			const res = await app.request("/proposals/space/660e8400-e29b-41d4-a716-446655440000/status")

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposals[0].actions).toEqual([
				{
					actionType: "UNKNOWN",
				},
			])
		})

		it("returns nextCursor when there are more results", async () => {
			// Return 21 rows (limit + 1) to indicate there are more
			const rows = Array.from({length: 21}, (_, i) =>
				makeDbProposalRow({
					id: `550e8400-e29b-41d4-a716-44665544${String(i).padStart(4, "0")}`,
					created_at: new Date(Date.now() - i * 1000).toISOString(),
				}),
			)
			db.execute.mockResolvedValueOnce({rows})

			const res = await app.request("/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?limit=20")

			expect(res.status).toBe(200)
			const body = await res.json()
			expect(body.proposals).toHaveLength(20)
			expect(body.nextCursor).not.toBeNull()
			// Cursor should contain timestamp and ID
			expect(body.nextCursor).toContain("|")
		})

		it("passes status filter to query", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			await app.request("/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?status=PROPOSED,EXECUTABLE")

			expect(db.execute).toHaveBeenCalled()
			// Verify the SQL was called (we can't easily inspect the SQL content with mocks)
		})

		it("passes orderBy and orderDirection to query", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			await app.request(
				"/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?orderBy=end_time&orderDirection=asc",
			)

			expect(db.execute).toHaveBeenCalled()
		})
	})

	describe("cursor pagination", () => {
		it("accepts valid cursor format for created_at ordering", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			const cursor = "2024-01-15T10:30:00.000Z|550e8400-e29b-41d4-a716-446655440000"
			const res = await app.request(
				`/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?cursor=${encodeURIComponent(cursor)}`,
			)

			expect(res.status).toBe(200)
		})

		it("accepts valid cursor format for end_time ordering", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			const cursor = "1706789400|550e8400-e29b-41d4-a716-446655440000"
			const res = await app.request(
				`/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?cursor=${encodeURIComponent(cursor)}&orderBy=end_time`,
			)

			expect(res.status).toBe(200)
		})

		it("ignores malformed cursor and returns results from beginning", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			// Malformed cursor (no pipe separator)
			const res = await app.request(
				"/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?cursor=invalid-cursor",
			)

			// Should succeed but start from beginning
			expect(res.status).toBe(200)
		})

		it("ignores cursor with invalid UUID", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			const cursor = "2024-01-15T10:30:00.000Z|not-a-uuid"
			const res = await app.request(
				`/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?cursor=${encodeURIComponent(cursor)}`,
			)

			// Should succeed but start from beginning
			expect(res.status).toBe(200)
		})
	})

	describe("database errors", () => {
		it("returns 500 for database errors without leaking details", async () => {
			db.execute.mockRejectedValueOnce(new Error("Connection refused"))

			const res = await app.request("/proposals/space/660e8400-e29b-41d4-a716-446655440000/status")

			expect(res.status).toBe(500)
			const body = await res.json()
			expect(body.error).toBe("Internal server error")
			expect(body.message).toBe("An unexpected error occurred")
			expect(body.message).not.toContain("Connection refused")
		})
	})

	describe("combined filters", () => {
		it("accepts status filter with orderBy and orderDirection", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(
				"/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?status=PROPOSED,EXECUTABLE&orderBy=end_time&orderDirection=asc",
			)

			expect(res.status).toBe(200)
			expect(db.execute).toHaveBeenCalled()
		})

		it("accepts actionTypes filter with status filter", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(
				"/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?actionTypes=Publish,AddMember&status=EXECUTABLE",
			)

			expect(res.status).toBe(200)
			expect(db.execute).toHaveBeenCalled()
		})

		it("accepts SET_TOPIC and UNSET_TOPIC action type filters", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(
				"/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?actionTypes=SetTopic,UnsetTopic",
			)

			expect(res.status).toBe(200)
			expect(db.execute).toHaveBeenCalled()
		})

		it("accepts all filters combined", async () => {
			db.execute.mockResolvedValueOnce({rows: []})

			const res = await app.request(
				"/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?status=PROPOSED&actionTypes=Publish&orderBy=start_time&orderDirection=desc&limit=50",
			)

			expect(res.status).toBe(200)
			expect(db.execute).toHaveBeenCalled()
		})

		it("returns 400 when both actionTypes and excludeActionTypes are provided", async () => {
			const res = await app.request(
				"/proposals/space/660e8400-e29b-41d4-a716-446655440000/status?actionTypes=Publish&excludeActionTypes=AddMember",
			)

			expect(res.status).toBe(400)
			const body = await res.json()
			expect(body.message).toContain("Cannot specify both")
		})
	})
})

// =============================================================================
// Status Parity Tests
// =============================================================================

describe("SQL/TypeScript status parity", () => {
	/**
	 * These test cases document the expected status for various proposal states.
	 * The SQL implementation should match the TypeScript computeProposalStatus function.
	 *
	 * NOTE: These tests verify the TypeScript logic. Integration tests with a real
	 * database would be needed to verify the SQL implementation matches.
	 */

	const now = BigInt(Math.floor(Date.now() / 1000))

	describe("ACCEPTED status", () => {
		it("proposal is ACCEPTED when executedAt is set", () => {
			const proposal = makeProposal({executedAt: now - 100n})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("ACCEPTED")
		})
	})

	describe("EXECUTABLE status - Fast path", () => {
		it("proposal is EXECUTABLE when yes votes >= threshold (Fast)", () => {
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 10n,
				yesCount: 10n, // meets threshold
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("EXECUTABLE")
		})

		it("proposal is EXECUTABLE when yes votes > threshold (Fast)", () => {
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 10n,
				yesCount: 15n, // exceeds threshold
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("EXECUTABLE")
		})
	})

	describe("EXECUTABLE status - Slow path", () => {
		it("proposal is EXECUTABLE when voting ended, quorum met, and threshold met (Slow)", () => {
			const proposal = makeProposal({
				votingMode: "Slow",
				threshold: 5_000_000n, // 50%
				quorum: 100n,
				yesCount: 60n,
				noCount: 30n,
				abstainCount: 20n, // total 110 >= 100 quorum
				endTime: now - 100n, // voting ended
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("EXECUTABLE")
			expect(result.isQuorumReached).toBe(true)
			expect(result.isThresholdReached).toBe(true)
		})
	})

	describe("REJECTED status - Fast path", () => {
		it("proposal is REJECTED when voting ended and threshold not met (Fast)", () => {
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 10n,
				yesCount: 5n, // below threshold
				endTime: now - 100n, // voting ended
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("REJECTED")
		})
	})

	describe("REJECTED status - Slow path", () => {
		it("proposal is REJECTED when voting ended and quorum not met (Slow)", () => {
			const proposal = makeProposal({
				votingMode: "Slow",
				threshold: 5_000_000n,
				quorum: 100n,
				yesCount: 30n,
				noCount: 10n,
				abstainCount: 5n, // total 45 < 100 quorum
				endTime: now - 100n, // voting ended
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("REJECTED")
			expect(result.isQuorumReached).toBe(false)
		})

		it("proposal is REJECTED when voting ended, quorum met, but threshold not met (Slow)", () => {
			const proposal = makeProposal({
				votingMode: "Slow",
				threshold: 5_000_000n, // 50%
				quorum: 100n,
				yesCount: 40n,
				noCount: 50n, // no > yes
				abstainCount: 20n, // total 110 >= 100 quorum
				endTime: now - 100n, // voting ended
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("REJECTED")
			expect(result.isQuorumReached).toBe(true)
			expect(result.isThresholdReached).toBe(false)
		})

		it("exact tie results in REJECTED (Slow)", () => {
			const proposal = makeProposal({
				votingMode: "Slow",
				threshold: 5_000_000n, // 50%
				quorum: 10n,
				yesCount: 50n,
				noCount: 50n, // exact tie
				abstainCount: 10n,
				endTime: now - 100n, // voting ended
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("REJECTED")
		})
	})

	describe("PROPOSED status", () => {
		it("proposal is PROPOSED when voting not ended and threshold not met (Fast)", () => {
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 10n,
				yesCount: 5n, // below threshold
				endTime: now + 3600n, // voting not ended
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("PROPOSED")
		})

		it("proposal is PROPOSED when voting not ended (Slow)", () => {
			const proposal = makeProposal({
				votingMode: "Slow",
				threshold: 5_000_000n,
				quorum: 100n,
				yesCount: 100n, // would meet threshold
				noCount: 10n,
				abstainCount: 10n,
				endTime: now + 3600n, // voting not ended
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("PROPOSED")
		})
	})

	describe("edge cases", () => {
		it("handles zero threshold (Fast path executable with any yes vote)", () => {
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 0n,
				yesCount: 1n,
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("EXECUTABLE")
		})

		it("handles zero threshold with zero yes votes (Fast path NOT executable)", () => {
			// This is the edge case: threshold=0 means we need yes > -1, which means yes >= 0
			// But the contract logic uses yes > (threshold - 1), with threshold - 1 = 0 when threshold = 0
			// So we need yes > 0, meaning at least 1 yes vote
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 0n,
				yesCount: 0n,
				endTime: now + 3600n, // voting not ended
			})
			const result = computeProposalStatus(proposal, now)
			// With threshold=0 and yesCount=0, threshold formula is yes > max(threshold-1, 0) = yes > 0
			// 0 > 0 is false, so NOT executable yet
			expect(result.status).toBe("PROPOSED")
		})

		it("handles zero threshold with zero yes votes after voting ends (REJECTED)", () => {
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 0n,
				yesCount: 0n,
				endTime: now - 100n, // voting ended
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("REJECTED")
		})

		it("handles threshold=1 exactly met", () => {
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 1n,
				yesCount: 1n,
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("EXECUTABLE")
		})

		it("handles threshold=1 not met", () => {
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 1n,
				yesCount: 0n,
				endTime: now + 3600n,
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("PROPOSED")
		})

		it("handles zero votes", () => {
			const proposal = makeProposal({
				votingMode: "Slow",
				threshold: 5_000_000n,
				quorum: 1n,
				yesCount: 0n,
				noCount: 0n,
				abstainCount: 0n,
				endTime: now - 100n,
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("REJECTED")
			expect(result.isQuorumReached).toBe(false)
		})

		it("boundary: exactly at threshold (Fast path)", () => {
			// yes >= threshold should be executable
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 5n,
				yesCount: 5n,
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("EXECUTABLE")
		})

		it("boundary: one below threshold (Fast path)", () => {
			const proposal = makeProposal({
				votingMode: "Fast",
				threshold: 5n,
				yesCount: 4n,
				endTime: now + 3600n,
			})
			const result = computeProposalStatus(proposal, now)
			expect(result.status).toBe("PROPOSED")
		})
	})
})

// =============================================================================
// Active Proposal Check Endpoint Tests
// =============================================================================

const SPACE_ID = "660e8400-e29b-41d4-a716-446655440000"
const TARGET_SPACE_ID = "770e8400-e29b-41d4-a716-446655440000"

/**
 * Generates the full test suite for an active proposal check endpoint.
 * Both the member and editor endpoints share identical behavior — only
 * the URL segment and target parameter label differ.
 */
function describeActiveProposalEndpoint(config: {
	label: string
	/** URL segment between spaceId and targetId, e.g. "members" */
	segment: string
	/** Human-readable target label expected in validation errors */
	targetLabel: string
}) {
	const buildUrl = (spaceId: string, targetId: string) =>
		`/proposals/space/${spaceId}/${config.segment}/${targetId}/active`

	describe(`GET /proposals/space/:spaceId/${config.segment}/:targetId/active`, () => {
		let app: Hono
		let db: ReturnType<typeof createMockDb>

		beforeEach(() => {
			const setup = setupTestApp()
			app = setup.app
			db = setup.db
		})

		describe("parameter validation", () => {
			it("returns 400 when spaceId is not a valid UUID", async () => {
				const res = await app.request(buildUrl("not-a-uuid", TARGET_SPACE_ID))

				expect(res.status).toBe(400)
				const body = await res.json()
				expect(body.error).toBe("Invalid parameter")
				expect(body.message).toContain("Space ID")
				expect(body.message).toContain("UUID")
			})

			it(`returns 400 when ${config.label} target ID is not a valid UUID`, async () => {
				const res = await app.request(buildUrl(SPACE_ID, "not-a-uuid"))

				expect(res.status).toBe(400)
				const body = await res.json()
				expect(body.error).toBe("Invalid parameter")
				expect(body.message).toContain(config.targetLabel)
				expect(body.message).toContain("UUID")
			})
		})

		describe("successful responses", () => {
			it("returns { active: true } when an active proposal exists", async () => {
				db.execute.mockResolvedValueOnce({rows: [{exists: true}]})

				const res = await app.request(buildUrl(SPACE_ID, TARGET_SPACE_ID))

				expect(res.status).toBe(200)
				const body = await res.json()
				expect(body).toEqual({active: true})
			})

			it("returns { active: false } when no active proposal exists", async () => {
				db.execute.mockResolvedValueOnce({rows: [{exists: false}]})

				const res = await app.request(buildUrl(SPACE_ID, TARGET_SPACE_ID))

				expect(res.status).toBe(200)
				const body = await res.json()
				expect(body).toEqual({active: false})
			})

			it("returns 500 when SELECT EXISTS returns zero rows (contract violation)", async () => {
				db.execute.mockResolvedValueOnce({rows: []})

				const res = await app.request(buildUrl(SPACE_ID, TARGET_SPACE_ID))

				expect(res.status).toBe(500)
				const body = await res.json()
				expect(body.error).toBe("Internal server error")
				expect(body.message).toBe("An unexpected error occurred")
			})
		})

		describe("database errors", () => {
			it("returns 500 for database errors without leaking details", async () => {
				db.execute.mockRejectedValueOnce(new Error("Connection refused"))

				const res = await app.request(buildUrl(SPACE_ID, TARGET_SPACE_ID))

				expect(res.status).toBe(500)
				const body = await res.json()
				expect(body.error).toBe("Internal server error")
				expect(body.message).toBe("An unexpected error occurred")
				expect(body.message).not.toContain("Connection refused")
			})
		})
	})
}

describeActiveProposalEndpoint({
	label: "member",
	segment: "members",
	targetLabel: "Member space ID",
})

describeActiveProposalEndpoint({
	label: "editor",
	segment: "editors",
	targetLabel: "Editor space ID",
})
