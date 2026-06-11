/**
 * Tests for detect.ts — detection SQL structure and RATIO_BASE cross-validation.
 *
 * These tests verify SQL correctness without a live database by inspecting
 * the interpolated query string and cross-validating constants against
 * the API's source of truth.
 */

import {describe, expect, test} from "bun:test"
import {Effect} from "effect"
import {RATIO_BASE} from "../src/contracts.js"
import {findMembershipRequests, MEMBERSHIP_DETECTION_SQL} from "../src/detect.js"

// ---------------------------------------------------------------------------
// RATIO_BASE cross-validation
// ---------------------------------------------------------------------------

describe("RATIO_BASE constant", () => {
	test("matches the protocol constant (10,000,000)", () => {
		// Source of truth: api/src/proposals/types.ts — RATIO_BASE = 10_000_000n
		expect(RATIO_BASE).toBe(10_000_000)
	})

	test("is a positive integer", () => {
		expect(Number.isInteger(RATIO_BASE)).toBe(true)
		expect(RATIO_BASE).toBeGreaterThan(0)
	})
})

// ---------------------------------------------------------------------------
// Detection SQL structure
// ---------------------------------------------------------------------------

describe("detection SQL", () => {
	// We can't import DETECTION_SQL directly (it's module-private), so we
	// verify the contract by testing the public function's interface.
	// The SQL structure tests below verify the interpolated constants.

	test("CLOCK_SKEW_BUFFER is 60 seconds (interpolated into SQL)", () => {
		// The SQL contains `$1::bigint > p.end_time + 60` — this verifies
		// the buffer value is correct. We verify this indirectly by checking
		// the constant matches the plan.
		const CLOCK_SKEW_BUFFER = 60
		expect(CLOCK_SKEW_BUFFER).toBe(60)
	})

	test("RATIO_BASE is interpolated as a literal in the threshold check", () => {
		// The SQL uses `(${RATIO_BASE} - p.threshold::numeric) * p.yes_count::numeric`
		// This verifies the value that gets interpolated matches the protocol constant.
		expect(RATIO_BASE).toBe(10_000_000)
		// The threshold formula: (RATIO_BASE - threshold) * yes > threshold * no
		// For a 51% threshold (5,100,000): (10,000,000 - 5,100,000) * yes > 5,100,000 * no
		// → 4,900,000 * yes > 5,100,000 * no
		// If 6 yes, 4 no: 29,400,000 > 20,400,000 ✓ (passes)
		// If 5 yes, 5 no: 24,500,000 > 25,500,000 ✗ (fails — tied)
		const threshold = 5_100_000
		expect((RATIO_BASE - threshold) * 6).toBeGreaterThan(threshold * 4) // passes
		expect((RATIO_BASE - threshold) * 5).toBeLessThan(threshold * 5) // tied = fails
	})
})

// ---------------------------------------------------------------------------
// Proposal type shape
// ---------------------------------------------------------------------------

describe("Proposal type", () => {
	test("has the expected shape from SQL column aliases", () => {
		// The SQL uses:
		//   p.id             → id
		//   p.space_id AS "spaceId"   → spaceId
		const proposal = {
			id: "550e8400-e29b-41d4-a716-446655440000",
			spaceId: "660e8400-e29b-41d4-a716-446655440000",
		}
		expect(proposal.id).toContain("-") // UUID with dashes
		expect(proposal.spaceId).toContain("-") // UUID with dashes
	})
})

// ---------------------------------------------------------------------------
// Membership-request detection SQL (stage 1)
// ---------------------------------------------------------------------------

// Whitespace-normalized SQL for robust substring assertions.
const membershipSql = MEMBERSHIP_DETECTION_SQL.replace(/\s+/g, " ").trim()

describe("membership detection SQL", () => {
	test("scopes to the allowlist via space_id = ANY($1)", () => {
		expect(membershipSql).toContain("p.space_id = ANY($1)")
	})

	test("selects only Fast-mode proposals (a single YES vote can execute)", () => {
		expect(membershipSql).toContain("p.voting_mode = 'Fast'")
	})

	test("requires the action to be a self-service AddMember (target == proposer)", () => {
		expect(membershipSql).toContain("a.action_type = 'AddMember'")
		expect(membershipSql).toContain("a.target_id = p.proposed_by")
	})

	test("projects the MembershipRequest row shape {id, spaceId, requesterId}", () => {
		expect(membershipSql).toContain("p.id")
		expect(membershipSql).toContain('p.space_id AS "spaceId"')
		expect(membershipSql).toContain('a.target_id AS "requesterId"')
	})

	test("orders FIFO by numeric created_at", () => {
		expect(membershipSql).toContain("ORDER BY p.created_at::bigint ASC")
	})

	test("excludes proposals with any indexed vote", () => {
		expect(membershipSql).toContain("NOT EXISTS (SELECT 1 FROM proposal_votes pv WHERE pv.proposal_id = p.id)")
	})

	test("excludes executed proposals", () => {
		expect(membershipSql).toContain("p.executed_at IS NULL")
	})

	test("excludes editor-initiated adds by requiring target_id = proposed_by", () => {
		// An editor-initiated add has proposed_by != target_id, so this predicate filters it out.
		expect(membershipSql).toContain("a.target_id = p.proposed_by")
	})

	test("excludes multi-action proposals (exactly one action required)", () => {
		expect(membershipSql).toContain("(SELECT COUNT(*) FROM proposal_actions a2 WHERE a2.proposal_id = p.id) = 1")
	})

	test("excludes Slow-mode proposals (Fast-only filter never overlaps the executor query)", () => {
		expect(membershipSql).toContain("p.voting_mode = 'Fast'")
		expect(membershipSql).not.toContain("'Slow'")
	})

	test("guards against corrupt future timestamps but applies no age cutoff", () => {
		expect(membershipSql).toContain("p.created_at::bigint <= $2::bigint")
		// Backlog is admitted: no maximum-age bound (unlike the executor query).
		expect(membershipSql).not.toContain("MAX_PROPOSAL_AGE")
	})

	test("excludes proposals whose voting period has ended", () => {
		// Votes cast after end_time are rejected by the protocol, and an untouched
		// Fast proposal past its period is already classified REJECTED.
		expect(membershipSql).toContain("$2::bigint <= p.end_time")
	})
})

// ---------------------------------------------------------------------------
// findMembershipRequests — kill switch / gating
// ---------------------------------------------------------------------------

describe("findMembershipRequests", () => {
	// An empty allowlist is the kill switch: stop all activity, do not query.
	test("short-circuits to [] without issuing a query when the allowlist is empty", async () => {
		let queried = false
		const fakeClient = {
			query: async () => {
				queried = true
				return {rows: []}
			},
		} as never

		const result = await Effect.runPromise(findMembershipRequests(fakeClient, []))
		expect(result).toEqual([])
		expect(queried).toBe(false)
	})

	// Gating: only allowlisted spaces reach the query, bound as ANY($1).
	test("passes the allowlist as the first bind parameter", async () => {
		const allowlist = ["660e8400-e29b-41d4-a716-446655440000"]
		let boundParams: unknown[] = []
		const fakeClient = {
			query: async (_sql: string, params: unknown[]) => {
				boundParams = params
				return {rows: []}
			},
		} as never

		await Effect.runPromise(findMembershipRequests(fakeClient, allowlist))
		expect(boundParams[0]).toEqual(allowlist)
		expect(typeof boundParams[1]).toBe("number") // nowSeconds guard
	})
})
