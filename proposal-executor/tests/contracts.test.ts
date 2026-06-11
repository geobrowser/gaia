/**
 * Tests for contracts.ts — membership vote constants and the DAOSpace /
 * SpaceRegistry ABI view subsets used by the membership-accept path.
 *
 * These pin the on-chain encoding (VoteOption.Yes = 1) and the action hash
 * against the authoritative source (geo-contracts-foundry IDAOSpace), and
 * verify the ABI subsets expose the view names the membership path calls.
 */

import {describe, expect, test} from "bun:test"
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
