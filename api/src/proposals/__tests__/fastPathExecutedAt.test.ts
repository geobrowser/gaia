/**
 * Equivalence of the three statements of the Fast-path threshold rule: geo-db's vote fold
 * (`yes >= max(flat, 1)`, collection 31), the TypeScript status function, and the SQL fragment in
 * `queries.ts` that the list and filter endpoints run. Plus the reader's flag and decoding.
 */

import {PgDialect} from "drizzle-orm/pg-core"
import {describe, expect, it} from "vitest"
import {sqlIsAccepted, sqlIsExecutable} from "../queries"
import {
	computeProposalStatus,
	decodeFastPathExecutionFact,
	FASTPATH_EXECUTED_AT_FACT_ENV,
	foldReachesFastThreshold,
	isFastPathFactReadEnabled,
	resolveExecutedAt,
} from "../status"
import type {ProposalWithVotes} from "../types"

const now = 1_800_000_000n

function fastProposal(yesCount: bigint, flat: bigint): ProposalWithVotes {
	return {
		id: "p",
		votingMode: "Fast",
		threshold: flat,
		flatSupportThreshold: flat,
		yesCount,
		noCount: 0n,
		abstainCount: 0n,
		quorum: 0n,
		endTime: now + 3600n,
		executeBy: 0n,
		executedAt: null,
		unexecutableAt: null,
		totalEditors: 0n,
		universalPercentageSupportThreshold: 0n,
		partialPercentageSupportThreshold: 0n,
		votes: [],
		// biome-ignore lint/suspicious/noExplicitAny: only the status inputs matter here
	} as any
}

function render(fragment: ReturnType<typeof sqlIsExecutable>): string {
	return new PgDialect().sqlToQuery(fragment).sql.replace(/\s+/g, " ")
}

/** Evaluates the Fast arm exactly as the SQL text spells it. */
function sqlFastArm(yes: bigint, flat: bigint): boolean {
	return yes > (flat === 0n ? 0n : flat - 1n)
}

describe("fast-path threshold rule: fold, status.ts and SQL agree", () => {
	it("the SQL fragment spells the arm the test evaluates", () => {
		expect(render(sqlIsExecutable(now))).toContain(
			"p.voting_mode = 'Fast' AND p.yes_count > (CASE WHEN p.flat_support_threshold = 0 THEN 0 ELSE p.flat_support_threshold - 1 END)",
		)
		expect(render(sqlIsAccepted())).toContain("p.executed_at IS NOT NULL")
	})

	it("agrees on every (yes, flat) pair around the boundary", () => {
		for (let flat = 0n; flat <= 6n; flat++) {
			for (let yes = 0n; yes <= 8n; yes++) {
				const fold = foldReachesFastThreshold(yes, flat)
				const status = computeProposalStatus(fastProposal(yes, flat), now).isThresholdReached
				expect(fold, `fold yes=${yes} flat=${flat}`).toBe(sqlFastArm(yes, flat))
				expect(status, `status yes=${yes} flat=${flat}`).toBe(fold)
			}
		}
	})

	it("flat 0 and flat 1 both need exactly one yes", () => {
		expect(foldReachesFastThreshold(0n, 0n)).toBe(false)
		expect(foldReachesFastThreshold(1n, 0n)).toBe(true)
		expect(foldReachesFastThreshold(0n, 1n)).toBe(false)
		expect(foldReachesFastThreshold(1n, 1n)).toBe(true)
		expect(foldReachesFastThreshold(2n, 3n)).toBe(false)
		expect(foldReachesFastThreshold(3n, 3n)).toBe(true)
	})
})

describe("fast-path execution fact reader", () => {
	const fact = {version: 2, seconds: 1_799_999_000n}

	it("is off unless the variable is exactly true", () => {
		expect(isFastPathFactReadEnabled({})).toBe(false)
		expect(isFastPathFactReadEnabled({[FASTPATH_EXECUTED_AT_FACT_ENV]: "1"})).toBe(false)
		expect(isFastPathFactReadEnabled({[FASTPATH_EXECUTED_AT_FACT_ENV]: "true"})).toBe(true)
	})

	it("changes nothing while off", () => {
		expect(resolveExecutedAt(null, fact, false)).toBeNull()
		expect(resolveExecutedAt(5n, fact, false)).toBe(5n)
	})

	it("fills a missing executedAt from the fact when on, and never overrides a stored one", () => {
		expect(resolveExecutedAt(null, fact, true)).toBe(fact.seconds)
		expect(resolveExecutedAt(5n, fact, true)).toBe(5n)
		expect(resolveExecutedAt(null, null, true)).toBeNull()
	})

	it("decodes the 12-byte value geo-db writes and refuses other lengths", () => {
		const bytes = new Uint8Array(12)
		const view = new DataView(bytes.buffer)
		view.setUint32(0, 2, true)
		view.setBigInt64(4, 1_799_999_000n, true)
		expect(decodeFastPathExecutionFact(bytes)).toEqual(fact)
		expect(decodeFastPathExecutionFact(new Uint8Array(11))).toBeNull()
	})
})
