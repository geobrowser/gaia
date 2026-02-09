import {describe, expect, it} from "vitest"
import {type Uuid, isValidUuid, toBase58, toUuid} from "./uuid"

// =============================================================================
// toUuid — format detection and normalization
// =============================================================================

describe("toUuid", () => {
	describe("dashed hex input", () => {
		it("accepts lowercase dashed hex", () => {
			expect(toUuid("550e8400-e29b-41d4-a716-446655440000")).toBe("550e8400-e29b-41d4-a716-446655440000")
		})

		it("lowercases uppercase dashed hex", () => {
			expect(toUuid("550E8400-E29B-41D4-A716-446655440000")).toBe("550e8400-e29b-41d4-a716-446655440000")
		})

		it("trims whitespace", () => {
			expect(toUuid("  550e8400-e29b-41d4-a716-446655440000  ")).toBe("550e8400-e29b-41d4-a716-446655440000")
		})
	})

	describe("dashless hex input", () => {
		it("inserts dashes into dashless hex", () => {
			expect(toUuid("550e8400e29b41d4a716446655440000")).toBe("550e8400-e29b-41d4-a716-446655440000")
		})

		it("lowercases uppercase dashless hex", () => {
			expect(toUuid("550E8400E29B41D4A716446655440000")).toBe("550e8400-e29b-41d4-a716-446655440000")
		})
	})

	describe("Base58 input", () => {
		it("decodes Base58 to dashed hex", () => {
			// "BDuZwkjCg3nPWMDshoYtpS" = 52c8b540-e249-4e1b-babc-c8eac0acaa23
			expect(toUuid("BDuZwkjCg3nPWMDshoYtpS")).toBe("52c8b540-e249-4e1b-babc-c8eac0acaa23")
		})

		it("decodes single-char Base58", () => {
			// "2" = index 0 in Base58 alphabet = value 1
			expect(toUuid("2")).toBe("00000000-0000-0000-0000-000000000001")
		})

		it("decodes Rust test vector", () => {
			// "4Z6VLmpipszCVZb21Fey5F" = 1cc6995f-6cc2-4c7a-9592-1466bf95f6be
			expect(toUuid("4Z6VLmpipszCVZb21Fey5F")).toBe("1cc6995f-6cc2-4c7a-9592-1466bf95f6be")
		})
	})

	describe("disambiguation — hex wins over Base58", () => {
		it("treats 32-char hex-only string as dashless hex, not Base58", () => {
			// "abcdefabcdefabcdefabcdefabcdefab" — all chars valid in both hex and Base58
			// But it's 32 chars, so hex wins.
			const result = toUuid("abcdefabcdefabcdefabcdefabcdefab")
			expect(result).toBe("abcdefab-cdef-abcd-efab-cdefabcdefab")
		})

		it("treats 36-char dashed hex as dashed, not anything else", () => {
			const result = toUuid("abcdefab-cdef-abcd-efab-cdefabcdefab")
			expect(result).toBe("abcdefab-cdef-abcd-efab-cdefabcdefab")
		})
	})

	describe("invalid input", () => {
		it("throws on empty string", () => {
			expect(() => toUuid("")).toThrow()
		})

		it("throws on random text", () => {
			expect(() => toUuid("not-a-uuid")).toThrow()
		})

		it("throws on too-short hex", () => {
			expect(() => toUuid("550e8400")).toThrow()
		})

		it("surfaces overflow error for Base58 exceeding 128 bits", () => {
			// 22 chars of 'z' — isBase58 accepts it, but decodeBase58 detects overflow
			expect(() => toUuid("zzzzzzzzzzzzzzzzzzzzzz")).toThrow("exceeds 128-bit")
		})
	})
})

// =============================================================================
// isValidUuid
// =============================================================================

describe("isValidUuid", () => {
	it("accepts all three formats", () => {
		expect(isValidUuid("550e8400-e29b-41d4-a716-446655440000")).toBe(true)
		expect(isValidUuid("550e8400e29b41d4a716446655440000")).toBe(true)
		expect(isValidUuid("BDuZwkjCg3nPWMDshoYtpS")).toBe(true)
	})

	it("rejects invalid strings", () => {
		expect(isValidUuid("")).toBe(false)
		expect(isValidUuid("not-a-uuid")).toBe(false)
		expect(isValidUuid("zzzzzzzzzzzzzzzzzzzzzz")).toBe(false) // overflow
	})
})

// =============================================================================
// toBase58
// =============================================================================

describe("toBase58", () => {
	it("encodes a known UUID to Base58", () => {
		const uuid = toUuid("52c8b540-e249-4e1b-babc-c8eac0acaa23")
		expect(toBase58(uuid)).toBe("BDuZwkjCg3nPWMDshoYtpS")
	})

	it("roundtrips through toUuid → toBase58 → toUuid", () => {
		const original = "1cc6995f-6cc2-4c7a-9592-1466bf95f6be"
		const uuid = toUuid(original)
		const base58 = toBase58(uuid)
		const back = toUuid(base58)
		expect(back).toBe(original)
	})

	it("returns empty string for zero UUID (Rust parity)", () => {
		const uuid = toUuid("00000000-0000-0000-0000-000000000000")
		expect(toBase58(uuid)).toBe("")
	})

	it("throws on invalid branded string (runtime guard)", () => {
		// Simulate a bad cast — someone passes a raw string as Uuid
		const fake = "not-a-real-uuid" as Uuid
		expect(() => toBase58(fake)).toThrow("expected 36-char dashed UUID")
	})
})
