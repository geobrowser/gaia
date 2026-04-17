import {parse} from "graphql"
import {afterEach, beforeEach, describe, expect, it, vi} from "vitest"
import {log} from "../../services/telemetry"
import {graphqlServer} from "../postgraphile"
import {extractClientIp, findSearchInvocations} from "../searchInvocationLogger"

async function executeGraphQL(query: string, variables?: Record<string, unknown>, headers?: Record<string, string>) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json", ...(headers ?? {})},
			body: JSON.stringify({query, variables}),
		}),
		{},
	)
	return {
		status: response.status,
		body: await response.json(),
	}
}

// ---------------------------------------------------------------------------
// Unit tests — pure AST walk + header parsing, no DB needed.
// ---------------------------------------------------------------------------

describe("extractClientIp", () => {
	function h(entries: Record<string, string>): Headers {
		return new Headers(entries)
	}

	it("prefers X-Real-IP", () => {
		expect(extractClientIp(h({"x-real-ip": "203.0.113.5"}))).toBe("203.0.113.5")
	})

	it("returns rightmost X-Forwarded-For entry when X-Real-IP is absent", () => {
		// nginx appends its observation to the right of any client-supplied XFF,
		// so the rightmost entry is trusted and the leftmost is spoofable.
		expect(extractClientIp(h({"x-forwarded-for": "1.2.3.4, 5.6.7.8, 203.0.113.5"}))).toBe("203.0.113.5")
	})

	it("ignores spoofed leftmost X-Forwarded-For entries when X-Real-IP is present", () => {
		const headers = h({
			"x-real-ip": "203.0.113.5",
			"x-forwarded-for": "1.2.3.4, 203.0.113.5",
		})
		expect(extractClientIp(headers)).toBe("203.0.113.5")
	})

	it("returns null when neither header is present", () => {
		expect(extractClientIp(h({}))).toBeNull()
	})

	it("returns null when X-Forwarded-For is empty / whitespace-only", () => {
		expect(extractClientIp(h({"x-forwarded-for": ""}))).toBeNull()
		expect(extractClientIp(h({"x-forwarded-for": ",  ,"}))).toBeNull()
	})

	it("trims whitespace around X-Real-IP", () => {
		expect(extractClientIp(h({"x-real-ip": "  203.0.113.5  "}))).toBe("203.0.113.5")
	})

	it("handles single-entry X-Forwarded-For", () => {
		expect(extractClientIp(h({"x-forwarded-for": "203.0.113.5"}))).toBe("203.0.113.5")
	})
})

describe("findSearchInvocations", () => {
	it("returns empty when no search field is present", () => {
		const doc = parse(`{ entities(first: 5) { id name } }`)
		expect(findSearchInvocations(doc)).toEqual([])
	})

	it("detects a top-level `search` call with inline args", () => {
		const doc = parse(`{ search(query: "geo", first: 10) { id } }`)
		expect(findSearchInvocations(doc)).toEqual([
			{field: "search", query: "geo", first: 10, spaceId: undefined, similarityThreshold: undefined},
		])
	})

	it("detects `searchConnection` and resolves numeric/string args via variables", () => {
		const doc = parse(`
			query S($q: String!, $n: Int!, $space: UUID) {
				searchConnection(query: $q, first: $n, spaceId: $space) { nodes { id } }
			}
		`)
		expect(findSearchInvocations(doc, {q: "eth", n: 25, space: "ab-cd"})).toEqual([
			{field: "searchConnection", query: "eth", first: 25, spaceId: "ab-cd", similarityThreshold: undefined},
		])
	})

	it("captures multiple invocations in one document", () => {
		const doc = parse(`
			{
				a: search(query: "foo") { id }
				b: searchConnection(query: "bar", first: 5) { nodes { id } }
			}
		`)
		const result = findSearchInvocations(doc)
		expect(result).toHaveLength(2)
		expect(result.map((i) => i.field)).toEqual(["search", "searchConnection"])
		expect(result.map((i) => i.query)).toEqual(["foo", "bar"])
	})

	it("follows inline fragments and fragment spreads", () => {
		const doc = parse(`
			fragment SearchFrag on Query { search(query: "via-fragment") { id } }
			query Outer {
				... { searchConnection(query: "inline-frag") { nodes { id } } }
				...SearchFrag
			}
		`)
		const queries = findSearchInvocations(doc).map((i) => i.query)
		expect(queries.sort()).toEqual(["inline-frag", "via-fragment"])
	})

	it("does not confuse an unrelated field named 'search' elsewhere in the selection set", () => {
		// e.g. some nested type might have its own `search` field. We only walk
		// operation selection sets and their descendants, so nested matches are
		// still logged. This test documents that behavior — any field *named*
		// search/searchConnection is logged; we don't distinguish by type.
		const doc = parse(`{ entity(id: "x") { search(query: "nested") { id } } }`)
		const result = findSearchInvocations(doc)
		expect(result).toHaveLength(1)
		expect(result[0]).toMatchObject({field: "search", query: "nested"})
	})

	it("returns undefined for args when the variable isn't supplied", () => {
		const doc = parse(`query S($q: String!) { search(query: $q) { id } }`)
		const [inv] = findSearchInvocations(doc, {})
		expect(inv).toBeDefined()
		expect(inv?.query).toBeUndefined()
	})
})

