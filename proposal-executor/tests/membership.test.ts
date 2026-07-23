/**
 * Tests for membership.ts — vote-data encoding, the enter() vote calldata, the
 * stage-2 on-chain tally read, two-stage eligibility, and error classification.
 *
 * Encoding is pinned against the authoritative IDAOSpace.VoteOption enum (Yes = 1).
 * On-chain calls are exercised against a fake SmartWallet that captures calldata /
 * returns canned reads, so no live RPC or paymaster is needed.
 */

import {describe, expect, test} from "bun:test"
import {Effect} from "effect"
import {type Address, decodeAbiParameters, decodeFunctionData, type Hex} from "viem"
import {PROPOSAL_VOTED_ACTION, SpaceRegistryAbi, VOTE_YES} from "../src/contracts.js"
import type {MembershipRequest} from "../src/detect.js"
import type {SmartWallet} from "../src/execute.js"
import {
	castMembershipVote,
	encodeVoteData,
	isEligibleToVote,
	isVotingOpen,
	type ProposalTally,
	readChainTimeSeconds,
	readProposalTally,
	resolveDaoSpaceAddress,
} from "../src/membership.js"

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const PROPOSAL_UUID = "550e8400-e29b-41d4-a716-446655440000"
const PROPOSAL_BYTES16 = "0x550e8400e29b41d4a716446655440000"
const SPACE_UUID = "660e8400-e29b-41d4-a716-446655440000"
const SPACE_BYTES16 = "0x660e8400e29b41d4a716446655440000"
const BOT_SPACE_ID = "0x770e8400e29b41d4a716446655440000" as Hex
const REGISTRY = "0x1111111111111111111111111111111111111111" as Address
const DAO_SPACE_ADDR = "0x2222222222222222222222222222222222222222" as Address

const REQUEST: MembershipRequest = {
	id: PROPOSAL_UUID,
	spaceId: SPACE_UUID,
	requesterId: "880e8400-e29b-41d4-a716-446655440000",
}

// Voting-window fixtures: NOW sits comfortably inside an open window.
const NOW = 1_700_000_000n
const START_DATE = NOW - 3_600n // opened an hour ago
const LAST_DATE = NOW + 3_600n // closes in an hour
/** A ProposalParameters tuple whose only fields the read uses are startDate/lastDate. */
const OPEN_PARAMS = {startDate: START_DATE, lastDate: LAST_DATE}

/**
 * Minimal SmartWallet stub. sendTransaction / readContract / getBlock are injected
 * per-test; the account is present by default so the "no account" guard does not trip.
 */
function fakeWallet(overrides: {
	sendTransaction?: (args: {data: Hex; to: Address}) => Promise<string>
	readContract?: (args: {functionName: string; args: readonly unknown[]}) => Promise<unknown>
	getBlock?: () => Promise<{timestamp: bigint}>
}): SmartWallet {
	return {
		smartAccountClient: {
			account: {address: "0xbot"},
			sendTransaction: overrides.sendTransaction ?? (async () => "0xhash"),
		},
		publicClient: {
			readContract: overrides.readContract ?? (async () => undefined),
			getBlock: overrides.getBlock ?? (async () => ({timestamp: NOW})),
		},
		chain: {id: 19411},
		accountAddress: "0xsafe",
	} as unknown as SmartWallet
}

// ---------------------------------------------------------------------------
// encodeVoteData
// ---------------------------------------------------------------------------

describe("encodeVoteData", () => {
	test("encodes abi.encode(bytes16 proposalId, uint8 voteOption)", () => {
		const data = encodeVoteData(PROPOSAL_BYTES16 as Hex, VOTE_YES)
		const [proposalId, voteOption] = decodeAbiParameters(
			[
				{name: "proposalId", type: "bytes16"},
				{name: "voteOption", type: "uint8"},
			],
			data,
		)
		// bytes16 is left-aligned in its 32-byte word; viem returns it padded to bytes32.
		expect((proposalId as string).slice(0, 34)).toBe(PROPOSAL_BYTES16)
		expect(voteOption).toBe(VOTE_YES)
	})

	test("uses VOTE_YES === 1 for a YES vote", () => {
		// Guards the documented enum discrepancy: a YES vote must encode uint8 1, not 2.
		expect(VOTE_YES).toBe(1)
		const data = encodeVoteData(PROPOSAL_BYTES16 as Hex, VOTE_YES)
		const [, voteOption] = decodeAbiParameters([{type: "bytes16"}, {type: "uint8"}], data)
		expect(voteOption).toBe(1)
	})

	test("two words: bytes16 proposalId then uint8 voteOption (0x + 128 hex chars)", () => {
		const data = encodeVoteData(PROPOSAL_BYTES16 as Hex, VOTE_YES)
		expect(data.length).toBe(2 + 128)
	})
})

