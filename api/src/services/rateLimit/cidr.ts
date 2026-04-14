/**
 * Minimal IPv4 CIDR matcher used by the rate-limit middleware to evaluate the
 * env-configured allowlist. Per-row DB overrides are matched server-side via
 * Postgres's native `>>=` operator on the `cidr` column.
 *
 * IPv6 is not supported; allowlist entries that don't parse as IPv4 (or as an
 * IPv4 CIDR like `10.0.0.0/24`) are ignored at parse time and logged as a
 * warning. We can extend to IPv6 if/when a use case appears.
 */

export type Cidr = {
	/** Network address as 32-bit unsigned integer */
	network: number
	/** Subnet mask as 32-bit unsigned integer */
	mask: number
	/** Original string, kept for logging */
	source: string
}

/** Parse a dotted-quad IPv4 to an unsigned 32-bit integer, or `null` if invalid. */
export function parseIPv4(ip: string): number | null {
	const parts = ip.split(".")
	if (parts.length !== 4) return null
	let result = 0
	for (const part of parts) {
		// Reject leading zeros (ambiguous: octal in C/Python, decimal in JS),
		// non-digit, and out-of-range octets. Only "0" itself may start with 0.
		if (!/^(0|[1-9]\d{0,2})$/.test(part)) return null
		const n = Number(part)
		if (n < 0 || n > 255) return null
		result = (result << 8) | n
	}
	// Force unsigned interpretation
	return result >>> 0
}

/** Parse `"1.2.3.0/24"` or `"1.2.3.4"` (treated as `/32`). Returns `null` on invalid input. */
export function parseCidr(entry: string): Cidr | null {
	const trimmed = entry.trim()
	if (trimmed === "") return null

	const slash = trimmed.indexOf("/")
	const ipPart = slash === -1 ? trimmed : trimmed.slice(0, slash)
	const prefixPart = slash === -1 ? "32" : trimmed.slice(slash + 1)

	const ip = parseIPv4(ipPart)
	if (ip === null) return null

	if (!/^\d{1,2}$/.test(prefixPart)) return null
	const prefix = Number(prefixPart)
	if (prefix < 0 || prefix > 32) return null

	// /0 means "match anything" — mask is 0
	const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0
	const network = (ip & mask) >>> 0

	return {network, mask, source: trimmed}
}

/** Returns true if `ip` (dotted-quad) is contained in `cidr`. */
export function ipInCidr(ip: string, cidr: Cidr): boolean {
	const numeric = parseIPv4(ip)
	if (numeric === null) return false
	return (numeric & cidr.mask) >>> 0 === cidr.network
}

/** Returns true if `ip` matches any of the supplied CIDRs. */
export function ipInAnyCidr(ip: string, cidrs: readonly Cidr[]): boolean {
	if (cidrs.length === 0) return false
	for (const cidr of cidrs) {
		if (ipInCidr(ip, cidr)) return true
	}
	return false
}
