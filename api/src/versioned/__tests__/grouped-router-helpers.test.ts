import {describe, expect, it} from "vitest"
import {MAX_GROUP_SIZE} from "../proposal-diff"
import {mapGroupedProposalError, validateGroupedRequest} from "../router"

const SPACE = "00000000-0000-4000-8000-000000000b01"
const P1 = "00000000-0000-4000-8000-000000000001"
const P2 = "00000000-0000-4000-8000-000000000002"

function reject<T extends {ok: boolean}>(r: T): Extract<T, {ok: false}> {
	if (r.ok) throw new Error("expected validation failure but it succeeded")
	return r as Extract<T, {ok: false}>
}

function accept<T extends {ok: boolean}>(r: T): Extract<T, {ok: true}> {
	if (!r.ok) throw new Error("expected validation success but it failed")
	return r as Extract<T, {ok: true}>
}

describe("validateGroupedRequest", () => {
	it("rejects missing spaceId", () => {
		const r = reject(
			validateGroupedRequest({
				spaceId: undefined,
				proposalIds: `${P1},${P2}`,
				cursor: undefined,
				limit: undefined,
			}),
		)
		expect(r.failure.status).toBe(400)
		expect(r.failure.body.message).toContain("spaceId")
	})

	it("rejects invalid spaceId UUID", () => {
		const r = reject(
			validateGroupedRequest({
				spaceId: "not-a-uuid",
				proposalIds: `${P1},${P2}`,
				cursor: undefined,
				limit: undefined,
			}),
		)
		expect(r.failure.body.message).toContain("spaceId")
	})

	it("rejects missing proposalIds", () => {
		const r = reject(
			validateGroupedRequest({spaceId: SPACE, proposalIds: undefined, cursor: undefined, limit: undefined}),
		)
		expect(r.failure.body.message).toContain("proposalIds")
	})

	it("rejects fewer than 2 proposal IDs", () => {
		const r = reject(validateGroupedRequest({spaceId: SPACE, proposalIds: P1, cursor: undefined, limit: undefined}))
		expect(r.failure.body.message).toContain("at least 2")
	})

	it("rejects an invalid UUID in the proposalIds list", () => {
		const r = reject(
			validateGroupedRequest({
				spaceId: SPACE,
				proposalIds: `${P1},not-a-uuid`,
				cursor: undefined,
				limit: undefined,
			}),
		)
		expect(r.failure.body.message).toContain("Invalid UUID")
	})

	it("rejects when proposalIds count exceeds MAX_GROUP_SIZE", () => {
		const tooMany = Array.from(
			{length: MAX_GROUP_SIZE + 1},
			(_, i) => `00000000-0000-4000-8000-00000000${(i + 10).toString(16).padStart(4, "0")}`,
		)
		const r = reject(
			validateGroupedRequest({
				spaceId: SPACE,
				proposalIds: tooMany.join(","),
				cursor: undefined,
				limit: undefined,
			}),
		)
		expect(r.failure.body.message).toContain(`exceeds maximum of ${MAX_GROUP_SIZE}`)
	})

	it("rejects negative or zero limit", () => {
		const r1 = reject(
			validateGroupedRequest({spaceId: SPACE, proposalIds: `${P1},${P2}`, cursor: undefined, limit: "-1"}),
		)
		expect(r1.failure.body.message).toContain("limit")

		const r2 = reject(
			validateGroupedRequest({spaceId: SPACE, proposalIds: `${P1},${P2}`, cursor: undefined, limit: "0"}),
		)
		expect(r2.failure.body.message).toContain("limit")
	})

	it("rejects non-numeric limit", () => {
		const r = reject(
			validateGroupedRequest({spaceId: SPACE, proposalIds: `${P1},${P2}`, cursor: undefined, limit: "abc"}),
		)
		expect(r.failure.body.message).toContain("limit")
	})

	it("caps limit at 100 when a larger value is requested", () => {
		const r = accept(
			validateGroupedRequest({spaceId: SPACE, proposalIds: `${P1},${P2}`, cursor: undefined, limit: "500"}),
		)
		expect(r.value.limit).toBe(100)
	})

	it("uses default limit 50 when not provided", () => {
		const r = accept(
			validateGroupedRequest({spaceId: SPACE, proposalIds: `${P1},${P2}`, cursor: undefined, limit: undefined}),
		)
		expect(r.value.limit).toBe(50)
	})

	it("normalizes UUIDs to dashless lowercase hex on success", () => {
		const r = accept(
			validateGroupedRequest({
				spaceId: SPACE.toUpperCase(),
				proposalIds: `${P1.toUpperCase()},${P2}`,
				cursor: "SomeCursor==",
				limit: "25",
			}),
		)
		expect(r.value.spaceId).toBe(SPACE.replace(/-/g, "").toLowerCase())
		expect(r.value.proposalIds).toEqual([P1.replace(/-/g, "").toLowerCase(), P2.replace(/-/g, "").toLowerCase()])
		expect(r.value.cursor).toBe("SomeCursor==")
		expect(r.value.limit).toBe(25)
	})

	it("trims whitespace from proposalIds", () => {
		const r = accept(
			validateGroupedRequest({
				spaceId: SPACE,
				proposalIds: ` ${P1} , ${P2} `,
				cursor: undefined,
				limit: undefined,
			}),
		)
		expect(r.value.proposalIds).toHaveLength(2)
	})
})

