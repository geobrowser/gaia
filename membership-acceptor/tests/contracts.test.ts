import {describe, expect, test} from "bun:test"

import {
	encodeVoteData,
	getChain,
	isRevert,
	padBytes16ToBytes32,
	sanitizeError,
	toBytes16Hex,
	uuidToBytes16,
	VOTE_YES,
} from "../src/contracts.js"

describe("uuidToBytes16", () => {
	test("strips dashes and 0x-prefixes", () => {
		expect(uuidToBytes16("550e8400-e29b-41d4-a716-446655440000")).toBe("0x550e8400e29b41d4a716446655440000")
	})

	test("throws on a non-UUID", () => {
		expect(() => uuidToBytes16("not-a-uuid")).toThrow()
	})
})

describe("toBytes16Hex", () => {
	test("converts a dashed UUID to 0x bytes16", () => {
		expect(toBytes16Hex("c9f267dc-b0d2-7071-8c2a-3c45a64afd32")).toBe("0xc9f267dcb0d270718c2a3c45a64afd32")
	})

	test("is idempotent on an already-0x bytes16 (lowercased)", () => {
		expect(toBytes16Hex("0xC9F267DCB0D270718C2A3C45A64AFD32")).toBe("0xc9f267dcb0d270718c2a3c45a64afd32")
	})

	test("a dashed UUID and its 0x form canonicalize equal", () => {
		expect(toBytes16Hex("c9f267dc-b0d2-7071-8c2a-3c45a64afd32")).toBe(
			toBytes16Hex("0xc9f267dcb0d270718c2a3c45a64afd32"),
		)
	})
})

describe("padBytes16ToBytes32", () => {
	test("right-pads a bytes16 to bytes32 with zeros", () => {
		expect(padBytes16ToBytes32("0x550e8400e29b41d4a716446655440000")).toBe(
			`0x550e8400e29b41d4a716446655440000${"0".repeat(32)}`,
		)
	})

	test("throws when not 32 hex chars", () => {
		expect(() => padBytes16ToBytes32("0x1234")).toThrow()
	})
})

describe("encodeVoteData", () => {
	test("encodes (bytes16 proposalId, uint8 YES) as two abi words", () => {
		const encoded = encodeVoteData("0x550e8400e29b41d4a716446655440000", VOTE_YES)
		// 2 × 32-byte words = 128 hex chars after 0x.
		expect(encoded).toHaveLength(2 + 128)
		// Word 1: bytes16 left-aligned (value then 16 zero bytes).
		expect(encoded.startsWith("0x550e8400e29b41d4a716446655440000")).toBe(true)
		// Word 2: uint8 right-aligned → all zeros except the final byte = 0x01 (YES).
		const voteWord = encoded.slice(2 + 64)
		expect(voteWord).toBe(`${"0".repeat(63)}1`)
	})
})

describe("getChain", () => {
	test("maps the supported chain ids", () => {
		expect(getChain(80451).id).toBe(80451)
		expect(getChain(19411).id).toBe(19411)
	})
})

describe("isRevert", () => {
	test("true for known revert error names", () => {
		const e = new Error("boom")
		e.name = "ContractFunctionRevertedError"
		expect(isRevert(e)).toBe(true)
	})

	test("true when the cause is a revert", () => {
		const cause = new Error("reverted")
		cause.name = "ContractFunctionRevertedError"
		expect(isRevert(new Error("wrap", {cause}))).toBe(true)
	})

	test("true on definite revert message patterns", () => {
		expect(isRevert(new Error("execution reverted: CanNotVote"))).toBe(true)
		expect(isRevert(new Error("ProposalAlreadyExecuted()"))).toBe(true)
		expect(isRevert(new Error("reverted with AlreadyVoted"))).toBe(true)
	})

	test("false for plain infrastructure errors", () => {
		expect(isRevert(new Error("ECONNREFUSED"))).toBe(false)
		expect(isRevert(new Error("fetch failed"))).toBe(false)
	})

	test("false for the broad execution/call wrappers (not necessarily reverts)", () => {
		// These wrap RPC/estimation failures too, so they must NOT be auto-benign.
		const exec = new Error("call failed")
		exec.name = "ContractFunctionExecutionError"
		expect(isRevert(exec)).toBe(false)

		const call = new Error("call failed")
		call.name = "CallExecutionError"
		expect(isRevert(call)).toBe(false)

		// A bare "UserOperation reverted" with no revert reason is ambiguous → infra.
		expect(isRevert(new Error("UserOperation reverted"))).toBe(false)
	})
})

describe("sanitizeError", () => {
	test("redacts a bundler apikey", () => {
		const msg = sanitizeError(new Error("https://api.pimlico.io/v2/80451/rpc?apikey=pim_secret failed"))
		expect(msg).toContain("apikey=<redacted>")
		expect(msg).not.toContain("pim_secret")
	})
})
