import {describe, expect, it} from "vitest"
import {extractClientIp} from "../clientIp"

describe("extractClientIp", () => {
	function h(entries: Record<string, string>): Headers {
		return new Headers(entries)
	}

	it("prefers X-Real-IP", () => {
		expect(extractClientIp(h({"x-real-ip": "203.0.113.5"}))).toBe("203.0.113.5")
	})

	it("returns rightmost X-Forwarded-For entry when X-Real-IP is absent", () => {
		// nginx appends its observation to the right of any client-supplied XFF,
		// so the rightmost entry is trusted and the leftmost is spoofable.
		expect(extractClientIp(h({"x-forwarded-for": "1.2.3.4, 5.6.7.8, 203.0.113.5"}))).toBe("203.0.113.5")
	})

	it("ignores spoofed leftmost X-Forwarded-For entries when X-Real-IP is present", () => {
		const headers = h({
			"x-real-ip": "203.0.113.5",
			"x-forwarded-for": "1.2.3.4, 203.0.113.5",
		})
		expect(extractClientIp(headers)).toBe("203.0.113.5")
	})

	it("returns null when neither header is present", () => {
		expect(extractClientIp(h({}))).toBeNull()
	})

	it("returns null when X-Forwarded-For is empty / whitespace-only", () => {
		expect(extractClientIp(h({"x-forwarded-for": ""}))).toBeNull()
		expect(extractClientIp(h({"x-forwarded-for": ",  ,"}))).toBeNull()
	})

	it("trims whitespace around X-Real-IP", () => {
		expect(extractClientIp(h({"x-real-ip": "  203.0.113.5  "}))).toBe("203.0.113.5")
	})

	it("handles single-entry X-Forwarded-For", () => {
		expect(extractClientIp(h({"x-forwarded-for": "203.0.113.5"}))).toBe("203.0.113.5")
	})
})
