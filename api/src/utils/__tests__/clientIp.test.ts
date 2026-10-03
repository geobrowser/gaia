import {describe, expect, it} from "vitest"
import {extractClientIp} from "../clientIp"

describe("extractClientIp", () => {
	function h(entries: Record<string, string>): Headers {
		return new Headers(entries)
	}

	it("returns the rightmost X-Forwarded-For entry, the one the Gateway appended", () => {
		expect(extractClientIp(h({"x-forwarded-for": "1.2.3.4, 5.6.7.8, 203.0.113.5"}))).toBe("203.0.113.5")
	})

	it("ignores X-Real-IP, which the Gateway passes through unchanged", () => {
		expect(extractClientIp(h({"x-real-ip": "198.51.100.1"}))).toBeNull()
		expect(extractClientIp(h({"x-real-ip": "198.51.100.1", "x-forwarded-for": "203.0.113.5"}))).toBe("203.0.113.5")
	})

	it("ignores spoofed leftmost X-Forwarded-For entries", () => {
		expect(extractClientIp(h({"x-forwarded-for": "198.51.100.1, 203.0.113.5"}))).toBe("203.0.113.5")
	})

	it("returns null when X-Forwarded-For is absent (in-cluster callers)", () => {
		expect(extractClientIp(h({}))).toBeNull()
	})

	it("returns null when X-Forwarded-For is empty / whitespace-only", () => {
		expect(extractClientIp(h({"x-forwarded-for": ""}))).toBeNull()
		expect(extractClientIp(h({"x-forwarded-for": ",  ,"}))).toBeNull()
	})

	it("trims whitespace", () => {
		expect(extractClientIp(h({"x-forwarded-for": "1.2.3.4,   203.0.113.5  "}))).toBe("203.0.113.5")
	})

	it("handles single-entry X-Forwarded-For", () => {
		expect(extractClientIp(h({"x-forwarded-for": "203.0.113.5"}))).toBe("203.0.113.5")
	})
})
