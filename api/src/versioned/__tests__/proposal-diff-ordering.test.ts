import {describe, expect, it} from "vitest"
import type {NormalizedUuid} from "../../utils/uuid"
import {compareGroupedEdits} from "../proposal-diff"

type Edit = {createdAt: bigint; proposalId: NormalizedUuid}

const uuid = (s: string) => s as NormalizedUuid

describe("compareGroupedEdits (RFC 0004 ordering)", () => {
	it("orders strictly by createdAt ascending when timestamps differ", () => {
		const edits: Edit[] = [
			{createdAt: 3000n, proposalId: uuid("11111111-1111-4111-8111-111111111111")},
			{createdAt: 1000n, proposalId: uuid("22222222-2222-4222-8222-222222222222")},
			{createdAt: 2000n, proposalId: uuid("33333333-3333-4333-8333-333333333333")},
		]

		const sorted = [...edits].sort(compareGroupedEdits).map((e) => e.createdAt)

		expect(sorted).toEqual([1000n, 2000n, 3000n])
	})

	it("tiebreaks by proposalId ascending when createdAt is equal", () => {
		const HIGH = uuid("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee")
		const LOW = uuid("11111111-1111-4111-8111-111111111111")

		const edits: Edit[] = [
			{createdAt: 1000n, proposalId: HIGH},
			{createdAt: 1000n, proposalId: LOW},
		]

		const sorted = [...edits].sort(compareGroupedEdits).map((e) => e.proposalId)

		expect(sorted).toEqual([LOW, HIGH])
	})

	it("applies createdAt order first, proposalId only as tiebreaker", () => {
		// Earlier timestamp wins even with a lexicographically later proposalId.
		const EARLY_LATE_ID = uuid("ffffffff-ffff-4fff-8fff-ffffffffffff")
		const LATE_EARLY_ID = uuid("00000000-0000-4000-8000-000000000001")

		const edits: Edit[] = [
			{createdAt: 2000n, proposalId: LATE_EARLY_ID},
			{createdAt: 1000n, proposalId: EARLY_LATE_ID},
		]

		const sorted = [...edits].sort(compareGroupedEdits)

		expect(sorted[0]?.createdAt).toBe(1000n)
		expect(sorted[0]?.proposalId).toBe(EARLY_LATE_ID)
		expect(sorted[1]?.createdAt).toBe(2000n)
		expect(sorted[1]?.proposalId).toBe(LATE_EARLY_ID)
	})

	it("is stable for exactly-equal edits (deduped input)", () => {
		const edits: Edit[] = [
			{createdAt: 1000n, proposalId: uuid("11111111-1111-4111-8111-111111111111")},
			{createdAt: 1000n, proposalId: uuid("11111111-1111-4111-8111-111111111111")},
		]

		expect(compareGroupedEdits(edits[0] as Edit, edits[1] as Edit)).toBe(0)
	})

	it("handles bigints that exceed Number.MAX_SAFE_INTEGER without precision loss", () => {
		// microsecond timestamps near the end of 2262 approach 2^63; the previous
		// implementation computed `Number(a - b)` which silently lost precision
		// on any difference smaller than the bigint span.
		const BIG_A = 9_007_199_254_740_993n // MAX_SAFE_INTEGER + 2
		const BIG_B = 9_007_199_254_740_992n // MAX_SAFE_INTEGER + 1
		const edits: Edit[] = [
			{createdAt: BIG_A, proposalId: uuid("22222222-2222-4222-8222-222222222222")},
			{createdAt: BIG_B, proposalId: uuid("11111111-1111-4111-8111-111111111111")},
		]

		const sorted = [...edits].sort(compareGroupedEdits).map((e) => e.createdAt)

		// B is smaller and must come first. `Number(BIG_A - BIG_B) === 0` under
		// the old implementation because both round to the same float — this
		// test asserts the new bigint comparison doesn't lose the ordering.
		expect(sorted).toEqual([BIG_B, BIG_A])
	})
})
