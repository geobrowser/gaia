/**
 * Extract the real client IP from request headers.
 *
 * Prefers `X-Real-IP` because it's the unspoofable value in this cluster's
 * topology: DO LoadBalancer → ingress-nginx with `externalTrafficPolicy:
 * Local` and L4 TCP passthrough preserves the real client source IP to
 * nginx, and nginx sets `X-Real-IP = $remote_addr` — single-valued, and
 * overwrites any client-supplied header so it can't be forged over HTTP.
 *
 * `X-Forwarded-For` is a fallback: nginx appends its own observed
 * `$remote_addr` to the *right* of any client-supplied XFF via
 * `$proxy_add_x_forwarded_for`, so the **rightmost** entry is trustworthy
 * and everything to the left is client-controlled / spoofable. We never
 * read the leftmost XFF entry here.
 *
 * Returns `null` if neither header is present.
 */
export function extractClientIp(headers: Headers): string | null {
	const xRealIp = headers.get("x-real-ip")?.trim()
	if (xRealIp) return xRealIp

	const xff = headers.get("x-forwarded-for")
	if (xff) {
		const parts = xff
			.split(",")
			.map((s) => s.trim())
			.filter(Boolean)
		const rightmost = parts[parts.length - 1]
		if (rightmost) return rightmost
	}
	return null
}
