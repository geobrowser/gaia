import {describe, expect, it} from "vitest"
import {redactUrlCredentials} from "../redactUrl"

describe("redactUrlCredentials", () => {
	it("strips a username and password from the URL", () => {
		expect(redactUrlCredentials("https://doadmin:hunter2@search.example.com:25060")).toBe(
			"https://search.example.com:25060",
		)
	})

	it("leaves a URL without userinfo unchanged (scheme/host/port only)", () => {
		expect(redactUrlCredentials("http://localhost:9200")).toBe("http://localhost:9200")
	})

	it("drops a path, query string and fragment along with any userinfo", () => {
		// A non-default port, so it's reported explicitly rather than treated
		// as implied by the scheme.
		expect(redactUrlCredentials("https://user:pass@host:8443/some/path?query=1#frag")).toBe("https://host:8443")
	})

	it("handles special characters in the password", () => {
		// '@', ':' and '/' in a password must be percent-encoded in a valid URL,
		// but the redaction must still never leak the decoded or encoded secret.
		const url = "postgres://admin:p%40ss%2Fw%3Ard@db.example.com:5432/mydb"
		const result = redactUrlCredentials(url)
		expect(result).toBe("postgres://db.example.com:5432")
		expect(result).not.toContain("admin")
		expect(result).not.toContain("p@ss")
		expect(result).not.toContain("p%40ss")
	})

	it("falls back to a fixed placeholder for unparseable input", () => {
		expect(redactUrlCredentials("not a url at all")).toBe("[redacted-invalid-url]")
		expect(redactUrlCredentials("")).toBe("[redacted-invalid-url]")
	})
})