// ---------------------------------------------------------------------------
// Integration tests — real Yoga server, spies on log.warn.
// ---------------------------------------------------------------------------

describe("useSearchInvocationLogger (integration)", () => {
	let warnSpy: ReturnType<typeof vi.spyOn>

	beforeEach(() => {
		warnSpy = vi.spyOn(log, "warn").mockImplementation(() => {})
	})

	afterEach(() => {
		warnSpy.mockRestore()
	})

	function warningsMatching(substring: string): unknown[][] {
		return warnSpy.mock.calls.filter(([msg]) => typeof msg === "string" && msg.includes(substring))
	}

	it("does not warn on a non-search query", async () => {
		await executeGraphQL(`{ entities(first: 5) { id } }`)
		expect(warningsMatching("search field invoked")).toHaveLength(0)
	})

	it("warns once on a `search` invocation and captures args", async () => {
		await executeGraphQL(`{ search(query: "test-probe-1", first: 3) { id } }`)
		const warnings = warningsMatching("search field invoked")
		expect(warnings).toHaveLength(1)
		const [, payload] = warnings[0] as [string, Record<string, unknown>]
		expect(payload).toMatchObject({
			field: "search",
			query: "test-probe-1",
			first: 3,
		})
	})

	it("warns on `searchConnection` with variable-supplied args", async () => {
		await executeGraphQL(`query Probe($q: String!) { searchConnection(query: $q, first: 7) { nodes { id } } }`, {
			q: "test-probe-2",
		})
		const warnings = warningsMatching("search field invoked")
		expect(warnings).toHaveLength(1)
		const [, payload] = warnings[0] as [string, Record<string, unknown>]
		expect(payload).toMatchObject({
			field: "searchConnection",
			query: "test-probe-2",
			first: 7,
			operationName: "Probe",
		})
	})

	it("records X-Real-IP as clientIp", async () => {
		await executeGraphQL(`{ search(query: "ip-probe") { id } }`, undefined, {"X-Real-IP": "203.0.113.5"})
		const warnings = warningsMatching("search field invoked")
		expect(warnings).toHaveLength(1)
		const [, payload] = warnings[0] as [string, Record<string, unknown>]
		expect(payload.clientIp).toBe("203.0.113.5")
	})

	it("ignores spoofed leftmost X-Forwarded-For entries when X-Real-IP is present", async () => {
		await executeGraphQL(`{ search(query: "spoof-probe") { id } }`, undefined, {
			"X-Real-IP": "203.0.113.5",
			"X-Forwarded-For": "1.2.3.4, 203.0.113.5",
		})
		const warnings = warningsMatching("search field invoked")
		const [, payload] = warnings[0] as [string, Record<string, unknown>]
		expect(payload.clientIp).toBe("203.0.113.5")
	})

	it("falls back to rightmost X-Forwarded-For entry when X-Real-IP is absent", async () => {
		await executeGraphQL(`{ search(query: "xff-probe") { id } }`, undefined, {
			"X-Forwarded-For": "1.2.3.4, 203.0.113.5",
		})
		const warnings = warningsMatching("search field invoked")
		const [, payload] = warnings[0] as [string, Record<string, unknown>]
		expect(payload.clientIp).toBe("203.0.113.5")
	})

	it("captures origin and user-agent when present", async () => {
		await executeGraphQL(`{ search(query: "ua-probe") { id } }`, undefined, {
			"X-Real-IP": "203.0.113.5",
			Origin: "https://app.geobrowser.io",
			"User-Agent": "Mozilla/5.0 test-runner",
		})
		const warnings = warningsMatching("search field invoked")
		const [, payload] = warnings[0] as [string, Record<string, unknown>]
		expect(payload.origin).toBe("https://app.geobrowser.io")
		expect(payload.userAgent).toBe("Mozilla/5.0 test-runner")
	})

	it("emits one warning per search invocation in a document with multiple", async () => {
		await executeGraphQL(
			`{
				a: search(query: "q1") { id }
				b: searchConnection(query: "q2", first: 5) { nodes { id } }
			}`,
		)
		expect(warningsMatching("search field invoked")).toHaveLength(2)
	})
})
