/**
 * Tests for contracts.ts — membership vote constants and the DAOSpace /
 * SpaceRegistry ABI view subsets used by the membership-accept path.
 *
 * These pin the on-chain encoding (VoteOption.Yes = 1) and the action hash
 * against the authoritative source (geo-contracts-foundry IDAOSpace), and
 * verify the ABI subsets expose the view names the membership path calls.
 */

import {describe, expect, test} from "bun:test"
import {decodeFunctionResult} from "viem"
import {DAOSpaceAbi, PROPOSAL_VOTED_ACTION, SpaceRegistryAbi, VOTE_YES} from "../src/contracts.js"

// ---------------------------------------------------------------------------
// VOTE_YES — guards the documented enum discrepancy
// ---------------------------------------------------------------------------

describe("VOTE_YES constant", () => {
	test("is 1 (IDAOSpace.VoteOption.Yes: None=0, Yes=1, No=2, Abstain=3)", () => {
		// Source of truth: geo-contracts-foundry/src/interfaces/IDAOSpace.sol
		expect(VOTE_YES).toBe(1)
	})
})

// ---------------------------------------------------------------------------
// PROPOSAL_VOTED_ACTION — keccak256('GOVERNANCE.PROPOSAL_VOTED')
// ---------------------------------------------------------------------------

describe("PROPOSAL_VOTED_ACTION constant", () => {
	test("matches the action hash from hermes-substream", () => {
		expect(PROPOSAL_VOTED_ACTION).toBe("0x4ebf5f29676cedf7e2e4d346a8433289278f95a9fda73691dc1ce24574d5819e")
	})

	test("is a 32-byte hex string (0x + 64 hex chars)", () => {
		expect(PROPOSAL_VOTED_ACTION.length).toBe(66)
		expect(PROPOSAL_VOTED_ACTION).toMatch(/^0x[0-9a-f]{64}$/)
	})
})

// ---------------------------------------------------------------------------
// DAOSpaceAbi — getLatestProposalInformation view (stage-2 tally read)
// ---------------------------------------------------------------------------

