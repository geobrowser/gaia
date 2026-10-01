/**
 * Redact credentials from a URL before it is logged.
 *
 * Connection strings such as `OPENSEARCH_URL` and `DATABASE_URL` embed a
 * username and password as URL userinfo (`scheme://user:password@host:port`).
 * Logging the raw value leaks the credential into log aggregation, CI output
 * and anything else that collects stdout. This keeps only what's useful for
 * diagnostics — scheme, host and port — and drops the userinfo entirely.
 *
 * Falls back to a fixed placeholder for input that can't be parsed as a URL,
 * so a malformed value never leaks verbatim either.
 */
export function redactUrlCredentials(rawUrl: string): string {
	try {
		const parsed = new URL(rawUrl)
		return `${parsed.protocol}//${parsed.host}`
	} catch {
		return "[redacted-invalid-url]"
	}
}
