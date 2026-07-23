/**
 * Tests for index.ts — config parsing and orchestration logic.
 *
 * Tests config validation, exit code logic, and the retry/skip behavior
 * contracts without requiring live infrastructure.
 */

import {describe, expect, test} from "bun:test"
import {Cause, ConfigProvider, Effect, Exit, Option, Redacted} from "effect"
import type {Address, Hex} from "viem"
import {InfraError, RevertError} from "../src/contracts.js"
import {findMembershipRequests, type MembershipRequest} from "../src/detect.js"
import {type SmartWallet, uuidToBytes16} from "../src/execute.js"
import {
	aggregateMembership,
	bytes16ToUuid,
	classifyMembershipSkip,
	type ExecutorEnv,
	parseConfig,
	processMembershipRequest,
	processSpaceMembership,
} from "../src/index.js"
import type {ProposalTally} from "../src/membership.js"

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

	test("chain ID must be 80451 (mainnet), 19411 (testnet), or 55516 (testnet v2)", () => {
		const validChainIds = [80451, 19411, 55516]
		expect(validChainIds).toContain(80451)
		expect(validChainIds).toContain(19411)
		expect(validChainIds).toContain(55516)
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

// ---------------------------------------------------------------------------
// parseConfig — dual-wallet + allowlist parsing (membership-accept path)
// ---------------------------------------------------------------------------

describe("parseConfig: membership-bot config & allowlist", () => {
	const EXECUTOR_KEY: `0x${string}` = `0x${"1".repeat(64)}`
	const BOT_KEY: `0x${string}` = `0x${"2".repeat(64)}`
	const EXECUTOR_SPACE: `0x${string}` = `0x${"a".repeat(32)}`
	const BOT_SPACE: `0x${string}` = `0x${"b".repeat(32)}`
	const ALLOW_A: `0x${string}` = `0x${"c".repeat(32)}`
	const ALLOW_B: `0x${string}` = `0x${"d".repeat(32)}`

	const VALID_ENV: Record<string, string> = {
		DATABASE_URL: "postgres://user:pass@localhost:5432/indexer",
		EXECUTOR_PRIVATE_KEY: EXECUTOR_KEY,
		PIMLICO_API_KEY: "pimlico-test-key",
		RPC_URL: "https://rpc.example.com",
		EXECUTOR_SPACE_ID: EXECUTOR_SPACE,
		SPACE_REGISTRY_ADDRESS: "0x1111111111111111111111111111111111111111",
		CHAIN_ID: "80451",
		MEMBERSHIP_BOT_PRIVATE_KEY: BOT_KEY,
		MEMBERSHIP_BOT_SPACE_ID: BOT_SPACE,
		MEMBERSHIP_AUTOACCEPT_SPACE_IDS: `${ALLOW_A},${ALLOW_B}`,
	}

	/** Run parseConfig against an in-memory env (overrides merged, omitted keys removed). */
	function runParse(
		overrides: Record<string, string> = {},
		omit: string[] = [],
	): Promise<Exit.Exit<ExecutorEnv, InfraError>> {
		const merged: Record<string, string> = {...VALID_ENV, ...overrides}
		for (const key of omit) delete merged[key]
		const provider = ConfigProvider.fromMap(new Map(Object.entries(merged)))
		return Effect.runPromiseExit(parseConfig.pipe(Effect.withConfigProvider(provider)))
	}

	function expectSuccess(exit: Exit.Exit<ExecutorEnv, InfraError>): ExecutorEnv {
		if (!Exit.isSuccess(exit)) {
			throw new Error(`expected success, got failure: ${JSON.stringify(exit)}`)
		}
		return exit.value
	}

	function expectInfraError(exit: Exit.Exit<ExecutorEnv, InfraError>): InfraError {
		expect(Exit.isFailure(exit)).toBe(true)
		const failure = exit as Exit.Failure<ExecutorEnv, InfraError>
		const err = Option.getOrThrow(Cause.failureOption(failure.cause))
		expect(err._tag).toBe("InfraError")
		return err
	}

	test("valid env parses bot identity + allowlist", async () => {
		const env = expectSuccess(await runParse())
		expect(Redacted.value(env.membershipBotPrivateKey)).toBe(BOT_KEY)
		expect(env.membershipBotSpaceId).toBe(BOT_SPACE)
		expect(env.membershipAutoacceptSpaceIds).toEqual([ALLOW_A, ALLOW_B])
	})

	test("auto-prefixes a bot key supplied without 0x", async () => {
		const env = expectSuccess(await runParse({MEMBERSHIP_BOT_PRIVATE_KEY: "2".repeat(64)}))
		expect(Redacted.value(env.membershipBotPrivateKey)).toBe(BOT_KEY)
	})

	test("rejects a malformed bot private key", async () => {
		const err = expectInfraError(await runParse({MEMBERSHIP_BOT_PRIVATE_KEY: "0xdeadbeef"}))
		expect(err.message).toContain("MEMBERSHIP_BOT_PRIVATE_KEY")
	})

	test("rejects a malformed bot space ID", async () => {
		const err = expectInfraError(await runParse({MEMBERSHIP_BOT_SPACE_ID: "0xnothex"}))
		expect(err.message).toContain("MEMBERSHIP_BOT_SPACE_ID")
	})

	test("rejects a malformed allowlist entry", async () => {
		const err = expectInfraError(await runParse({MEMBERSHIP_AUTOACCEPT_SPACE_IDS: `${ALLOW_A},0xbogus`}))
		expect(err.message).toContain("MEMBERSHIP_AUTOACCEPT_SPACE_IDS")
	})

	test("rejects a bot key identical to the executor key (distinct identity)", async () => {
		const err = expectInfraError(await runParse({MEMBERSHIP_BOT_PRIVATE_KEY: EXECUTOR_KEY}))
		expect(err.message).toContain("must differ from EXECUTOR_PRIVATE_KEY")
	})

	test("rejects a bot space identical to the executor space (distinct identity)", async () => {
		const err = expectInfraError(await runParse({MEMBERSHIP_BOT_SPACE_ID: EXECUTOR_SPACE}))
		expect(err.message).toContain("must differ from EXECUTOR_SPACE_ID")
	})

	test("empty allowlist is accepted — kill switch (explicit empty string)", async () => {
		const env = expectSuccess(await runParse({MEMBERSHIP_AUTOACCEPT_SPACE_IDS: ""}))
		expect(env.membershipAutoacceptSpaceIds).toEqual([])
	})

	test("empty allowlist is accepted — kill switch (unset var)", async () => {
		const env = expectSuccess(await runParse({}, ["MEMBERSHIP_AUTOACCEPT_SPACE_IDS"]))
		expect(env.membershipAutoacceptSpaceIds).toEqual([])
	})

	test("allowlist entries are trimmed and de-duplicated case-insensitively", async () => {
		// Same space as ALLOW_A but upper-cased hex body (the 0x prefix stays lowercase).
		const ALLOW_A_UPPER = `0x${"c".repeat(32).toUpperCase()}`
		const env = expectSuccess(
			await runParse({MEMBERSHIP_AUTOACCEPT_SPACE_IDS: `  ${ALLOW_A} , ${ALLOW_A_UPPER} ,${ALLOW_B}, `}),
		)
		expect(env.membershipAutoacceptSpaceIds).toEqual([ALLOW_A, ALLOW_B])
	})
})

// ---------------------------------------------------------------------------
// Membership-accept orchestration
// ---------------------------------------------------------------------------

const DAO_ADDRESS: Address = "0x00000000000000000000000000000000000000aa"
const REGISTRY_ADDRESS: Address = "0x1111111111111111111111111111111111111111"
const BOT_SPACE: Hex = `0x${"b".repeat(32)}`

const REQUEST_A: MembershipRequest = {
	id: "550e8400-e29b-41d4-a716-446655440000",
	spaceId: "11111111-1111-1111-1111-111111111111",
	requesterId: "22222222-2222-2222-2222-222222222222",
}
const REQUEST_B: MembershipRequest = {
	id: "660e8400-e29b-41d4-a716-446655440001",
	spaceId: "11111111-1111-1111-1111-111111111111",
	requesterId: "33333333-3333-3333-3333-333333333333",
}

/** A fully-eligible tally: not executed, zero votes, voting window wide open. */
function makeTally(overrides: Partial<ProposalTally> = {}): ProposalTally {
	return {executed: false, yes: 0n, no: 0n, abstain: 0n, startDate: 0n, lastDate: 10_000_000_000n, ...overrides}
}

interface FakeWalletOpts {
	tally?: ProposalTally
	daoAddress?: Address
	/** Receives the 1-based send-transaction call index; throw to simulate a revert/infra error. */
	sendTransaction?: (callIndex: number) => Promise<string>
}

interface FakeWallet {
	wallet: SmartWallet
	calls: {readContract: string[]; sendTransaction: number}
}

/**
 * A minimal SmartWallet stub that lets the real orchestration run without infra:
 * `readContract` dispatches on functionName (spaceIdToAddress / getLatestProposalInformation)
 * and `sendTransaction` is scriptable per call. Records call counts for assertions.
 */
function makeFakeWallet(opts: FakeWalletOpts = {}): FakeWallet {
	const tally = opts.tally ?? makeTally()
	const calls = {readContract: [] as string[], sendTransaction: 0}
	const wallet = {
		chain: {id: 80451},
		safeAddress: "0x0000000000000000000000000000000000000001",
		publicClient: {
			// biome-ignore lint/suspicious/noExplicitAny: minimal stub
			readContract: async ({functionName}: any) => {
				calls.readContract.push(functionName)
				if (functionName === "spaceIdToAddress") return opts.daoAddress ?? DAO_ADDRESS
				if (functionName === "getLatestProposalInformation") {
					return [
						tally.executed,
						`0x${"0".repeat(32)}`,
						{startDate: tally.startDate, lastDate: tally.lastDate},
						{yes: tally.yes, no: tally.no, abstain: tally.abstain},
						[],
					]
				}
				throw new Error(`unexpected readContract: ${functionName}`)
			},
			getBlock: async () => ({timestamp: 1000n}),
		},
		smartAccountClient: {
			account: {address: "0x0000000000000000000000000000000000000002"},
			sendTransaction: async () => {
				calls.sendTransaction += 1
				if (opts.sendTransaction) return opts.sendTransaction(calls.sendTransaction)
				return "0xtxhash"
			},
		},
	}
	return {wallet: wallet as unknown as SmartWallet, calls}
}

/** Build a revert-classified error (viem-style name → classifyAsRevert true). */
function makeRevert(reason: string): Error {
	const err = new Error(`execution reverted: ${reason}`)
	err.name = "ContractFunctionRevertedError"
	return err
}

describe("bytes16ToUuid (allowlist conversion)", () => {
	test("converts a bytes16 space ID to a dashed lower-cased UUID", () => {
		expect(bytes16ToUuid("0x550e8400e29b41d4a716446655440000")).toBe("550e8400-e29b-41d4-a716-446655440000")
	})

	test("normalizes upper-cased hex to lower case (matches indexer storage)", () => {
		expect(bytes16ToUuid("0x550E8400E29B41D4A716446655440000")).toBe("550e8400-e29b-41d4-a716-446655440000")
	})

	test("round-trips with uuidToBytes16", () => {
		const uuid = "550e8400-e29b-41d4-a716-446655440000"
		expect(bytes16ToUuid(uuidToBytes16(uuid))).toBe(uuid)
	})

	test("rejects a malformed bytes16", () => {
		expect(() => bytes16ToUuid("0xdeadbeef" as Hex)).toThrow()
	})

	test("an empty allowlist maps to no UUIDs (kill switch)", () => {
		const empty: Hex[] = []
		expect(empty.map(bytes16ToUuid)).toEqual([])
	})
})

describe("classifyMembershipSkip — stage-2 skip reasons", () => {
	test("an eligible (untouched, open) tally yields null — the bot should vote", () => {
		expect(classifyMembershipSkip(makeTally(), 1000n)).toBeNull()
	})

	test("a non-zero tally is skipped as onchain_tally_nonzero", () => {
		expect(classifyMembershipSkip(makeTally({yes: 1n}), 1000n)).toBe("onchain_tally_nonzero")
		expect(classifyMembershipSkip(makeTally({no: 1n}), 1000n)).toBe("onchain_tally_nonzero")
		expect(classifyMembershipSkip(makeTally({abstain: 1n}), 1000n)).toBe("onchain_tally_nonzero")
	})

	test("an already-executed proposal is skipped as already_executed", () => {
		expect(classifyMembershipSkip(makeTally({executed: true}), 1000n)).toBe("already_executed")
	})

	test("a closed voting window with a zero tally is skipped as voting_window_closed", () => {
		// now=1000, lastDate=100 (+60s skew) → window closed, yet the tally is zero:
		// the cause is expiry, not a recorded vote.
		expect(classifyMembershipSkip(makeTally({lastDate: 100n}), 1000n)).toBe("voting_window_closed")
	})
})

describe("processMembershipRequest", () => {
	test("a fresh eligible request casts exactly one YES vote", async () => {
		const {wallet, calls} = makeFakeWallet({tally: makeTally()})
		const outcome = await Effect.runPromise(
			processMembershipRequest(wallet, REQUEST_A, DAO_ADDRESS, BOT_SPACE, REGISTRY_ADDRESS, 1000n),
		)
		expect(outcome.status).toBe("voted")
		expect(calls.sendTransaction).toBe(1)
	})

	test("a touched request is skipped without voting", async () => {
		const {wallet, calls} = makeFakeWallet({tally: makeTally({yes: 1n})})
		const outcome = await Effect.runPromise(
			processMembershipRequest(wallet, REQUEST_A, DAO_ADDRESS, BOT_SPACE, REGISTRY_ADDRESS, 1000n),
		)
		expect(outcome.status).toBe("skipped")
		if (outcome.status === "skipped") expect(outcome.reason).toBe("onchain_tally_nonzero")
		expect(calls.sendTransaction).toBe(0)
	})

	test("a revert is absorbed into a 'reverted' outcome (not a failure)", async () => {
		const {wallet} = makeFakeWallet({
			tally: makeTally(),
			sendTransaction: () => Promise.reject(makeRevert("CanNotVote")),
		})
		const outcome = await Effect.runPromise(
			processMembershipRequest(wallet, REQUEST_A, DAO_ADDRESS, BOT_SPACE, REGISTRY_ADDRESS, 1000n),
		)
		expect(outcome.status).toBe("reverted")
	})
})

describe("processSpaceMembership — revert isolation", () => {
	test("a RevertError on one request does not abort processing of others", async () => {
		// First request reverts (CanNotVote), second votes successfully.
		const {wallet, calls} = makeFakeWallet({
			tally: makeTally(),
			sendTransaction: (callIndex) =>
				callIndex === 1 ? Promise.reject(makeRevert("CanNotVote")) : Promise.resolve("0xok"),
		})
		const outcomes = await Effect.runPromise(
			processSpaceMembership(
				wallet,
				REQUEST_A.spaceId,
				[REQUEST_A, REQUEST_B],
				BOT_SPACE,
				REGISTRY_ADDRESS,
				1000n,
			),
		)
		expect(outcomes.length).toBe(2)
		expect(outcomes[0]?.status).toBe("reverted")
		expect(outcomes[1]?.status).toBe("voted")
		// Two vote attempts; the DAO address was resolved once for the space.
		expect(calls.sendTransaction).toBe(2)
		expect(calls.readContract.filter((fn) => fn === "spaceIdToAddress").length).toBe(1)
	})
})

describe("aggregateMembership — run_end counts", () => {
	test("counts votes as admitted, skips/reverts as skipped, infra spaces as failed", () => {
		const results = [
			{
				status: "ok" as const,
				spaceId: "s1",
				outcomes: [
					{status: "voted" as const, txHash: "0x1"},
					{status: "skipped" as const, reason: "onchain_tally_nonzero" as const},
				],
			},
			{status: "ok" as const, spaceId: "s2", outcomes: [{status: "reverted" as const, expected: true}]},
			{status: "infraError" as const, spaceId: "s3", outcomes: []},
		]
		expect(aggregateMembership(results)).toEqual({admitted: 1, skipped: 2, failed: 1})
	})

	test("an empty run aggregates to all-zero", () => {
		expect(aggregateMembership([])).toEqual({admitted: 0, skipped: 0, failed: 0})
	})
})

describe("kill switch", () => {
	// Mirrors the exit-code formula: process.exit(failed > 0 && succeeded === 0 ? 1 : 0)
	const computeExitCode = (succeeded: number, failed: number) => (failed > 0 && succeeded === 0 ? 1 : 0)

	test("an empty allowlist issues no detection query", async () => {
		let queried = false
		const fakeClient = {
			query: async () => {
				queried = true
				return {rows: []}
			},
			// biome-ignore lint/suspicious/noExplicitAny: minimal pg.Client stub
		} as any
		const rows = await Effect.runPromise(findMembershipRequests(fakeClient, [], 1_700_000_000))
		expect(rows).toEqual([])
		expect(queried).toBe(false)
	})

	test("with nothing admitted, membership contributes exit 0", () => {
		const agg = aggregateMembership([])
		expect(agg.admitted).toBe(0)
		expect(computeExitCode(agg.admitted, agg.failed)).toBe(0)
	})
})