// ---------------------------------------------------------------------------
// castMembershipVote — enter() calldata
// ---------------------------------------------------------------------------

describe("castMembershipVote calldata", () => {
	test("builds enter() with the PROPOSAL_VOTED action, bytes32(proposalId) topic, and YES vote data", async () => {
		let captured: {data: Hex; to: Address} | undefined
		const wallet = fakeWallet({
			sendTransaction: async (args) => {
				captured = args
				return "0xtxhash"
			},
		})

		const hash = await Effect.runPromise(castMembershipVote(wallet, REQUEST, BOT_SPACE_ID, REGISTRY))
		expect(hash).toBe("0xtxhash")
		if (!captured) throw new Error("sendTransaction was never called")
		expect(captured.to).toBe(REGISTRY)

		const decoded = decodeFunctionData({abi: SpaceRegistryAbi, data: captured.data})
		expect(decoded.functionName).toBe("enter")

		const [fromSpace, toSpace, action, topic, voteData, signature] = decoded.args as readonly [
			Hex,
			Hex,
			Hex,
			Hex,
			Hex,
			Hex,
		]
		expect(fromSpace.toLowerCase()).toBe(BOT_SPACE_ID.toLowerCase())
		expect(toSpace.toLowerCase()).toBe(SPACE_BYTES16)
		expect(action.toLowerCase()).toBe(PROPOSAL_VOTED_ACTION)
		// Topic = bytes32(proposalId), left-aligned (proposalId in the high 16 bytes).
		expect(topic.toLowerCase()).toBe(`${PROPOSAL_BYTES16}${"0".repeat(32)}`)
		expect(signature).toBe("0x")

		// Vote data decodes to (proposalId, Yes=1).
		const [encodedId, encodedVote] = decodeAbiParameters([{type: "bytes16"}, {type: "uint8"}], voteData)
		expect((encodedId as string).slice(0, 34)).toBe(PROPOSAL_BYTES16)
		expect(encodedVote).toBe(1)
	})
})

// ---------------------------------------------------------------------------
// castMembershipVote — error classification
// ---------------------------------------------------------------------------

describe("castMembershipVote error classification", () => {
	test("classifies an on-chain revert as RevertError (CanNotVote ⇒ not expected)", async () => {
		const revert = new Error("execution reverted: CanNotVote")
		revert.name = "ContractFunctionRevertedError"
		const wallet = fakeWallet({
			sendTransaction: async () => {
				throw revert
			},
		})

		const failure = await Effect.runPromise(
			Effect.flip(castMembershipVote(wallet, REQUEST, BOT_SPACE_ID, REGISTRY)),
		)
		expect(failure._tag).toBe("RevertError")
		expect((failure as {proposalId: string}).proposalId).toBe(REQUEST.id)
		// CanNotVote (bot lacks EDITOR) is a real misconfiguration, not an expected race.
		expect((failure as {expected: boolean}).expected).toBe(false)
	})

	test("marks an already-executed revert as expected (request resolved elsewhere)", async () => {
		const revert = new Error("execution reverted: proposal already executed")
		revert.name = "ContractFunctionRevertedError"
		const wallet = fakeWallet({
			sendTransaction: async () => {
				throw revert
			},
		})

		const failure = await Effect.runPromise(
			Effect.flip(castMembershipVote(wallet, REQUEST, BOT_SPACE_ID, REGISTRY)),
		)
		expect(failure._tag).toBe("RevertError")
		expect((failure as {expected: boolean}).expected).toBe(true)
	})

	test("classifies a bundler/RPC failure as InfraError", async () => {
		const wallet = fakeWallet({
			sendTransaction: async () => {
				throw new Error("connect ECONNREFUSED api.pimlico.io")
			},
		})

		const failure = await Effect.runPromise(
			Effect.flip(castMembershipVote(wallet, REQUEST, BOT_SPACE_ID, REGISTRY)),
		)
		expect(failure._tag).toBe("InfraError")
		expect((failure as {proposalId?: string}).proposalId).toBe(REQUEST.id)
	})

	test("redacts a Pimlico API key leaked into an error message", async () => {
		const wallet = fakeWallet({
			sendTransaction: async () => {
				throw new Error("request to https://api.pimlico.io/rpc?apikey=secret123 failed")
			},
		})

		const failure = await Effect.runPromise(
			Effect.flip(castMembershipVote(wallet, REQUEST, BOT_SPACE_ID, REGISTRY)),
		)
		expect((failure as {message: string}).message).toContain("apikey=<redacted>")
		expect((failure as {message: string}).message).not.toContain("secret123")
	})
})

