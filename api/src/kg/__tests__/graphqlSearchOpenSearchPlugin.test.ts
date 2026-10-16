import {print as gqlPrint, parse} from "graphql"
import {afterEach, beforeEach, describe, expect, it, vi} from "vitest"
import {DEFAULT_EXCLUDED_TYPE_IDS} from "../../search"
import type {SearchClient, SearchQuery, SearchResponse} from "../../services/search"
import {log} from "../../services/telemetry"
import {
	buildOpenSearchQuery,
	CLAIM_TYPE_ID,
	computeFetchSize,
	findRootSearchFields,
	injectIds,
	resolveSearchIds,
} from "../graphqlSearchOpenSearchPlugin"

const A = "11111111-1111-4111-8111-111111111111"
const B = "22222222-2222-4222-8222-222222222222"
const SPACE = "33333333-3333-4333-8333-333333333333"
const TYPE = "44444444-4444-4444-8444-444444444444"
const print = (d: Parameters<typeof gqlPrint>[0]) => gqlPrint(d).replace(/\s+/g, " ")
const dashless = (s: string) => s.replaceAll("-", "")

function client(impl: (q: SearchQuery) => Promise<string[]>): SearchClient & {calls: SearchQuery[]} {
	const calls: SearchQuery[] = []
	return {
		calls,
		async search(q: SearchQuery) {
			calls.push(q)
			const ids = await impl(q)
			return {
				results: ids.map((entityId) => ({entityId})),
				total: ids.length,
				tookMs: 1,
			} as unknown as SearchResponse
		},
		async healthCheck() {
			return true
		},
	}
}

async function inject(src: string, c: SearchClient, variables: Record<string, unknown> = {}) {
	const doc = parse(src)
	const ids = await resolveSearchIds(doc, variables, c, 200)
	return {doc, out: injectIds(doc, ids), ids}
}

describe("computeFetchSize", () => {
	it("is offset + first + slack", () => {
		expect(computeFetchSize({first: 10, cursorPaged: false})).toBe(30)
		expect(computeFetchSize({first: 10, offset: 5, cursorPaged: false})).toBe(35)
	})
	it("rises to at least 100 with a filter, and caps at 100", () => {
		expect(computeFetchSize({first: 10, cursorPaged: false, filter: {typeIds: {in: [A]}}})).toBe(100)
		expect(computeFetchSize({first: 500, cursorPaged: false})).toBe(100)
	})
	it("ignores an empty filter", () => {
		expect(computeFetchSize({first: 10, cursorPaged: false, filter: {}})).toBe(30)
	})
	it("takes the cap when there is no first or the page is cursor based", () => {
		expect(computeFetchSize({cursorPaged: false})).toBe(100)
		expect(computeFetchSize({first: 5, cursorPaged: true})).toBe(100)
	})
})

describe("buildOpenSearchQuery", () => {
	const base = {cursorPaged: false, first: 10}

	it("excludes the REST defaults and Claims", () => {
		const q = buildOpenSearchQuery({...base, query: " geo "})
		expect(q?.query).toBe("geo")
		expect(q?.scope).toBe("GLOBAL")
		const ex = q?.exclude_type_ids ?? []
		for (const id of [...DEFAULT_EXCLUDED_TYPE_IDS, CLAIM_TYPE_ID]) expect(ex).toContain(dashless(id))
	})

	it("does not exclude a type the filter asks for, and hints it", () => {
		const q = buildOpenSearchQuery({...base, query: "x", filter: {typeIds: {in: [CLAIM_TYPE_ID, TYPE]}}})
		expect(q?.exclude_type_ids).not.toContain(dashless(CLAIM_TYPE_ID))
		expect(q?.exclude_type_ids).toContain(dashless(DEFAULT_EXCLUDED_TYPE_IDS[0] as string))
		expect(q?.type_ids).toEqual([dashless(CLAIM_TYPE_ID), dashless(TYPE)])
	})

	it("keeps a non-`in` typeIds filter from excluding what it names, without hinting", () => {
		const q = buildOpenSearchQuery({...base, query: "x", filter: {typeIds: {anyEqualTo: CLAIM_TYPE_ID}}})
		expect(q?.exclude_type_ids).not.toContain(dashless(CLAIM_TYPE_ID))
		expect(q?.type_ids).toBeUndefined()
	})

	it("maps spaceId, or a single-space filter, to SPACE_SINGLE", () => {
		expect(buildOpenSearchQuery({...base, query: "x", spaceId: SPACE})).toMatchObject({
			scope: "SPACE_SINGLE",
			space_id: dashless(SPACE),
		})
		expect(buildOpenSearchQuery({...base, query: "x", filter: {spaceIds: {anyEqualTo: SPACE}}})).toMatchObject({
			scope: "SPACE_SINGLE",
			space_id: dashless(SPACE),
		})
		expect(buildOpenSearchQuery({...base, query: "x", filter: {spaceIds: {in: [SPACE, A]}}})?.scope).toBe("GLOBAL")
	})

	it("leaves an empty query on SQL", () => {
		expect(buildOpenSearchQuery({...base, query: "   "})).toBeNull()
	})

	it("sizes the request", () => {
		expect(buildOpenSearchQuery({...base, query: "x"})?.limit).toBe(30)
		expect(buildOpenSearchQuery({...base, query: "x", filter: {typeIds: {in: [TYPE]}}})?.limit).toBe(100)
	})
})

