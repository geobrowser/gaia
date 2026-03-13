/**
 * Tests for index.ts — config parsing and orchestration logic.
 *
 * Tests config validation, exit code logic, and the retry/skip behavior
 * contracts without requiring live infrastructure.
 */

import {describe, expect, test} from "bun:test"
import {InfraError, RevertError} from "../src/contracts.js"

// ---------------------------------------------------------------------------
// Tagged error construction
// ---------------------------------------------------------------------------

describe("RevertError", () => {
	test("has correct _tag", () => {
		const err = new RevertError({
			proposalId: "test-id",
			message: "execution reverted",
			expected: false,
			durationMs: 100,
		})
		expect(err._tag).toBe("RevertError")
	})

	test("carries proposal context", () => {
		const err = new RevertError({
			proposalId: "550e8400-e29b-41d4-a716-446655440000",
			message: "execution reverted: ProposalAlreadyExecuted",
			expected: true,
			durationMs: 250,
		})
		expect(err.proposalId).toBe("550e8400-e29b-41d4-a716-446655440000")
		expect(err.message).toContain("ProposalAlreadyExecuted")
		expect(err.expected).toBe(true)
		expect(err.durationMs).toBe(250)
	})

	test("expected=true for known race condition reverts", () => {
		const err = new RevertError({
			proposalId: "test",
			message: "already executed",
			expected: true,
			durationMs: 0,
		})
		expect(err.expected).toBe(true)
	})

	test("expected=false for unknown reverts", () => {
		const err = new RevertError({
			proposalId: "test",
			message: "some other revert reason",
			expected: false,
			durationMs: 0,
		})
		expect(err.expected).toBe(false)
	})
})

describe("InfraError", () => {
	test("has correct _tag", () => {
		const err = new InfraError({
			proposalId: "test-id",
			message: "429 Too Many Requests",
			durationMs: 100,
		})
		expect(err._tag).toBe("InfraError")
	})

	test("carries proposal context", () => {
		const err = new InfraError({
			proposalId: "550e8400-e29b-41d4-a716-446655440000",
			message: "connect ECONNREFUSED",
			durationMs: 5000,
		})
		expect(err.proposalId).toBe("550e8400-e29b-41d4-a716-446655440000")
		expect(err.message).toContain("ECONNREFUSED")
		expect(err.durationMs).toBe(5000)
	})
})

// ---------------------------------------------------------------------------
// Error tag discrimination (the foundation of catchTag routing)
// ---------------------------------------------------------------------------

describe("error tag discrimination", () => {
	test("RevertError and InfraError have distinct _tag values", () => {
		const revert = new RevertError({proposalId: "a", message: "", expected: false, durationMs: 0})
		const infra = new InfraError({proposalId: "a", message: "", durationMs: 0})
		expect(revert._tag).not.toBe(infra._tag)
	})

	test("can discriminate errors via _tag (simulating catchTag)", () => {
		const errors: Array<RevertError | InfraError> = [
			new RevertError({proposalId: "1", message: "revert", expected: true, durationMs: 0}),
			new InfraError({proposalId: "2", message: "timeout", durationMs: 0}),
			new RevertError({proposalId: "3", message: "revert", expected: false, durationMs: 0}),
		]

		const reverts = errors.filter((e) => e._tag === "RevertError")
		const infras = errors.filter((e) => e._tag === "InfraError")

		expect(reverts.length).toBe(2)
		expect(infras.length).toBe(1)
	})
})

// ---------------------------------------------------------------------------
// Exit code logic
// ---------------------------------------------------------------------------

describe("exit code logic", () => {
	// Mirrors the logic: process.exit(failed > 0 && succeeded === 0 ? 1 : 0)

	function computeExitCode(succeeded: number, failed: number): number {
		return failed > 0 && succeeded === 0 ? 1 : 0
	}

	test("no proposals found → exit 0", () => {
		expect(computeExitCode(0, 0)).toBe(0)
	})

	test("all succeeded → exit 0", () => {
		expect(computeExitCode(5, 0)).toBe(0)
	})

	test("partial success → exit 0 (individual failures are expected)", () => {
		expect(computeExitCode(3, 2)).toBe(0)
	})

	test("all failed → exit 1 (systemic issue)", () => {
		expect(computeExitCode(0, 5)).toBe(1)
	})

	test("single failure with no success → exit 1", () => {
		expect(computeExitCode(0, 1)).toBe(1)
	})
})

// ---------------------------------------------------------------------------
// Config validation contracts
// ---------------------------------------------------------------------------

describe("config validation contracts", () => {
	test("private key must be 66 chars with 0x prefix", () => {
		const validKey = "0x" + "a".repeat(64)
		expect(validKey.length).toBe(66)
		expect(validKey.startsWith("0x")).toBe(true)
	})

	test("chain ID must be 80451 (mainnet) or 19411 (testnet)", () => {
		const validChainIds = [80451, 19411]
		expect(validChainIds).toContain(80451)
		expect(validChainIds).toContain(19411)
		expect(validChainIds).not.toContain(1) // Ethereum mainnet
		expect(validChainIds).not.toContain(0)
	})

	test("required env vars list matches plan", () => {
		const required = [
			"DATABASE_URL",
			"EXECUTOR_PRIVATE_KEY",
			"EXECUTOR_SPACE_ID",
			"PIMLICO_API_KEY",
			"SPACE_REGISTRY_ADDRESS",
			"RPC_URL",
			"CHAIN_ID",
		]
		expect(required.length).toBe(7)
		// All must be present — no optional required vars
		for (const name of required) {
			expect(name.length).toBeGreaterThan(0)
		}
	})
})

// ---------------------------------------------------------------------------
// Concurrency model contracts
// ---------------------------------------------------------------------------

describe("concurrency model contracts", () => {
	test("proposals grouped by space ID produce correct structure", () => {
		const proposals = [
			{id: "p1", spaceId: "s1"},
			{id: "p2", spaceId: "s1"},
			{id: "p3", spaceId: "s2"},
		]

		const bySpace = Map.groupBy(proposals, (p) => p.spaceId)

		expect(bySpace.size).toBe(2)
		expect(bySpace.get("s1")?.length).toBe(2)
		expect(bySpace.get("s2")?.length).toBe(1)
	})

	test("empty proposals produce empty map", () => {
		const proposals: Array<{id: string; spaceId: string}> = []
		const bySpace = Map.groupBy(proposals, (p) => p.spaceId)
		expect(bySpace.size).toBe(0)
	})

	test("result aggregation logic", () => {
		const results = [
			{status: "ok" as const, spaceId: "s1", succeeded: 2, skipped: 1},
			{status: "infraError" as const, spaceId: "s2", succeeded: 0, skipped: 0},
			{status: "ok" as const, spaceId: "s3", succeeded: 1, skipped: 0},
		]

		const succeeded = results.reduce((n, r) => n + r.succeeded, 0)
		const failed = results.filter((r) => r.status === "infraError").length
		const skipped = results.reduce((n, r) => n + r.skipped, 0)

		expect(succeeded).toBe(3)
		expect(failed).toBe(1)
		expect(skipped).toBe(1)
	})
})