// ---------------------------------------------------------------------------
// readProposalTally
// ---------------------------------------------------------------------------

describe("readProposalTally", () => {
	test("decodes the (executed, creator, parameters, Tally, actions) tuple into the tally", async () => {
		const wallet = fakeWallet({
			readContract: async (args) => {
				expect(args.functionName).toBe("getLatestProposalInformation")
				// proposalId passed as bytes16
				expect(args.args[0]).toBe(PROPOSAL_BYTES16)
				return [
					false, // executed
					"0xcreator", // creator
					OPEN_PARAMS, // parameters → startDate/lastDate
					{yes: 3n, no: 1n, abstain: 2n}, // tally
					[], // actions
				]
			},
		})

		const tally = await Effect.runPromise(readProposalTally(wallet, DAO_SPACE_ADDR, PROPOSAL_UUID))
		expect(tally).toEqual({
			executed: false,
			yes: 3n,
			no: 1n,
			abstain: 2n,
			startDate: START_DATE,
			lastDate: LAST_DATE,
		})
	})

	test("surfaces a read failure as an InfraError carrying the proposalId", async () => {
		const wallet = fakeWallet({
			readContract: async () => {
				throw new Error("RPC timeout")
			},
		})

		const failure = await Effect.runPromise(Effect.flip(readProposalTally(wallet, DAO_SPACE_ADDR, PROPOSAL_UUID)))
		expect(failure._tag).toBe("InfraError")
		expect((failure as {proposalId?: string}).proposalId).toBe(PROPOSAL_UUID)
	})
})

// ---------------------------------------------------------------------------
// isEligibleToVote — two-stage eligibility (stage 2)
// ---------------------------------------------------------------------------

describe("isEligibleToVote", () => {
	const fresh: ProposalTally = {
		executed: false,
		yes: 0n,
		no: 0n,
		abstain: 0n,
		startDate: START_DATE,
		lastDate: LAST_DATE,
	}

	test("eligible iff !executed && zero tally && voting window open", () => {
		expect(isEligibleToVote(fresh, NOW)).toBe(true)
	})

	test("ineligible when already executed", () => {
		expect(isEligibleToVote({...fresh, executed: true}, NOW)).toBe(false)
	})

	test("ineligible when any vote is recorded (yes / no / abstain)", () => {
		expect(isEligibleToVote({...fresh, yes: 1n}, NOW)).toBe(false)
		expect(isEligibleToVote({...fresh, no: 1n}, NOW)).toBe(false)
		expect(isEligibleToVote({...fresh, abstain: 1n}, NOW)).toBe(false)
	})

	test("ineligible once the voting window has closed (beyond the skew buffer)", () => {
		// 61s past lastDate: outside the 60s lenient buffer ⇒ no vote.
		expect(isEligibleToVote(fresh, LAST_DATE + 61n)).toBe(false)
	})

	test("still eligible within the lenient skew buffer past lastDate", () => {
		// 30s past close but inside the 60s buffer: prefer voting (may revert) over skipping.
		expect(isEligibleToVote(fresh, LAST_DATE + 30n)).toBe(true)
	})

	test("ineligible before the voting window opens", () => {
		expect(isEligibleToVote(fresh, START_DATE - 1n)).toBe(false)
	})
})

// ---------------------------------------------------------------------------
// isVotingOpen — voting-window predicate
// ---------------------------------------------------------------------------

