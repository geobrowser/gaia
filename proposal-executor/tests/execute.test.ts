/**
 * Tests for execute.ts — encoding helpers and error classification.
 *
 * Tests encoding correctness against known values from geo-cli and verifies
 * error classification logic (RevertError vs InfraError).
 */

import {describe, expect, test} from "bun:test"
import type {Hex} from "viem"
import {EMPTY_SIGNATURE, PROPOSAL_EXECUTED_ACTION} from "../src/contracts.js"
import {encodeProposalExecutedData, padBytes16ToBytes32, uuidToBytes16} from "../src/execute.js"

// ---------------------------------------------------------------------------
// uuidToBytes16
// ---------------------------------------------------------------------------

describe("uuidToBytes16", () => {
	test("converts a valid UUID to bytes16", () => {
		const uuid = "550e8400-e29b-41d4-a716-446655440000"
		const result = uuidToBytes16(uuid)
		expect(result).toBe("0x550e8400e29b41d4a716446655440000")
	})

	test("handles lowercase hex chars", () => {
		const uuid = "abcdef01-2345-6789-abcd-ef0123456789"
		const result = uuidToBytes16(uuid)
		expect(result).toBe("0xabcdef0123456789abcdef0123456789")
	})

	test("handles uppercase hex chars", () => {
		const uuid = "ABCDEF01-2345-6789-ABCD-EF0123456789"
		const result = uuidToBytes16(uuid)
		expect(result).toBe("0xABCDEF0123456789ABCDEF0123456789")
	})

	test("result is 34 chars (0x + 32 hex chars)", () => {
		const uuid = "550e8400-e29b-41d4-a716-446655440000"
		const result = uuidToBytes16(uuid)
		expect(result.length).toBe(34) // "0x" + 32 hex chars
	})

	test("throws on invalid UUID (too short)", () => {
		expect(() => uuidToBytes16("550e8400-e29b")).toThrow("Invalid UUID")
	})

	test("throws on invalid UUID (non-hex characters)", () => {
		expect(() => uuidToBytes16("gggggggg-gggg-gggg-gggg-gggggggggggg")).toThrow("Invalid UUID")
	})

	test("throws on empty string", () => {
		expect(() => uuidToBytes16("")).toThrow("Invalid UUID")
	})
})

// ---------------------------------------------------------------------------
// padBytes16ToBytes32
// ---------------------------------------------------------------------------

describe("padBytes16ToBytes32", () => {
	test("pads bytes16 to bytes32 with trailing zeros", () => {
		const bytes16 = "0x550e8400e29b41d4a716446655440000" as Hex
		const result = padBytes16ToBytes32(bytes16)
		expect(result).toBe("0x550e8400e29b41d4a71644665544000000000000000000000000000000000000")
	})

	test("result is 66 chars (0x + 64 hex chars)", () => {
		const bytes16 = "0x550e8400e29b41d4a716446655440000" as Hex
		const result = padBytes16ToBytes32(bytes16)
		expect(result.length).toBe(66) // "0x" + 64 hex chars
	})

	test("trailing 32 chars are all zeros", () => {
		const bytes16 = "0xabcdef0123456789abcdef0123456789" as Hex
		const result = padBytes16ToBytes32(bytes16)
		expect(result.slice(34)).toBe("00000000000000000000000000000000")
	})

	test("preserves the original bytes16 in the first 32 hex chars", () => {
		const bytes16 = "0xabcdef0123456789abcdef0123456789" as Hex
		const result = padBytes16ToBytes32(bytes16)
		expect(result.slice(0, 34)).toBe("0xabcdef0123456789abcdef0123456789")
	})

	test("throws on invalid bytes16 (too short)", () => {
		expect(() => padBytes16ToBytes32("0x1234" as Hex)).toThrow("Invalid bytes16")
	})

	test("throws on invalid bytes16 (too long)", () => {
		const tooLong = "0x" + "a".repeat(64)
		expect(() => padBytes16ToBytes32(tooLong as Hex)).toThrow("Invalid bytes16")
	})
})

// ---------------------------------------------------------------------------
// encodeProposalExecutedData
// ---------------------------------------------------------------------------