describe("mapGroupedProposalError", () => {
	it("maps ValidationError → 400", () => {
		const r = mapGroupedProposalError({_tag: "ValidationError", message: "bad"} as never)
		expect(r.status).toBe(400)
		expect(r.body.error).toBe("Invalid parameter")
	})

	it("maps NotFoundError → 404", () => {
		const r = mapGroupedProposalError({_tag: "NotFoundError", message: "nope"} as never)
		expect(r.status).toBe(404)
	})

	it("maps ProposalNotFoundError → 404", () => {
		const r = mapGroupedProposalError({_tag: "ProposalNotFoundError"} as never)
		expect(r.status).toBe(404)
		expect(r.body.message).toContain("not found")
	})

	it("maps EditBlobNotCachedError → 404", () => {
		const r = mapGroupedProposalError({_tag: "EditBlobNotCachedError", uri: "ipfs://x"} as never)
		expect(r.status).toBe(404)
		expect(r.body.message).toContain("not cached")
	})

	it("maps EditBlobDecodeFailedError → 422 with uri", () => {
		const r = mapGroupedProposalError({_tag: "EditBlobDecodeFailedError", uri: "ipfs://bad"} as never)
		expect(r.status).toBe(422)
		expect(r.body.uri).toBe("ipfs://bad")
	})

	it("maps SpaceMismatchError → 400", () => {
		const r = mapGroupedProposalError({_tag: "SpaceMismatchError"} as never)
		expect(r.status).toBe(400)
		expect(r.body.message).toContain("do not belong")
	})

	it("maps InvalidCursorError → 400", () => {
		const r = mapGroupedProposalError({_tag: "InvalidCursorError", cursor: "bad"} as never)
		expect(r.status).toBe(400)
		expect(r.body.message).toContain("cursor")
	})

	it("maps GroupSizeLimitError → 400 with actual/max", () => {
		const r = mapGroupedProposalError({_tag: "GroupSizeLimitError", actual: 25, max: 20} as never)
		expect(r.status).toBe(400)
		expect(r.body.message).toContain("25")
		expect(r.body.message).toContain("20")
	})

	it("maps DuplicateProposalError → 400", () => {
		const r = mapGroupedProposalError({_tag: "DuplicateProposalError", duplicates: []} as never)
		expect(r.status).toBe(400)
		expect(r.body.message).toContain("Duplicate")
	})

	it("maps MixedModeError → 400 with counts", () => {
		const r = mapGroupedProposalError({_tag: "MixedModeError", activeCount: 2, nonActiveCount: 3} as never)
		expect(r.status).toBe(400)
		expect(r.body.message).toContain("2")
		expect(r.body.message).toContain("3")
	})

	it("maps MissingPublishActionError → 422 with proposalId", () => {
		const r = mapGroupedProposalError({_tag: "MissingPublishActionError", proposalId: P1} as never)
		expect(r.status).toBe(422)
		expect(r.body.proposalId).toBe(P1)
	})

	it("maps EditDecodeError → 500", () => {
		const r = mapGroupedProposalError({_tag: "EditDecodeError", cause: null} as never)
		expect(r.status).toBe(500)
	})

	it("maps QueryError → 500 without leaking internal details", () => {
		const r = mapGroupedProposalError({_tag: "QueryError", operation: "x", cause: "secret"} as never)
		expect(r.status).toBe(500)
		expect(JSON.stringify(r.body)).not.toContain("secret")
	})
})
