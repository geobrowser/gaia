import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import {conformingId} from "../../geodb/card"
import {readGatewayConfig} from "../config"
import {createGateway, type ResolveContext} from "../gateway"
import {createSearchResolver, planSearchServe, type SearchServeDeps} from "../searchServe"
import {__resetServeMetricsForTests, renderServeMetrics} from "../serveMetrics"

const SPACE = "11111111-1111-4111-8111-111111111111"
const GEO_SPACE = "22222222222242228222222222222222"
const GEO_SPACE_2 = "33333333333343338333333333333333"
const hex = (n: number) => `${String(n).padStart(8, "0")}00004000800000000000${String(n).padStart(4, "0")}`
const A = hex(1)
const B = hex(2)
const C = hex(3)
const D = hex(4)

const context = (field: string, query: string, variables: Record<string, unknown> = {}): ResolveContext => ({
	classification: {operationName: null, operationType: "query", rootFields: [field]},
	params: {query, operationName: null, variables},
	document: parse(query),
	signal: new AbortController().signal,
})

type Call = {geoSpaceIds: string[]; text: string; mode: string; limit: number; everySpace?: boolean}
const identity = async (ids: readonly string[]) => new Map(ids.map((id) => [id, id]))

/** `bm25` and `typeahead` are the hits each mode returns, in order. */
function make(bm25: string[], typeahead: string[] = [], overrides: Partial<SearchServeDeps> = {}) {
	const calls: Call[] = []
	const searchLexical = vi.fn(async (args: Call) => {
		calls.push(args)
		return args.mode === "bm25" ? bm25 : typeahead
	})
	const readCards = vi.fn(async () => [] as never[])
	const spaceOf = async (space: string) => (space === SPACE.replaceAll("-", "") ? GEO_SPACE : null)
	const geoSpaceOf = Object.assign(spaceOf, {all: async () => [GEO_SPACE, GEO_SPACE_2]})
	const deps: SearchServeDeps = {
		search: {searchLexical},
		geoSpaceOf,
		gaiaIdsOf: identity,
		readCards: readCards as unknown as SearchServeDeps["readCards"],
		enabled: true,
		...overrides,
	}
	return {deps, calls, searchLexical, readCards}
}

const S = `{ search(query: "ada lovelace", spaceId: "${SPACE}", first: 3) { id } }`
const C_ = `{ searchConnection(query: "ada lovelace", spaceId: "${SPACE}", first: 3) { nodes { id } } }`

describe("planSearchServe", () => {
	const reason = (field: "search" | "searchConnection", query: string, variables: Record<string, unknown> = {}) => {
		try {
			planSearchServe(field, parse(query), null, variables)
			return "ok"
		} catch (error) {
			return (error as {reason?: string}).reason
		}
	}
	it("accepts the shapes the shadow compares, aliases and variables included", () => {
		expect(reason("search", S)).toBe("ok")
		expect(reason("searchConnection", C_)).toBe("ok")
		expect(
			reason("search", `query Q($q: String!) { s: search(query: $q, first: 5) { i: id __typename } }`, {q: "ada"}),
		).toBe("ok")
	})
	it("names the gap for every other shape", () => {
		expect(reason("search", `{ search(query: "ada", first: 3) { id name } }`)).toBe("selection")
		expect(reason("search", `{ search(query: "ada", first: 3) { name } }`)).toBe("selection")
		expect(reason("searchConnection", `{ searchConnection(query: "ada", first: 3) { totalCount nodes { id } } }`)).toBe(
			"selection",
		)
		expect(reason("searchConnection", `{ searchConnection(query: "ada", first: 3) { nodes { id name } } }`)).toBe(
			"selection",
		)
		expect(reason("search", `{ search(query: "ada", first: 3, offset: 2) { id } }`)).toBe("args")
		expect(reason("search", `{ search(query: "ada", first: 3, similarityThreshold: 0.5) { id } }`)).toBe("args")
		expect(reason("search", `{ search(query: "ada") { id } }`)).toBe("args")
		expect(reason("search", `{ search(query: "ada", first: 0) { id } }`)).toBe("args")
		expect(reason("search", `{ search(query: "a", first: 3) { id } }`)).toBe("args")
		expect(reason("search", `{ search(query: "-ada", first: 3) { id } }`)).toBe("args")
		expect(reason("search", `{ search(query: "ada", spaceId: "nope", first: 3) { id } }`)).toBe("args")
		expect(reason("searchConnection", `{ searchConnection(query: "ada", first: 3, filter: {x: 1}) { nodes { id } } }`)).toBe(
			"filter",
		)
		expect(reason("search", `{ search(query: "ada", first: 3) { id } spaces { id } }`)).toBe("not_single_root")
		expect(reason("search", `{ __typename search(query: "ada", first: 3) { id } }`)).toBe("not_single_root")
		expect(reason("search", `{ search(query: "ada", first: 3) @skip(if: false) { id } }`)).toBe("directives")
	})
})