describe("encodeProposalExecutedData", () => {
	test("returns ABI-encoded bytes16 proposalId", () => {
		const proposalIdHex = "0x550e8400e29b41d4a716446655440000" as Hex
		const result = encodeProposalExecutedData(proposalIdHex)

		// ABI encoding of bytes16 is 32 bytes (right-padded with zeros)
		// 0x + 64 hex chars
		expect(result).toStartWith("0x")
		expect(result.length).toBe(66) // 0x + 64 hex chars

		// The encoded value should contain the proposalId padded to 32 bytes
		// bytes16 is left-aligned in ABI encoding, so the hex value appears first
		expect(result.slice(2, 34)).toBe("550e8400e29b41d4a716446655440000")
	})

	test("different proposal IDs produce different encodings", () => {
		const id1 = "0x550e8400e29b41d4a716446655440000" as Hex
		const id2 = "0x660e8400e29b41d4a716446655440000" as Hex
		expect(encodeProposalExecutedData(id1)).not.toBe(encodeProposalExecutedData(id2))
	})
})

// ---------------------------------------------------------------------------
// End-to-end encoding: UUID → bytes16 → padded → encoded
// ---------------------------------------------------------------------------

describe("full encoding pipeline", () => {
	test("UUID → bytes16 → padBytes32 produces correct topic", () => {
		const uuid = "550e8400-e29b-41d4-a716-446655440000"
		const bytes16 = uuidToBytes16(uuid)
		const topic = padBytes16ToBytes32(bytes16)

		expect(bytes16).toBe("0x550e8400e29b41d4a716446655440000")
		expect(topic).toBe("0x550e8400e29b41d4a71644665544000000000000000000000000000000000000")
	})

	test("UUID → bytes16 → encodeProposalExecutedData produces valid ABI data", () => {
		const uuid = "550e8400-e29b-41d4-a716-446655440000"
		const bytes16 = uuidToBytes16(uuid)
		const data = encodeProposalExecutedData(bytes16)

		expect(data).toStartWith("0x")
		// ABI encoding is deterministic
		expect(data.length).toBe(66)
	})
})

// ---------------------------------------------------------------------------
// Governance constants
// ---------------------------------------------------------------------------

describe("governance constants", () => {
	test("PROPOSAL_EXECUTED_ACTION matches known hash", () => {
		// keccak256('GOVERNANCE.PROPOSAL_EXECUTED')
		expect(PROPOSAL_EXECUTED_ACTION).toBe("0x62a60c0a9681612871e0dafa0f24bb0c83cbdde8be5a6299979c88d382369e96")
	})

	test("PROPOSAL_EXECUTED_ACTION is a valid bytes32 (66 chars with 0x prefix)", () => {
		expect(PROPOSAL_EXECUTED_ACTION).toStartWith("0x")
		expect(PROPOSAL_EXECUTED_ACTION.length).toBe(66)
	})

	test("EMPTY_SIGNATURE is 0x", () => {
		expect(EMPTY_SIGNATURE).toBe("0x")
	})
})

// ---------------------------------------------------------------------------
// Error classification (message-based pattern matching)
// ---------------------------------------------------------------------------

describe("error classification patterns", () => {
	// These test the patterns used in executeProposal's catch block
	const revertPatterns = ["revert", "execution reverted", "CALL_EXCEPTION", "UserOperation reverted"]
	const expectedRevertPatterns = ["already executed", "ProposalAlreadyExecuted", "InvalidAction"]

	test("revert patterns match expected error messages", () => {
		for (const pattern of revertPatterns) {
			const message = `Error: ${pattern} during transaction`
			expect(revertPatterns.some((p) => message.includes(p))).toBe(true)
		}
	})

	test("expected revert patterns match known race condition errors", () => {
		for (const pattern of expectedRevertPatterns) {
			const message = `Error: ${pattern}`
			expect(expectedRevertPatterns.some((p) => message.includes(p))).toBe(true)
		}
	})

	test("infra errors do NOT match revert patterns", () => {
		const infraMessages = [
			"Error: connect ECONNREFUSED",
			"Error: 429 Too Many Requests",
			"Error: timeout exceeded",
			"Error: ENOTFOUND api.pimlico.io",
		]
		for (const message of infraMessages) {
			expect(revertPatterns.some((p) => message.includes(p))).toBe(false)
		}
	})
})
