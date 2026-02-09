import {describe, expect, it} from "vitest"
import {decodeBase58, encodeBase58, isBase58} from "./base58"

describe("encodeBase58", () => {
	it("encodes UUIDs matching Rust test vectors", () => {
		// From indexer_utils/src/id.rs test_base58_encoding
		expect(encodeBase58("1cc6995f6cc24c7a95921466bf95f6be")).toBe("4Z6VLmpipszCVZb21Fey5F")
		// From indexer_utils/src/id.rs test_base58_encoding_2
		expect(encodeBase58("08c4f09378584b7c9b94b82e448abcff")).toBe("25omwWh6HYgeRQKCaSpVpa")
	})

	it("encodes known UUIDs from Sentry trace", () => {
		// BDuZwkjCg3nPWMDshoYtpS was seen in trace ae1434810a6340c89edd6ef5d14f88c4
		expect(encodeBase58("52c8b540e2494e1bbabcc8eac0acaa23")).toBe("BDuZwkjCg3nPWMDshoYtpS")
	})

	it("returns empty string for zero UUID", () => {
		expect(encodeBase58("00000000000000000000000000000000")).toBe("")
	})

	it("encodes single-digit values", () => {
		expect(encodeBase58("00000000000000000000000000000001")).toBe("2")
	})

	it("encodes max UUID", () => {
		const result = encodeBase58("ffffffffffffffffffffffffffffffff")
		expect(result.length).toBeLessThanOrEqual(22)
		expect(result.length).toBeGreaterThan(0)
	})

	it("rejects non-32-char input", () => {
		expect(() => encodeBase58("abc")).toThrow("expected 32-char lowercase hex")
		expect(() => encodeBase58("")).toThrow("expected 32-char lowercase hex")
		expect(() => encodeBase58("550e8400-e29b-41d4-a716-446655440000")).toThrow("expected 32-char lowercase hex")
	})

	it("rejects uppercase hex input", () => {
		expect(() => encodeBase58("550E8400E29B41D4A716446655440000")).toThrow("expected 32-char lowercase hex")
	})
})

describe("decodeBase58", () => {
	it("decodes Base58 matching Rust test vectors", () => {
		// From indexer_utils/src/id.rs test_base58_decoding
		expect(decodeBase58("4Z6VLmpipszCVZb21Fey5F")).toBe("1cc6995f6cc24c7a95921466bf95f6be")
	})

	it("decodes known IDs from Sentry trace", () => {
		expect(decodeBase58("BDuZwkjCg3nPWMDshoYtpS")).toBe("52c8b540e2494e1bbabcc8eac0acaa23")
	})

	it("decodes single character", () => {
		expect(decodeBase58("2")).toBe("00000000000000000000000000000001")
	})

	it("rejects empty string", () => {
		expect(() => decodeBase58("")).toThrow("empty string")
	})

	it("rejects invalid characters", () => {
		expect(() => decodeBase58("0invalid")).toThrow("Invalid Base58 character: 0")
		expect(() => decodeBase58("OOOO")).toThrow("Invalid Base58 character: O")
		expect(() => decodeBase58("IIll")).toThrow("Invalid Base58 character: I")
	})

	it("rejects values exceeding 128-bit range", () => {
		// 23 chars of 'z' (highest digit) will overflow u128
		expect(() => decodeBase58("zzzzzzzzzzzzzzzzzzzzzzz")).toThrow("exceeds 128-bit")
	})
})

describe("roundtrip", () => {
	it("encode → decode produces original UUID", () => {
		const uuids = [
			"1cc6995f6cc24c7a95921466bf95f6be",
			"08c4f09378584b7c9b94b82e448abcff",
			"52c8b540e2494e1bbabcc8eac0acaa23",
			"d00075188e424acb8cffe59cda88c104",
			"da2e8dcfd0ec4484a751f00226217799",
			"61681e0a8dd74b7ea82ed5fb53cdd16c",
			"ffffffffffffffffffffffffffffffff",
			"00000000000000000000000000000001",
		]
		for (const uuid of uuids) {
			const encoded = encodeBase58(uuid)
			const decoded = decodeBase58(encoded)
			expect(decoded).toBe(uuid)
		}
	})

	it("zero UUID does not roundtrip (Rust parity — encode returns empty, decode rejects empty)", () => {
		const zeroHex = "00000000000000000000000000000000"
		const encoded = encodeBase58(zeroHex)
		expect(encoded).toBe("")
		expect(() => decodeBase58("")).toThrow("empty string")
	})
})

describe("isBase58", () => {
	it("accepts valid Base58 strings", () => {
		expect(isBase58("4Z6VLmpipszCVZb21Fey5F")).toBe(true)
		expect(isBase58("2")).toBe(true)
		expect(isBase58("BDuZwkjCg3nPWMDshoYtpS")).toBe(true)
	})

	it("rejects empty string", () => {
		expect(isBase58("")).toBe(false)
	})

	it("rejects strings with invalid characters", () => {
		expect(isBase58("0invalid")).toBe(false)
		expect(isBase58("OOOO")).toBe(false)
		expect(isBase58("test with spaces")).toBe(false)
	})

	it("rejects strings longer than 22 chars", () => {
		expect(isBase58("12345678901234567890123")).toBe(false)
	})

	it("does not match hex UUIDs containing 0", () => {
		// Dashless hex UUIDs almost always contain '0' which is not in Base58
		expect(isBase58("550e8400e29b41d4a716446655440000")).toBe(false)
	})
})
