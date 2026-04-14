import {describe, expect, it} from "vitest"
import {ipInAnyCidr, ipInCidr, parseCidr, parseIPv4} from "../cidr"

// ─── parseIPv4 ───────────────────────────────────────────────────────

describe("parseIPv4", () => {
	it("parses dotted-quad addresses", () => {
		expect(parseIPv4("0.0.0.0")).toBe(0)
		expect(parseIPv4("255.255.255.255")).toBe(0xffffffff)
		expect(parseIPv4("10.0.0.1")).toBe((10 << 24) | 1)
		expect(parseIPv4("192.168.1.1")).toBe(((192 << 24) | (168 << 16) | (1 << 8) | 1) >>> 0)
		expect(parseIPv4("127.0.0.1")).toBe(((127 << 24) | 1) >>> 0)
		expect(parseIPv4("1.1.1.1")).toBe(((1 << 24) | (1 << 16) | (1 << 8) | 1) >>> 0)
	})

	it("parses boundary octets", () => {
		expect(parseIPv4("0.0.0.0")).toBe(0)
		expect(parseIPv4("0.0.0.255")).toBe(255)
		expect(parseIPv4("255.0.0.0")).toBe(0xff000000 >>> 0)
	})

	it("rejects leading-zero octets (octal ambiguity)", () => {
		expect(parseIPv4("010.0.0.1")).toBeNull()
		expect(parseIPv4("01.02.03.04")).toBeNull()
		expect(parseIPv4("1.2.3.04")).toBeNull()
		expect(parseIPv4("00.0.0.0")).toBeNull()
		expect(parseIPv4("0.0.0.00")).toBeNull()
		expect(parseIPv4("192.168.01.1")).toBeNull()
		expect(parseIPv4("192.168.001.1")).toBeNull()
	})

	it("allows single zero octets", () => {
		expect(parseIPv4("0.0.0.0")).toBe(0)
		expect(parseIPv4("10.0.0.0")).not.toBeNull()
		expect(parseIPv4("0.0.0.1")).toBe(1)
	})

	it("rejects too few octets", () => {
		expect(parseIPv4("")).toBeNull()
		expect(parseIPv4("1")).toBeNull()
		expect(parseIPv4("1.2")).toBeNull()
		expect(parseIPv4("1.2.3")).toBeNull()
	})

	it("rejects too many octets", () => {
		expect(parseIPv4("1.2.3.4.5")).toBeNull()
		expect(parseIPv4("1.2.3.4.5.6")).toBeNull()
	})

	it("rejects octets > 255", () => {
		expect(parseIPv4("256.0.0.0")).toBeNull()
		expect(parseIPv4("0.0.0.256")).toBeNull()
		expect(parseIPv4("999.999.999.999")).toBeNull()
	})

	it("rejects non-numeric characters", () => {
		expect(parseIPv4("1.2.3.foo")).toBeNull()
		expect(parseIPv4("a.b.c.d")).toBeNull()
		expect(parseIPv4("1.2.3.-1")).toBeNull()
		expect(parseIPv4("1.2.3. 4")).toBeNull()
		expect(parseIPv4("1.2.3.4 ")).toBeNull()
		expect(parseIPv4(" 1.2.3.4")).toBeNull()
	})

	it("rejects empty octets", () => {
		expect(parseIPv4(".1.2.3")).toBeNull()
		expect(parseIPv4("1..2.3")).toBeNull()
		expect(parseIPv4("1.2.3.")).toBeNull()
	})

	it("rejects IPv6 addresses", () => {
		expect(parseIPv4("::1")).toBeNull()
		expect(parseIPv4("fe80::1")).toBeNull()
		expect(parseIPv4("2001:db8::1")).toBeNull()
	})

	it("rejects hex/octal notation", () => {
		expect(parseIPv4("0x0a.0.0.1")).toBeNull()
		expect(parseIPv4("0xa.0xb.0xc.0xd")).toBeNull()
	})
})

// ─── parseCidr ───────────────────────────────────────────────────────

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

	it("parses common prefix lengths", () => {
		expect(parseCidr("10.0.0.0/8")?.mask).toBe(0xff000000)
		expect(parseCidr("10.0.0.0/16")?.mask).toBe(0xffff0000)
		expect(parseCidr("10.0.0.0/24")?.mask).toBe(0xffffff00)
		expect(parseCidr("10.0.0.0/32")?.mask).toBe(0xffffffff)
	})

	it("masks non-network bits in the network address", () => {
		// 10.244.5.42/16 should normalize to 10.244.0.0
		const c = parseCidr("10.244.5.42/16")
		expect(c?.network).toBe(parseIPv4("10.244.0.0"))

		// 192.168.1.100/24 should normalize to 192.168.1.0
		const c2 = parseCidr("192.168.1.100/24")
		expect(c2?.network).toBe(parseIPv4("192.168.1.0"))
	})

	it("supports /0 (match anything)", () => {
		const c = parseCidr("0.0.0.0/0")
		expect(c?.mask).toBe(0)
		expect(c?.network).toBe(0)
	})

	it("supports /1 (half of IPv4 space)", () => {
		const c = parseCidr("0.0.0.0/1")
		expect(c?.mask).toBe(0x80000000)
	})

	it("preserves original source string", () => {
		expect(parseCidr("  10.0.0.0/24  ")?.source).toBe("10.0.0.0/24")
	})

	it("trims whitespace", () => {
		expect(parseCidr("  10.0.0.0/24  ")).not.toBeNull()
		expect(parseCidr("\t10.0.0.0/24\t")).not.toBeNull()
	})

	it("rejects invalid prefixes", () => {
		expect(parseCidr("10.0.0.0/33")).toBeNull()
		expect(parseCidr("10.0.0.0/-1")).toBeNull()
		expect(parseCidr("10.0.0.0/abc")).toBeNull()
		expect(parseCidr("10.0.0.0/")).toBeNull()
		expect(parseCidr("10.0.0.0/1.5")).toBeNull()
	})

	it("rejects empty / whitespace", () => {
		expect(parseCidr("")).toBeNull()
		expect(parseCidr("   ")).toBeNull()
		expect(parseCidr("\t")).toBeNull()
	})

	it("rejects CIDR with invalid IP part", () => {
		expect(parseCidr("256.0.0.0/24")).toBeNull()
		expect(parseCidr("foo/24")).toBeNull()
		expect(parseCidr("010.0.0.0/8")).toBeNull() // leading zero
	})

	it("rejects double slash", () => {
		expect(parseCidr("10.0.0.0//24")).toBeNull()
	})
})