describe("createSearchResolver", () => {
	it("serves bm25 hits as gaia ids, keeping geo-db's order", async () => {
		const {deps, calls} = make([A, B, C])
		const result = await createSearchResolver("search", deps)(context("search", S))
		expect(result).toEqual({ok: true, data: {search: [{id: A}, {id: B}, {id: C}]}})
		expect(calls[0]).toMatchObject({mode: "bm25", text: "ada lovelace", geoSpaceIds: [GEO_SPACE]})
		expect(calls).toHaveLength(1)
	})
	it("shapes a connection as nodes", async () => {
		const {deps} = make([A, B])
		const result = await createSearchResolver("searchConnection", deps)(context("searchConnection", C_))
		expect(result).toEqual({ok: true, data: {searchConnection: {nodes: [{id: A}, {id: B}]}}})
	})
	it("cuts the page to first", async () => {
		const {deps} = make([A, B, C, D])
		const result = await createSearchResolver("search", deps)(context("search", S))
		expect(result).toEqual({ok: true, data: {search: [{id: A}, {id: B}, {id: C}]}})
	})
	it("appends a typeahead on the final token below the bm25 rows when bm25 returns fewer than first", async () => {
		const {deps, calls} = make([A], [A, B, C, D])
		const result = await createSearchResolver("search", deps)(context("search", S))
		expect(result).toEqual({ok: true, data: {search: [{id: A}, {id: B}, {id: C}]}})
		expect(calls.map((c) => [c.mode, c.text])).toEqual([
			["bm25", "ada lovelace"],
			["typeahead", "lovelace"],
		])
	})
	it("does not ask typeahead when bm25 fills the page", async () => {
		const {deps, calls} = make([A, B, C])
		await createSearchResolver("search", deps)(context("search", S))
		expect(calls.map((c) => c.mode)).toEqual(["bm25"])
	})
	it("skips the fallback when the final token is unusable", async () => {
		const {deps, calls} = make([A], [B])
		const q = `{ search(query: "ada x", spaceId: "${SPACE}", first: 3) { id } }`
		const result = await createSearchResolver("search", deps)(context("search", q))
		expect(result).toEqual({ok: true, data: {search: [{id: A}]}})
		expect(calls.map((c) => c.mode)).toEqual(["bm25"])
	})
	it("drops excluded types before counting against first, so the fallback fills the gap", async () => {
		const excluded = conformingId("entity", "96f859ef-a1ca-4b22-9372-c86ad58b694b")
		const readCards = vi.fn(async (args: {entityIds: string[]}) =>
			args.entityIds.includes(B)
				? [{entity: B, prop: conformingId("entity", "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"), raw: excluded}]
				: [],
		)
		const {deps} = make([A, B], [C, D], {readCards: readCards as never})
		const result = await createSearchResolver("search", deps)(context("search", S))
		expect(result).toEqual({ok: true, data: {search: [{id: A}, {id: C}, {id: D}]}})
	})
	it("searches every Space when none is named", async () => {
		const {deps, calls} = make([A])
		const q = `{ search(query: "ada", first: 1) { id } }`
		await createSearchResolver("search", deps)(context("search", q))
		expect(calls[0]?.geoSpaceIds).toEqual([GEO_SPACE, GEO_SPACE_2])
	})
	it("declines with a named reason and never reads geo-db when it cannot serve", async () => {
		const {deps, searchLexical} = make([A])
		const resolve = createSearchResolver("search", deps)
		expect(await resolve(context("search", `{ search(query: "ada", first: 3) { id name } }`))).toEqual({
			ok: false,
			reason: "selection",
		})
		expect(searchLexical).not.toHaveBeenCalled()
		const off = make([A], [], {enabled: false})
		expect(await createSearchResolver("search", off.deps)(context("search", S))).toEqual({
			ok: false,
			reason: "not_enabled",
		})
		expect(off.searchLexical).not.toHaveBeenCalled()
		const noCards = make([A], [], {readCards: undefined})
		expect(await createSearchResolver("search", noCards.deps)(context("search", S))).toEqual({
			ok: false,
			reason: "not_enabled",
		})
		const noMap = make([A], [], {gaiaIdsOf: undefined})
		expect(await createSearchResolver("search", noMap.deps)(context("search", S))).toEqual({
			ok: false,
			reason: "legacy_id",
		})
	})
	it("declines an unmapped Space, an empty answer, and an id Postgres cannot place", async () => {
		const other = `{ search(query: "ada", spaceId: "44444444-4444-4444-8444-444444444444", first: 3) { id } }`
		expect(await createSearchResolver("search", make([A]).deps)(context("search", other))).toEqual({
			ok: false,
			reason: "unmapped_space",
		})
		expect(await createSearchResolver("search", make([]).deps)(context("search", S))).toEqual({
			ok: false,
			reason: "empty",
		})
		const partial = make([A, B], [], {gaiaIdsOf: async (ids) => new Map(ids.filter((i) => i === A).map((i) => [i, i]))})
		expect(await createSearchResolver("search", partial.deps)(context("search", S))).toEqual({
			ok: false,
			reason: "legacy_id",
		})
	})
	it("declines no_space for an empty Space map and many_spaces past geo-db's bound", async () => {
		const q = `{ search(query: "ada", first: 3) { id } }`
		const none = make([A])
		none.deps.geoSpaceOf = Object.assign(async () => null, {all: async () => []})
		expect(await createSearchResolver("search", none.deps)(context("search", q))).toEqual({
			ok: false,
			reason: "no_space",
		})
		const many = make([A])
		many.deps.geoSpaceOf = Object.assign(async () => null, {
			all: async () => Array.from({length: 4097}, (_, i) => hex(i).slice(0, 32)),
		})
		expect(await createSearchResolver("search", many.deps)(context("search", q))).toEqual({
			ok: false,
			reason: "many_spaces",
		})
	})
	it("lets a geo-db failure through for the gateway to count", async () => {
		const deps = make([A]).deps
		deps.search = {searchLexical: async () => Promise.reject(Object.assign(new Error("x"), {reason: "overloaded"}))}
		await expect(createSearchResolver("search", deps)(context("search", S))).rejects.toMatchObject({
			reason: "overloaded",
		})
	})
})

