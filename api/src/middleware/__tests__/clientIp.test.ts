import {Hono} from "hono"
import {describe, expect, it} from "vitest"
import {extractClientIp} from "../clientIp"

function appWithIpEcho(trustedHops: number) {
	const app = new Hono()
	app.get("/whoami", (c) => {
		const ip = extractClientIp(c, trustedHops)
		return c.json({ip})
	})
	return app
}

async function fetchIp(app: Hono, headers: Record<string, string>): Promise<string | null> {
	const res = await app.request("/whoami", {headers})
	const body = (await res.json()) as {ip: string | null}
	return body.ip
}

describe("extractClientIp", () => {
	it("returns null when no headers present", async () => {
		const app = appWithIpEcho(1)
		expect(await fetchIp(app, {})).toBeNull()
	})

	it("returns leftmost X-Forwarded-For with trustedHops=1", async () => {
		const app = appWithIpEcho(1)
		// "client, ingress" with trustedHops=1 → take rightmost-1 = client
		expect(await fetchIp(app, {"x-forwarded-for": "203.0.113.42, 10.108.0.5"})).toBe("203.0.113.42")
	})

	it("respects trustedHops > 1 for nested proxies", async () => {
		const app = appWithIpEcho(2)
		// "client, lb, ingress" with trustedHops=2 → take rightmost-2 = client
		expect(await fetchIp(app, {"x-forwarded-for": "203.0.113.42, 10.0.0.1, 10.108.0.5"})).toBe("203.0.113.42")
	})

	it("falls back to X-Real-IP when XFF absent", async () => {
		const app = appWithIpEcho(1)
		expect(await fetchIp(app, {"x-real-ip": "203.0.113.7"})).toBe("203.0.113.7")
	})

	it("prefers XFF over X-Real-IP", async () => {
		const app = appWithIpEcho(1)
		expect(await fetchIp(app, {"x-forwarded-for": "203.0.113.42", "x-real-ip": "203.0.113.99"})).toBe(
			"203.0.113.42",
		)
	})

	it("ignores XFF entries that fail the shape check", async () => {
		const app = appWithIpEcho(1)
		expect(await fetchIp(app, {"x-forwarded-for": "not-an-ip"})).toBeNull()
	})

	it("handles single XFF entry", async () => {
		const app = appWithIpEcho(1)
		expect(await fetchIp(app, {"x-forwarded-for": "203.0.113.42"})).toBe("203.0.113.42")
	})

	it("trims whitespace from entries", async () => {
		const app = appWithIpEcho(1)
		expect(await fetchIp(app, {"x-forwarded-for": "  203.0.113.42  ,  10.108.0.5  "})).toBe("203.0.113.42")
	})
})
