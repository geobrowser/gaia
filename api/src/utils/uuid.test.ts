import {describe, expect, it} from "vitest"
import {fromBase58, isValidBase58Id, isValidUuid, toBase58, toUuid, type Uuid, uuidToBase58} from "./uuid"

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

// =============================================================================
// uuidToBase58
// =============================================================================

describe("uuidToBase58", () => {
	it("encodes dashed hex to Base58", () => {
		expect(uuidToBase58("52c8b540-e249-4e1b-babc-c8eac0acaa23")).toBe("BDuZwkjCg3nPWMDshoYtpS")
	})

	it("encodes dashless hex to Base58", () => {
		expect(uuidToBase58("52c8b540e2494e1bbabcc8eac0acaa23")).toBe("BDuZwkjCg3nPWMDshoYtpS")
	})

	it("encodes Base58 to Base58 (idempotent roundtrip)", () => {
		expect(uuidToBase58("BDuZwkjCg3nPWMDshoYtpS")).toBe("BDuZwkjCg3nPWMDshoYtpS")
	})

	it("throws on invalid input", () => {
		expect(() => uuidToBase58("not-valid")).toThrow()
	})
})

// =============================================================================
// fromBase58 — Base58-only input parsing
// =============================================================================

describe("fromBase58", () => {
	it("decodes Base58 to dashed hex", () => {
		expect(fromBase58("BDuZwkjCg3nPWMDshoYtpS")).toBe("52c8b540-e249-4e1b-babc-c8eac0acaa23")
	})

	it("decodes single-char Base58", () => {
		expect(fromBase58("2")).toBe("00000000-0000-0000-0000-000000000001")
	})

	it("trims whitespace", () => {
		expect(fromBase58("  BDuZwkjCg3nPWMDshoYtpS  ")).toBe("52c8b540-e249-4e1b-babc-c8eac0acaa23")
	})

	it("rejects dashed hex UUID", () => {
		expect(() => fromBase58("550e8400-e29b-41d4-a716-446655440000")).toThrow("Invalid Base58")
	})

	it("rejects dashless hex UUID", () => {
		expect(() => fromBase58("550e8400e29b41d4a716446655440000")).toThrow("Invalid Base58")
	})

	it("rejects empty string", () => {
		expect(() => fromBase58("")).toThrow("Invalid Base58")
	})

	it("rejects random text", () => {
		expect(() => fromBase58("not-a-valid-id")).toThrow("Invalid Base58")
	})

	it("surfaces overflow error for Base58 exceeding 128 bits", () => {
		expect(() => fromBase58("zzzzzzzzzzzzzzzzzzzzzz")).toThrow("exceeds 128-bit")
	})
})

// =============================================================================
// isValidBase58Id
// =============================================================================

describe("isValidBase58Id", () => {
	it("accepts valid Base58", () => {
		expect(isValidBase58Id("BDuZwkjCg3nPWMDshoYtpS")).toBe(true)
		expect(isValidBase58Id("2")).toBe(true)
		expect(isValidBase58Id("4Z6VLmpipszCVZb21Fey5F")).toBe(true)
	})

	it("rejects dashed hex", () => {
		expect(isValidBase58Id("550e8400-e29b-41d4-a716-446655440000")).toBe(false)
	})

	it("rejects dashless hex", () => {
		expect(isValidBase58Id("550e8400e29b41d4a716446655440000")).toBe(false)
	})

	it("rejects empty string", () => {
		expect(isValidBase58Id("")).toBe(false)
	})

	it("rejects overflow Base58", () => {
		expect(isValidBase58Id("zzzzzzzzzzzzzzzzzzzzzz")).toBe(false)
	})
})

// =============================================================================
// toUuid error messages
// =============================================================================

describe("toUuid error messages", () => {
	it("includes input length in error message", () => {
		// Use a string with characters outside Base58 so it doesn't get decoded
		expect(() => toUuid("this has spaces and is invalid")).toThrow("length=30")
	})

	it("does not echo raw input in error message", () => {
		const malicious = "<script>alert('xss')</script>"
		expect(() => toUuid(malicious)).toThrow("Invalid UUID format")
		try {
			toUuid(malicious)
		} catch (e) {
			// Error should contain length but not the raw input
			expect((e as Error).message).toContain("length=")
			expect((e as Error).message).not.toContain("<script>")
		}
	})
})
