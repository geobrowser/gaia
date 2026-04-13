import {describe, expect, it} from "vitest"
import {ipInAnyCidr, ipInCidr, parseCidr, parseIPv4} from "../cidr"

describe("parseIPv4", () => {
	it("parses dotted-quad addresses", () => {
		expect(parseIPv4("0.0.0.0")).toBe(0)
		expect(parseIPv4("255.255.255.255")).toBe(0xffffffff)
		expect(parseIPv4("10.0.0.1")).toBe((10 << 24) | 1)
		expect(parseIPv4("192.168.1.1")).toBe(((192 << 24) | (168 << 16) | (1 << 8) | 1) >>> 0)
	})

	it("rejects invalid addresses", () => {
		expect(parseIPv4("")).toBeNull()
		expect(parseIPv4("1.2.3")).toBeNull()
		expect(parseIPv4("1.2.3.4.5")).toBeNull()
		expect(parseIPv4("256.0.0.0")).toBeNull()
		expect(parseIPv4("1.2.3.foo")).toBeNull()
		expect(parseIPv4("::1")).toBeNull() // IPv6 not supported
	})
})

describe("parseCidr", () => {
	it("treats bare IPs as /32", () => {
		const c = parseCidr("10.0.0.5")
		expect(c).not.toBeNull()
		expect(c?.mask).toBe(0xffffffff)
		expect(c?.network).toBe(parseIPv4("10.0.0.5"))
	})

	it("parses standard CIDR notation", () => {
		const c = parseCidr("10.244.0.0/16")
		expect(c).not.toBeNull()
		expect(c?.mask).toBe(0xffff0000)
		expect(c?.network).toBe(parseIPv4("10.244.0.0"))
	})

	it("masks non-network bits in the network address", () => {
		// 10.244.5.42/16 should normalize to 10.244.0.0
		const c = parseCidr("10.244.5.42/16")
		expect(c?.network).toBe(parseIPv4("10.244.0.0"))
	})

	it("supports /0 (match anything)", () => {
		const c = parseCidr("0.0.0.0/0")
		expect(c?.mask).toBe(0)
		expect(c?.network).toBe(0)
	})

	it("rejects invalid prefixes", () => {
		expect(parseCidr("10.0.0.0/33")).toBeNull()
		expect(parseCidr("10.0.0.0/-1")).toBeNull()
		expect(parseCidr("10.0.0.0/abc")).toBeNull()
	})

	it("rejects empty / whitespace", () => {
		expect(parseCidr("")).toBeNull()
		expect(parseCidr("   ")).toBeNull()
	})
})

describe("ipInCidr", () => {
	it("matches /32 exactly", () => {
		const c = parseCidr("10.0.0.5/32")!
		expect(ipInCidr("10.0.0.5", c)).toBe(true)
		expect(ipInCidr("10.0.0.6", c)).toBe(false)
	})

	it("matches all addresses inside a /16", () => {
		const c = parseCidr("10.244.0.0/16")!
		expect(ipInCidr("10.244.0.0", c)).toBe(true)
		expect(ipInCidr("10.244.0.1", c)).toBe(true)
		expect(ipInCidr("10.244.5.42", c)).toBe(true)
		expect(ipInCidr("10.244.255.255", c)).toBe(true)
		expect(ipInCidr("10.245.0.0", c)).toBe(false)
		expect(ipInCidr("11.244.0.0", c)).toBe(false)
	})

	it("/0 matches everything", () => {
		const c = parseCidr("0.0.0.0/0")!
		expect(ipInCidr("1.2.3.4", c)).toBe(true)
		expect(ipInCidr("255.255.255.255", c)).toBe(true)
	})

	it("returns false for invalid IP", () => {
		const c = parseCidr("10.0.0.0/24")!
		expect(ipInCidr("not-an-ip", c)).toBe(false)
	})
})

describe("ipInAnyCidr", () => {
	it("returns true if any cidr matches", () => {
		const cidrs = [parseCidr("10.108.0.0/16")!, parseCidr("10.109.0.0/16")!]
		expect(ipInAnyCidr("10.108.5.42", cidrs)).toBe(true)
		expect(ipInAnyCidr("10.109.0.1", cidrs)).toBe(true)
		expect(ipInAnyCidr("192.168.1.1", cidrs)).toBe(false)
	})

	it("returns false on empty list", () => {
		expect(ipInAnyCidr("10.0.0.1", [])).toBe(false)
	})
})
