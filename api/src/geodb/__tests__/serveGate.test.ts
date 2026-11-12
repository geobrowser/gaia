import {beforeEach, describe, expect, it} from "vitest"

import {
	__resetServeGateForTests,
	activateServeGate,
	createMatchLedger,
	failureReason,
	readServeGateConfig,
	recordServed,
	recordServeFallback,
	renderServeGateMetrics,
	ServeTimeout,
	withServeTimeout,
} from "../serveGate"

describe("readServeGateConfig", () => {
	it("is off unless the family's own switch is explicitly on", () => {
		expect(readServeGateConfig({}, "governance").enabled).toBe(false)
		expect(readServeGateConfig({GEODB_GOVERNANCE_SERVE: "0"}, "governance").enabled).toBe(false)
		expect(readServeGateConfig({GEODB_GOVERNANCE_SERVE: "1"}, "governance").enabled).toBe(true)
		expect(readServeGateConfig({GEODB_GOVERNANCE_SERVE: "1"}, "versions").enabled).toBe(false)
		expect(readServeGateConfig({GEODB_VERSIONS_SERVE: "on"}, "versions").enabled).toBe(true)
	})
	it("takes a TTL and timeout, falling back to defaults on junk", () => {
		const c = readServeGateConfig(
			{
				GEODB_VERSIONS_SERVE: "true",
				GEODB_VERSIONS_SERVE_MATCH_TTL_MS: "1000",
				GEODB_VERSIONS_SERVE_TIMEOUT_MS: "x",
			},
			"versions",
		)
		expect(c).toEqual({enabled: true, matchTtlMs: 1000, timeoutMs: 500})
	})
})

describe("match ledger", () => {
	it("serves only a fresh match, and says why otherwise", () => {
		let t = 0
		const l = createMatchLedger<string>({ttlMs: 100, now: () => t})
		expect(l.check("a")).toEqual({ok: false, reason: "no_comparison"})
		l.record("a", "match", "x")
		expect(l.check("a")).toEqual({ok: true, detail: "x"})
		t = 101
		expect(l.check("a")).toEqual({ok: false, reason: "stale"})
		l.record("a", "mismatch")
		expect(l.check("a")).toEqual({ok: false, reason: "mismatch"})
		l.record("a", "unsupported")
		expect(l.check("a")).toEqual({ok: false, reason: "unsupported"})
		l.record("a", "match")
		expect(l.check("a").ok).toBe(true)
	})
	it("drops the oldest key past its bound", () => {
		const l = createMatchLedger({ttlMs: 1000, maxKeys: 2})
		l.record("a", "match")
		l.record("b", "match")
		l.record("c", "match")
		expect(l.check("a")).toEqual({ok: false, reason: "no_comparison"})
		expect(l.check("b").ok).toBe(true)
		expect(l.check("c").ok).toBe(true)
	})
})

describe("timeouts", () => {
	it("rejects a slow read with ServeTimeout and passes a fast one", async () => {
		await expect(withServeTimeout(new Promise(() => {}), 5)).rejects.toBeInstanceOf(ServeTimeout)
		await expect(withServeTimeout(Promise.resolve(3), 50)).resolves.toBe(3)
		await expect(withServeTimeout(Promise.reject(new Error("boom")), 50)).rejects.toThrow("boom")
	})
	it("labels failures", () => {
		expect(failureReason(new ServeTimeout())).toBe("timeout")
		expect(failureReason(new Error("request timed out"))).toBe("timeout")
		expect(failureReason(new Error("boom"))).toBe("error")
	})
})

describe("metrics", () => {
	beforeEach(() => __resetServeGateForTests())
	it("export nothing until a family is activated", () => {
		recordServed("governance", "r")
		expect(renderServeGateMetrics()).toBe("")
	})
	it("export every reason at zero once active, and count by family, read and reason", () => {
		activateServeGate("versions", ["relation_lookup"])
		recordServed("versions", "relation_lookup")
		recordServeFallback("versions", "relation_lookup", "mismatch")
		recordServeFallback("versions", "relation_lookup", "mismatch")
		const text = renderServeGateMetrics()
		expect(text).toContain('gaia_api_geodb_serve_total{family="versions",read="relation_lookup"} 1')
		expect(text).toContain(
			'gaia_api_geodb_serve_fallback_total{family="versions",read="relation_lookup",reason="mismatch"} 2',
		)
		expect(text).toContain(
			'gaia_api_geodb_serve_fallback_total{family="versions",read="relation_lookup",reason="shadow_off"} 0',
		)
		expect(text).not.toContain('family="governance"')
	})
})