describe("DAOSpaceAbi", () => {
	const getInfo = DAOSpaceAbi.find(
		(entry) => entry.type === "function" && entry.name === "getLatestProposalInformation",
	)

	test("exposes getLatestProposalInformation", () => {
		expect(getInfo).toBeDefined()
	})

	test("takes a single bytes16 proposalId input", () => {
		expect(getInfo?.inputs).toHaveLength(1)
		expect(getInfo?.inputs[0]?.type).toBe("bytes16")
	})

	test("returns the (executed, creator, parameters, tally, actions) tuple", () => {
		const outputs = getInfo?.outputs ?? []
		expect(outputs.map((o) => o.type)).toEqual(["bool", "bytes16", "tuple", "tuple", "tuple[]"])
	})

	test("decodes the Tally tuple as (yes, no, abstain) uint256 components", () => {
		const tally = getInfo?.outputs?.[3]
		expect(tally?.type).toBe("tuple")
		expect(tally?.components?.map((c) => c.name)).toEqual(["yes", "no", "abstain"])
		expect(tally?.components?.every((c) => c.type === "uint256")).toBe(true)
	})

	test("ProposalParameters matches the deployed v2 8-field struct", () => {
		const params = getInfo?.outputs?.[2]
		expect(params?.type).toBe("tuple")
		// Authoritative source: geo-contracts-foundry IDAOSpace.ProposalParameters.
		// The v2 struct has exactly 8 fields; an over- or under-count silently misaligns
		// the startDate/lastDate slot reads in readProposalTally (overflow / garbage),
		// which is what the on-chain decode regression below guards against end-to-end.
		// The 5-field v1 shape this used to declare belongs to chain 19411, which is
		// being retired; on 55516 it made startDate read flatSupportThreshold.
		expect(params?.components?.map((c) => c.name)).toEqual([
			"votingMode",
			"partialPercentageSupportThreshold",
			"universalPercentageSupportThreshold",
			"flatSupportThreshold",
			"quorum",
			"startDate",
			"lastDate",
			"executeBy",
		])
		expect(params?.components?.map((c) => c.type)).toEqual([
			"uint8",
			"uint256",
			"uint256",
			"uint256",
			"uint256",
			"uint256",
			"uint256",
			"uint256",
		])
	})

	// Regression for the ABI struct-shape bug: decode REAL on-chain return bytes through
	// DAOSpaceAbi and assert the fields land correctly. A fixture-based decode — not a
	// self-consistent round-trip — is the only thing that catches an ABI that doesn't
	// match the deployed contract.
	//
	// Captured from chain 55516 (the v2 stack behind geobrowser.io) on 2026-08-11:
	// DAOSpace 0xF3E30A62…111222, proposal c9b38b02…489101, a real member request that
	// the bot had failed to auto-accept. The previous fixture came from chain 19411,
	// whose v1 contract really does return the 5-field struct — which is exactly how an
	// ABI wrong for v2 kept a green test suite. Decoding these v2 bytes through the
	// 5-field shape slid every later slot and threw a safe-integer overflow.
	//
	// Note startDate/lastDate/executeBy are all 0 while the tally is empty: v2 snapshots
	// the voting window on the first vote, so "untouched" and "zero window" coincide.
	test("decodes a real v2 getLatestProposalInformation return with correct startDate/lastDate", () => {
		const raw = ("0x" +
			"0000000000000000000000000000000000000000000000000000000000000000" + // executed = false
			"70a0a4ddda1057868ae4feebceaaacef00000000000000000000000000000000" + // creator (bytes16) — non-zero: the DAO knows this proposal
			"0000000000000000000000000000000000000000000000000000000000000001" + // votingMode = Fast
			"00000000000000000000000000000000000000000000000000000000004dd1e0" + // partialPercentageSupportThreshold = 5100000
			"00000000000000000000000000000000000000000000000000000000004dd1e0" + // universalPercentageSupportThreshold = 5100000
			"0000000000000000000000000000000000000000000000000000000000000001" + // flatSupportThreshold = 1
			"0000000000000000000000000000000000000000000000000000000000000001" + // quorum = 1
			"0000000000000000000000000000000000000000000000000000000000000000" + // startDate = 0 (window not snapshotted)
			"0000000000000000000000000000000000000000000000000000000000000000" + // lastDate  = 0 (window not snapshotted)
			"0000000000000000000000000000000000000000000000000000000000000000" + // executeBy = 0
			"0000000000000000000000000000000000000000000000000000000000000000" + // tally.yes
			"0000000000000000000000000000000000000000000000000000000000000000" + // tally.no
			"0000000000000000000000000000000000000000000000000000000000000000" + // tally.abstain
			"00000000000000000000000000000000000000000000000000000000000001c0" + // actions offset
			"0000000000000000000000000000000000000000000000000000000000000001" + // actions.length = 1
			"0000000000000000000000000000000000000000000000000000000000000020" + // actions[0] offset
			"000000000000000000000000f3e30a621aaef2e924bbf777440999b973111222" + // actions[0].toAddress
			"0000000000000000000000000000000000000000000000000000000000000000" + // actions[0].toSpaceId
			"0000000000000000000000000000000000000000000000000000000000000000" + // actions[0].value
			"0000000000000000000000000000000000000000000000000000000000000080" + // actions[0].data offset
			"0000000000000000000000000000000000000000000000000000000000000024" + // actions[0].data length = 36
			"2afbe35070a0a4ddda1057868ae4feebceaaacef000000000000000000000000" + // actions[0].data — addMember(requester)
			"0000000000000000000000000000000000000000000000000000000000000000") as `0x${string}` // data tail pad

		const [executed, creator, parameters, tally] = decodeFunctionResult({
			abi: DAOSpaceAbi,
			functionName: "getLatestProposalInformation",
			data: raw,
		})

		expect(executed).toBe(false)
		expect(creator).toBe("0x70a0a4ddda1057868ae4feebceaaacef")
		expect(parameters.flatSupportThreshold).toBe(1n)
		expect(parameters.quorum).toBe(1n)
		expect(parameters.startDate).toBe(0n)
		expect(parameters.lastDate).toBe(0n)
		expect(tally.yes).toBe(0n)
		expect(tally.no).toBe(0n)
		expect(tally.abstain).toBe(0n)
	})
})

// ---------------------------------------------------------------------------
// SpaceRegistryAbi — spaceIdToAddress view (DAOSpace address resolution)
// ---------------------------------------------------------------------------

describe("SpaceRegistryAbi", () => {
	test("exposes spaceIdToAddress(bytes16) → address", () => {
		const fn = SpaceRegistryAbi.find((entry) => entry.type === "function" && entry.name === "spaceIdToAddress")
		expect(fn).toBeDefined()
		expect(fn?.inputs[0]?.type).toBe("bytes16")
		expect(fn?.outputs[0]?.type).toBe("address")
	})

	test("still exposes the pre-existing enter and addressToSpaceId views", () => {
		const names = SpaceRegistryAbi.map((entry) => entry.name)
		expect(names).toContain("enter")
		expect(names).toContain("addressToSpaceId")
	})
})