describe("search=on through the gateway", () => {
	const post = (query: string) =>
		new Request("http://api/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query}),
		})
	const gateway = (hits: string[]) => {
		__resetServeMetricsForTests()
		const delegate = vi.fn(async () => new Response("pg"))
		const resolvers = new Map([["search", createSearchResolver("search", make(hits).deps)]])
		const config = readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "search=on"})
		return {delegate, run: createGateway({config, delegate, resolvers})}
	}
	it("answers from geo-db and counts it served", async () => {
		const {delegate, run} = gateway([A, B, C])
		const response = await run(post(S), {})
		expect(response.headers.get("x-gaia-source")).toBe("geodb")
		expect(await response.json()).toEqual({data: {search: [{id: A}, {id: B}, {id: C}]}})
		expect(delegate).not.toHaveBeenCalled()
		expect(renderServeMetrics()).toContain('gaia_api_gateway_served_total{field="search",source="geodb"} 1')
	})
	it("falls back to PostGraphile on a decline and counts the reason", async () => {
		const {delegate, run} = gateway([])
		const response = await run(post(S), {})
		expect(await response.text()).toBe("pg")
		expect(delegate).toHaveBeenCalledTimes(1)
		expect(renderServeMetrics()).toContain('gaia_api_gateway_serve_fallback_total{field="search",reason="empty"} 1')
	})
})