// ─── ipInCidr ────────────────────────────────────────────────────────

describe("ipInCidr", () => {
	it("matches /32 exactly", () => {
		const c = parseCidr("10.0.0.5/32")!
		expect(ipInCidr("10.0.0.5", c)).toBe(true)
		expect(ipInCidr("10.0.0.4", c)).toBe(false)
		expect(ipInCidr("10.0.0.6", c)).toBe(false)
	})

	it("matches all addresses inside a /24", () => {
		const c = parseCidr("192.168.1.0/24")!
		expect(ipInCidr("192.168.1.0", c)).toBe(true)
		expect(ipInCidr("192.168.1.1", c)).toBe(true)
		expect(ipInCidr("192.168.1.255", c)).toBe(true)
		expect(ipInCidr("192.168.2.0", c)).toBe(false)
		expect(ipInCidr("192.168.0.255", c)).toBe(false)
	})

	it("matches all addresses inside a /16", () => {
		const c = parseCidr("10.244.0.0/16")!
		expect(ipInCidr("10.244.0.0", c)).toBe(true)
		expect(ipInCidr("10.244.0.1", c)).toBe(true)
		expect(ipInCidr("10.244.5.42", c)).toBe(true)
		expect(ipInCidr("10.244.255.255", c)).toBe(true)
		expect(ipInCidr("10.245.0.0", c)).toBe(false)
		expect(ipInCidr("10.243.255.255", c)).toBe(false)
		expect(ipInCidr("11.244.0.0", c)).toBe(false)
	})

	it("matches /8 (class A)", () => {
		const c = parseCidr("10.0.0.0/8")!
		expect(ipInCidr("10.0.0.0", c)).toBe(true)
		expect(ipInCidr("10.255.255.255", c)).toBe(true)
		expect(ipInCidr("11.0.0.0", c)).toBe(false)
		expect(ipInCidr("9.255.255.255", c)).toBe(false)
	})

	it("/0 matches everything", () => {
		const c = parseCidr("0.0.0.0/0")!
		expect(ipInCidr("0.0.0.0", c)).toBe(true)
		expect(ipInCidr("1.2.3.4", c)).toBe(true)
		expect(ipInCidr("255.255.255.255", c)).toBe(true)
	})

	it("boundary IPs at CIDR edges", () => {
		const c = parseCidr("10.0.0.0/25")!
		// /25 = 128 addresses: 10.0.0.0 – 10.0.0.127
		expect(ipInCidr("10.0.0.0", c)).toBe(true)
		expect(ipInCidr("10.0.0.127", c)).toBe(true)
		expect(ipInCidr("10.0.0.128", c)).toBe(false)
	})

	it("returns false for invalid IP input", () => {
		const c = parseCidr("10.0.0.0/24")!
		expect(ipInCidr("not-an-ip", c)).toBe(false)
		expect(ipInCidr("", c)).toBe(false)
		expect(ipInCidr("010.0.0.1", c)).toBe(false) // leading zero rejected
	})
})

// ─── ipInAnyCidr ─────────────────────────────────────────────────────

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

	it("matches first entry without checking the rest", () => {
		const cidrs = [parseCidr("10.0.0.0/8")!, parseCidr("192.168.0.0/16")!]
		expect(ipInAnyCidr("10.0.0.1", cidrs)).toBe(true)
	})

	it("matches last entry", () => {
		const cidrs = [parseCidr("192.168.0.0/16")!, parseCidr("10.0.0.0/8")!]
		expect(ipInAnyCidr("10.0.0.1", cidrs)).toBe(true)
	})

	it("handles mixed specificity CIDRs", () => {
		const cidrs = [parseCidr("10.0.0.1/32")!, parseCidr("192.168.0.0/16")!, parseCidr("0.0.0.0/0")!]
		expect(ipInAnyCidr("10.0.0.1", cidrs)).toBe(true)
		expect(ipInAnyCidr("172.16.0.1", cidrs)).toBe(true) // /0 catches everything
	})
})
