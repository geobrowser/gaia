/**
 * Extract the real client IP from request headers.
 *
 * Trusts exactly one thing: the RIGHTMOST `X-Forwarded-For` entry. Traffic reaches the api through
 * the Cilium Gateway's Envoy, which appends the address it accepted the connection from to the right
 * of whatever `X-Forwarded-For` the client sent. Everything to the left of that entry is
 * client-controlled.
 *
 * `X-Real-IP` is deliberately ignored. The old stack's ingress-nginx overwrote it with
 * `$remote_addr`, which made it the best header to read; Envoy does not set it at all, so on this
 * cluster it is whatever the caller chose to send. Verified 2026-10-03 against the live api: a
 * request with `X-Real-IP: 203.0.113.77` was logged as 203.0.113.77, and one with a forged
 * `X-Forwarded-For` was logged with the sender's true address. Reading `X-Real-IP` first let any
 * caller pick their own identity — for the logs, and for the per-IP rate limiter.
 *
 * Returns `null` when there is no `X-Forwarded-For`, which is the case for in-cluster callers that
 * reach the Service directly without passing the Gateway.
 */
export function extractClientIp(headers: Headers): string | null {
	const xff = headers.get("x-forwarded-for")
	if (!xff) return null
	const parts = xff
		.split(",")
		.map((s) => s.trim())
		.filter(Boolean)
	return parts[parts.length - 1] ?? null
}