describe("findRootSearchFields", () => {
	it("finds root fields, through fragments, and ignores nested ones and mutations", () => {
		const doc = parse(`
			query { a: search(query: "a") { id } ...F entities { search { id } } }
			fragment F on Query { searchConnection(query: "b") { nodes { id } } }
			mutation { search(query: "m") { id } }
		`)
		expect(findRootSearchFields(doc).map((f) => f.alias?.value ?? f.name.value)).toEqual(["a", "searchConnection"])
	})
})

describe("injectIds", () => {
	it("injects ids for a literal query and keeps alias, first, offset and filter intact", async () => {
		const c = client(async () => [A, B])
		const {doc, out} = await inject(
			`{ hits: search(query: "geo", first: 5, offset: 2, filter: {typeIds: {in: ["${TYPE}"]}}) { id name } }`,
			c,
		)
		const printed = print(out)
		expect(printed).toContain(`hits: search(`)
		for (const arg of [`query: "geo"`, "first: 5", "offset: 2", `filter: {typeIds: {in: ["${TYPE}"]}}`]) {
			expect(printed).toContain(arg)
		}
		expect(printed).toContain(`ids: ["${dashless(A)}", "${dashless(B)}"]`)
		expect(print(doc)).not.toContain("ids:")
		expect(c.calls).toHaveLength(1)
		expect(c.calls[0]?.limit).toBe(100)
	})

	it("resolves query, first and filter from variables", async () => {
		const c = client(async () => [A])
		const {out} = await inject(
			`query Q($q: String!, $n: Int, $f: EntityFilter) { searchConnection(query: $q, first: $n, filter: $f) { nodes { id } } }`,
			c,
			{q: "geo", n: 3, f: {typeIds: {in: [TYPE]}}},
		)
		expect(print(out)).toContain(`ids: ["${dashless(A)}"]`)
		expect(c.calls[0]).toMatchObject({query: "geo", type_ids: [dashless(TYPE)], limit: 100})
	})

	it("handles several search fields in one document, deduplicating identical lookups", async () => {
		const c = client(async (q) => (q.query === "one" ? [A] : [B]))
		const {out} = await inject(
			`{ a: search(query: "one", first: 3) { id } b: search(query: "one", first: 3) { id } c: searchConnection(query: "two", first: 3) { nodes { id } } }`,
			c,
		)
		const printed = print(out)
		expect(printed.match(/ids: \[/g)).toHaveLength(3)
		expect(printed).toMatch(
			new RegExp(`c: searchConnection\\( ?query: "two",? first: 3,? ids: \\["${dashless(B)}"\\] ?\\)`),
		)
		expect(c.calls).toHaveLength(2)
	})

	it("leaves a field that already passes ids alone, without calling OpenSearch", async () => {
		const c = client(async () => [A])
		const {out} = await inject(`{ search(query: "geo", ids: ["${B}"]) { id } }`, c)
		expect(print(out)).toContain(`ids: ["${B}"]`)
		expect(print(out)).not.toContain(dashless(A))
		expect(c.calls).toHaveLength(0)
	})

	it("injects an empty list when OpenSearch finds nothing", async () => {
		const {out} = await inject(
			`{ search(query: "zzzz") { id } }`,
			client(async () => []),
		)
		expect(print(out)).toContain("ids: []")
	})

	it("does not inject for an empty or non-string query", async () => {
		const c = client(async () => [A])
		const {out} = await inject(
			`query Q($q: String) { a: search(query: "") { id } b: search(query: $q) { id } }`,
			c,
			{},
		)
		expect(print(out)).not.toContain("ids:")
		expect(c.calls).toHaveLength(0)
	})

	it("injects into a field inside a fragment", async () => {
		const {out} = await inject(
			`query { ...F } fragment F on Query { search(query: "geo") { id } }`,
			client(async () => [A]),
		)
		expect(print(out)).toContain(`ids: ["${dashless(A)}"]`)
	})
})

describe("fallback", () => {
	let warn: ReturnType<typeof vi.spyOn>
	beforeEach(() => {
		warn = vi.spyOn(log, "warn").mockImplementation(() => {})
	})
	afterEach(() => warn.mockRestore())

	it("does not inject and warns once when the client throws", async () => {
		const c = client(async () => {
			throw new Error("boom")
		})
		const {doc, out} = await inject(`{ a: search(query: "one") { id } b: search(query: "two") { id } }`, c)
		expect(out).toBe(doc)
		expect(warn).toHaveBeenCalledTimes(1)
		expect(warn.mock.calls[0]?.[1]).toMatchObject({error: "boom"})
	})

	it("falls back on a timeout", async () => {
		const c = client(() => new Promise(() => {}))
		const {out, doc} = await inject(`{ search(query: "slow") { id } }`, c)
		expect(out).toBe(doc)
		expect(warn).toHaveBeenCalledTimes(1)
	})

	it("only the failing field falls back", async () => {
		const c = client(async (q) => {
			if (q.query === "bad") throw new Error("boom")
			return [A]
		})
		const {out} = await inject(`{ a: search(query: "good") { id } b: search(query: "bad") { id } }`, c)
		const printed = print(out)
		expect(printed.match(/ids: \[/g)).toHaveLength(1)
		expect(printed).toMatch(/a: search\(query: "good",? ids:/)
	})
})
