import {describe, expect, test} from "bun:test"

import {detectMembershipRequest, type MembershipRequest, SeenProposals} from "../src/detect.js"

const NOW = 1_700_000_000

/** A canonical fast-mode, single add_member proposal_created payload. */
function membershipPayload(overrides: Record<string, unknown> = {}): Record<string, unknown> {
	return {
		version: 1,
		event_type: "proposal_created",
		category: "governance",
		space_id: "d4f5a6b7-0000-0000-0000-000000000000",
		proposal_id: "c3e4f5a6-0000-0000-0000-000000000000",
		voting_mode: "fast",
		// target_address carries the requester's personal-space id (not an EOA address).
		actions: [{type: "add_member", target_address: "a1b2c3d4-0000-0000-0000-000000000000"}],
		settings: {start_date: NOW - 10, end_date: NOW + 86_400, voting_mode: "fast"},
		...overrides,
	}
}

describe("detectMembershipRequest", () => {
	test("detects a fast single-add_member proposal_created", () => {
		const req = detectMembershipRequest(membershipPayload(), NOW)
		expect(req).toEqual({
			proposalId: "c3e4f5a6-0000-0000-0000-000000000000",
			spaceId: "d4f5a6b7-0000-0000-0000-000000000000",
			requesterSpaceId: "a1b2c3d4-0000-0000-0000-000000000000",
		} satisfies MembershipRequest)
	})

	test("ignores non proposal_created events", () => {
		expect(detectMembershipRequest(membershipPayload({event_type: "proposal_voted"}), NOW)).toBeNull()
	})

	test("ignores slow voting mode", () => {
		expect(detectMembershipRequest(membershipPayload({voting_mode: "slow"}), NOW)).toBeNull()
	})

	test("ignores proposals with more than one action", () => {
		const actions = [
			{type: "add_member", target_address: "0xaaaa"},
			{type: "add_editor", target_address: "0xbbbb"},
		]
		expect(detectMembershipRequest(membershipPayload({actions}), NOW)).toBeNull()
	})

	test("ignores a single non-add_member action", () => {
		const actions = [{type: "add_editor", target_address: "0xbbbb"}]
		expect(detectMembershipRequest(membershipPayload({actions}), NOW)).toBeNull()
	})

	test("ignores an empty actions array", () => {
		expect(detectMembershipRequest(membershipPayload({actions: []}), NOW)).toBeNull()
	})

	test("ignores a payload missing proposal_id", () => {
		const p = membershipPayload()
		delete p.proposal_id
		expect(detectMembershipRequest(p, NOW)).toBeNull()
	})

	test("ignores a payload missing space_id", () => {
		const p = membershipPayload()
		delete p.space_id
		expect(detectMembershipRequest(p, NOW)).toBeNull()
	})

	test("best-effort: ignores a proposal whose end_date is already in the past", () => {
		expect(detectMembershipRequest(membershipPayload({settings: {end_date: NOW - 1}}), NOW)).toBeNull()
	})

	test("detects when end_date is in the future", () => {
		expect(detectMembershipRequest(membershipPayload({settings: {end_date: NOW + 1}}), NOW)).not.toBeNull()
	})

	test("detects when settings/end_date is absent (no window info to act on)", () => {
		const p = membershipPayload()
		delete p.settings
		expect(detectMembershipRequest(p, NOW)).not.toBeNull()
	})

	test("requesterSpaceId is empty when target_address is missing", () => {
		const actions = [{type: "add_member"}]
		expect(detectMembershipRequest(membershipPayload({actions}), NOW)?.requesterSpaceId).toBe("")
	})

	test("returns null for non-object payloads", () => {
		expect(detectMembershipRequest(null, NOW)).toBeNull()
		expect(detectMembershipRequest("string", NOW)).toBeNull()
		expect(detectMembershipRequest(42, NOW)).toBeNull()
		expect(detectMembershipRequest([], NOW)).toBeNull()
	})
})

describe("SeenProposals", () => {
	test("first sighting is not a duplicate; second is", () => {
		const seen = new SeenProposals()
		expect(seen.seen("p1")).toBe(false)
		expect(seen.seen("p1")).toBe(true)
		expect(seen.seen("p1")).toBe(true)
	})

	test("tracks distinct ids independently", () => {
		const seen = new SeenProposals()
		expect(seen.seen("p1")).toBe(false)
		expect(seen.seen("p2")).toBe(false)
		expect(seen.seen("p1")).toBe(true)
		expect(seen.seen("p2")).toBe(true)
	})

	test("unmark lets an id be processed again", () => {
		const seen = new SeenProposals()
		expect(seen.seen("p1")).toBe(false)
		expect(seen.seen("p1")).toBe(true)
		seen.unmark("p1")
		expect(seen.seen("p1")).toBe(false) // forgotten → first sighting again
		expect(seen.seen("p1")).toBe(true)
	})

	test("unmark of an unknown id is a no-op", () => {
		const seen = new SeenProposals()
		seen.unmark("never-seen")
		expect(seen.seen("never-seen")).toBe(false)
	})

	test("unmark frees the capacity slot (no phantom eviction)", () => {
		const seen = new SeenProposals(2)
		expect(seen.seen("a")).toBe(false)
		expect(seen.seen("b")).toBe(false)
		seen.unmark("a") // {b}
		expect(seen.seen("c")).toBe(false) // {b,c} — within capacity, nothing evicted
		expect(seen.seen("b")).toBe(true)
		expect(seen.seen("c")).toBe(true)
	})

	test("evicts oldest ids beyond capacity (FIFO)", () => {
		const seen = new SeenProposals(2)
		expect(seen.seen("a")).toBe(false) // {a}
		expect(seen.seen("b")).toBe(false) // {a,b}
		expect(seen.seen("c")).toBe(false) // len>2 → evict "a" → {b,c}
		expect(seen.seen("a")).toBe(false) // "a" was evicted → new again; evict "b" → {c,a}
		expect(seen.seen("c")).toBe(true) // "c" still tracked
		expect(seen.seen("b")).toBe(false) // "b" was evicted → new again
	})
})
