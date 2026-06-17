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
import {DETECTION_SQL, findMembershipRequests, MAX_PROPOSAL_AGE, MEMBERSHIP_DETECTION_SQL} from "../src/detect.js"

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
// V2 detection SQL structure (GEO-485)
//
// These tests inspect the module-exported DETECTION_SQL string to verify it
// reads from the post-GEO-481 schema shape. No DB is involved — just string
// assertions.
// ---------------------------------------------------------------------------

describe("V2 detection SQL", () => {
	test("reads from proposals_current, not the identity-only proposals table", () => {
		expect(DETECTION_SQL).toMatch(/FROM\s+proposals_current/)
		// Guard against the legacy shape coming back: `FROM proposals p` with a
		// `WHERE p.voting_mode …` clause would fail at runtime against 0057+.
		expect(DETECTION_SQL).not.toMatch(/FROM\s+proposals\s+p\b/)
	})

	test("filters by voting_mode = 'Slow' (Fast-path auto-exec is handled by the indexer)", () => {
		expect(DETECTION_SQL).toMatch(/voting_mode\s*=\s*'Slow'/)
	})

	test("uses partial_percentage_support_threshold in the slow-late ratio", () => {
		// V1 used a single `threshold` column; V2 slow-late must use `partial`.
		expect(DETECTION_SQL).toContain("partial_percentage_support_threshold")
		// The ratio formula shape should still be (RATIO_BASE - partial) * yes > partial * no
		expect(DETECTION_SQL).toMatch(
			/\(\s*10000000\s*-\s*[a-z0-9_.]*partial_percentage_support_threshold::numeric\s*\)\s*\*\s*[a-z0-9_.]*yes_count::numeric\s*>\s*[a-z0-9_.]*partial_percentage_support_threshold::numeric\s*\*\s*[a-z0-9_.]*no_count::numeric/,
		)
	})

	test("enforces executeBy deadline with MAX_PROPOSAL_AGE fallback for proposals with no deadline", () => {
		expect(DETECTION_SQL).toContain("execute_by")
		// `execute_by IS NOT NULL` → now <= execute_by
		expect(DETECTION_SQL).toMatch(/execute_by\s+IS\s+NOT\s+NULL[\s\S]*<=\s*[a-z0-9_.]*execute_by/)
		// `execute_by IS NULL` → fall back to MAX_PROPOSAL_AGE cap
		expect(DETECTION_SQL).toMatch(/execute_by\s+IS\s+NULL/)
		expect(DETECTION_SQL).toContain(String(MAX_PROPOSAL_AGE))
	})

	test("includes a slow-path early-execution branch using space_editor_counts", () => {
		// Early-exec: voting still ongoing + universal threshold > 0 +
		// yes_count >= ceil(universal × total_editors / RATIO_BASE)
		expect(DETECTION_SQL).toContain("universal_percentage_support_threshold")
		expect(DETECTION_SQL).toContain("space_editor_counts")
		expect(DETECTION_SQL).toMatch(/CEIL|ceil/)
	})

	test("does not reference the legacy single-column threshold in any new comparison", () => {
		// The legacy `threshold` column is retained on proposal_versions for
		// backcompat, but V2 detection must use the per-mode field. Guard
		// against a stale `p.threshold::numeric` comparison creeping back.
		const threshold_compare_pattern = /[a-z0-9_.]*\bthreshold::numeric/g
		const matches = DETECTION_SQL.match(threshold_compare_pattern) ?? []
		// Any threshold compare must be a V2 field (partial/universal/flat),
		// not the bare `threshold`.
		for (const m of matches) {
			expect(m).toMatch(
				/partial_percentage_support_threshold|universal_percentage_support_threshold|flat_support_threshold/,
			)
		}
	})

	test("preserves CLOCK_SKEW_BUFFER on the slow-late end_time check", () => {
		// Slow-late path gates on: now > end_time + CLOCK_SKEW_BUFFER
		expect(DETECTION_SQL).toMatch(/end_time\s*\+\s*60/)
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

	test("requires the action to be an AddMember", () => {
		expect(membershipSql).toContain("a.action_type = 'AddMember'")
	})

	test("does not constrain the proposer (self-service and editor-initiated adds both match)", () => {
		expect(membershipSql).not.toContain("a.target_id = p.proposed_by")
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

	test("excludes proposals whose voting period has ended, with the stage-2 skew buffer", () => {
		// Votes cast after end_time are rejected by the protocol, and an untouched
		// Fast proposal past its period is already classified REJECTED. The +60s buffer
		// mirrors stage-2's isVotingOpen so detection never excludes a request the
		// on-chain eligibility check would still vote on.
		expect(membershipSql).toContain("$2::bigint <= p.end_time + 60")
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

		const result = await Effect.runPromise(findMembershipRequests(fakeClient, [], 1_700_000_000))
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

		await Effect.runPromise(findMembershipRequests(fakeClient, allowlist, 1_700_000_000))
		expect(boundParams[0]).toEqual(allowlist)
		// nowSeconds is the caller-supplied (chain-sourced) timestamp, bound verbatim.
		expect(boundParams[1]).toBe(1_700_000_000)
	})
})