describe("isVotingOpen", () => {
	test("open in the middle of the window", () => {
		expect(isVotingOpen(NOW, START_DATE, LAST_DATE)).toBe(true)
	})

	test("open at exactly startDate, closed just before it", () => {
		expect(isVotingOpen(START_DATE, START_DATE, LAST_DATE)).toBe(true)
		expect(isVotingOpen(START_DATE - 1n, START_DATE, LAST_DATE)).toBe(false)
	})

	test("the buffer extends the close, never shrinks it", () => {
		// Last second of the real window is always open.
		expect(isVotingOpen(LAST_DATE - 1n, START_DATE, LAST_DATE)).toBe(true)
		// And the window stays open through skewSeconds past lastDate. The upper
		// bound is inclusive, matching the inclusive stage-1 detection SQL, so the
		// two stages agree exactly at the skew boundary (lastDate + skew).
		expect(isVotingOpen(LAST_DATE + 59n, START_DATE, LAST_DATE, 60n)).toBe(true)
		expect(isVotingOpen(LAST_DATE + 60n, START_DATE, LAST_DATE, 60n)).toBe(true)
		expect(isVotingOpen(LAST_DATE + 61n, START_DATE, LAST_DATE, 60n)).toBe(false)
	})
})

// ---------------------------------------------------------------------------
// readChainTimeSeconds — chain clock with wall-clock fallback
// ---------------------------------------------------------------------------

describe("readChainTimeSeconds", () => {
	test("returns the latest block timestamp", async () => {
		const wallet = fakeWallet({getBlock: async () => ({timestamp: 1_234_567n})})
		const now = await Effect.runPromise(readChainTimeSeconds(wallet))
		expect(now).toBe(1_234_567n)
	})

	test("falls back to the pod wall clock when the block read fails", async () => {
		const wallet = fakeWallet({
			getBlock: async () => {
				throw new Error("RPC down")
			},
		})
		const before = BigInt(Math.floor(Date.now() / 1000))
		const now = await Effect.runPromise(readChainTimeSeconds(wallet))
		expect(now).toBeGreaterThanOrEqual(before)
	})
})

// ---------------------------------------------------------------------------
// Idempotency (US2) — a non-zero on-chain tally ⇒ ineligible ⇒ no vote cast
// ---------------------------------------------------------------------------

describe("idempotency: the bot never double-votes", () => {
	test("the bot's own prior YES vote makes the request ineligible (stage-2 tally non-zero)", async () => {
		// After the bot votes once, yes >= 1 is visible on-chain immediately — even before
		// the indexer surfaces it — so the next cycle reads it and must skip.
		const wallet = fakeWallet({
			readContract: async () => [false, "0xcreator", OPEN_PARAMS, {yes: 1n, no: 0n, abstain: 0n}, []],
		})

		const tally = await Effect.runPromise(readProposalTally(wallet, DAO_SPACE_ADDR, PROPOSAL_UUID))
		expect(isEligibleToVote(tally, NOW)).toBe(false)
	})

	test("a human's pre-existing vote (indexing-lag window) also blocks the cast", async () => {
		const wallet = fakeWallet({
			readContract: async () => [false, "0xcreator", OPEN_PARAMS, {yes: 0n, no: 2n, abstain: 0n}, []],
		})

		const tally = await Effect.runPromise(readProposalTally(wallet, DAO_SPACE_ADDR, PROPOSAL_UUID))
		expect(isEligibleToVote(tally, NOW)).toBe(false)
	})
})

// ---------------------------------------------------------------------------
// resolveDaoSpaceAddress
// ---------------------------------------------------------------------------

describe("resolveDaoSpaceAddress", () => {
	test("returns the DAOSpace address from spaceIdToAddress", async () => {
		const wallet = fakeWallet({
			readContract: async (args) => {
				expect(args.functionName).toBe("spaceIdToAddress")
				return DAO_SPACE_ADDR
			},
		})

		const addr = await Effect.runPromise(resolveDaoSpaceAddress(wallet, REGISTRY, SPACE_BYTES16 as Hex))
		expect(addr).toBe(DAO_SPACE_ADDR)
	})

	test("rejects an unregistered space (zero address) as an InfraError", async () => {
		const wallet = fakeWallet({
			readContract: async () => "0x0000000000000000000000000000000000000000",
		})

		const failure = await Effect.runPromise(
			Effect.flip(resolveDaoSpaceAddress(wallet, REGISTRY, SPACE_BYTES16 as Hex)),
		)
		expect(failure._tag).toBe("InfraError")
		expect((failure as {message: string}).message).toContain("not registered")
	})
})
