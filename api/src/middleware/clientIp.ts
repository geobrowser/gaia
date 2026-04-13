import type {Context} from "hono"

/**
 * Extract the originating client IP from a Hono request, honoring `X-Forwarded-For`
 * with a configurable trusted-proxy hop count. Falls back to `X-Real-IP`.
 *
 * Behind ingress-nginx (DOKS), the typical chain is:
 *   client_ip, ingress_ip
 * so `trustedHops = 1` returns the leftmost (`client_ip`).
 *
 * Returns `null` when no header is present or no IP is parseable.
 */
export function extractClientIp(c: Context, trustedHops: number): string | null {
	const xff = c.req.header("x-forwarded-for")
	if (xff) {
		// Take the (Nth-from-right) entry where N = trustedHops.
		// e.g. "client, lb, ingress" with trustedHops=2 picks "client".
		const parts = xff
			.split(",")
			.map((s) => s.trim())
			.filter(Boolean)

		if (parts.length > 0) {
			// XFF is "client, proxy1, proxy2, …, immediate". We trust the rightmost
			// `trustedHops` entries (our own infra) and pick the rightmost untrusted
			// entry. With more reported hops than we trust, leftmost entries may be
			// spoofed; rightmost-untrusted is the standard safe choice.
			const idx = Math.max(0, parts.length - 1 - trustedHops)
			const candidate = parts[idx]
			if (candidate && isPlausibleIp(candidate)) return candidate
		}
	}

	const xRealIp = c.req.header("x-real-ip")
	if (xRealIp && isPlausibleIp(xRealIp)) return xRealIp.trim()

	return null
}

/**
 * Cheap shape check: digits/dots/colons only, length bounded.
 * Real validation is done downstream by `parseIPv4` / Postgres.
 */
function isPlausibleIp(value: string): boolean {
	const trimmed = value.trim()
	if (trimmed.length < 3 || trimmed.length > 45) return false
	return /^[0-9a-fA-F.:]+$/.test(trimmed)
}
