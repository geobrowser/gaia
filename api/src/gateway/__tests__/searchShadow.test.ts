import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId} from "../../geodb/card"
import {GeoDbError} from "../../geodb/client"
import {createCliGeoDbSearchClient, parseSearchOutput} from "../../geodb/search"
import {normalizeUuid} from "../../utils/uuid"
import {readGatewayConfig} from "../config"
import {createGateway} from "../gateway"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {compareRanked, overlapBucket, resultCountBucket} from "../searchMetrics"
import {analyzeSearchOperation, createSearchShadows, extractRankedIds} from "../searchShadow"

const SPACE = "11111111-1111-4111-8111-111111111111"
const GEO_SPACE = "22222222222242228222222222222222"
const GEO_SPACE_2 = "33333333333343338333333333333333"
const ids = (n: number) =>
	Array.from({length: n}, (_, i) => `00000000-0000-4000-8000-${String(i + 1).padStart(12, "0")}`)
const geo = (id: string) => conformingId("entity", id)

const SEARCH = `query S($q: String!, $spaceId: UUID) { search(query: $q, spaceId: $spaceId, first: 10) { id name } }`
const CONNECTION = `query C($q: String!, $spaceId: UUID) { searchConnection(query: $q, spaceId: $spaceId, first: 10) { nodes { id } } }`

const analyze = (field: "search" | "searchConnection", query: string, vars: Record<string, unknown> = {}) =>
	analyzeSearchOperation(field, parse(query), null, {q: "ada", spaceId: SPACE, ...vars})

function setup(hits: string[] | Error, mapped = true, all: string[] = [GEO_SPACE, GEO_SPACE_2]) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const searchLexical = vi.fn(async () => {
		if (hits instanceof Error) throw hits
		return hits
	})
	const shadows = createSearchShadows({
		search: {searchLexical},
		allGeoSpaces: async () => all,
		geoSpaceOf: async (space) => (mapped && space === SPACE.replaceAll("-", "") ? GEO_SPACE : null),
		log,
	})
	return {log, searchLexical, shadows}
}

const run = (
	shadows: ReturnType<typeof setup>["shadows"],
	field: "search" | "searchConnection",
	query: string,
	body: unknown,
	vars: Record<string, unknown> = {q: "ada", spaceId: SPACE},
) =>
	shadows.get(field)?.({
		classification: {operationName: null, operationType: "query", rootFields: [field]},
		params: {query, operationName: null, variables: vars},
		document: parse(query),
		response: Response.json(body),
	})

const metric = (name: string) =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${name}{`) && !l.endsWith(" 0"))
			.map((l) => [
				l.slice(l.indexOf("{") + 1, l.lastIndexOf("}")).replaceAll('"', ""),
				Number(l.split(" ").pop()),
			]),
	)

beforeEach(() => __resetGatewayMetricsForTests())

describe("analyzeSearchOperation", () => {
	it("accepts search and searchConnection with a space", () => {
		expect(analyze("search", SEARCH).ok).toBe(true)
		expect(analyze("searchConnection", CONNECTION).ok).toBe(true)
	})

	it("accepts a search without a space, as unscoped", () => {
		const noSpace = SEARCH.replace("spaceId: $spaceId, ", "")
		expect(analyze("search", noSpace)).toMatchObject({ok: true, plan: {spaceId: null}})
		expect(analyze("search", SEARCH, {spaceId: null})).toMatchObject({ok: true, plan: {spaceId: null}})
		expect(analyze("search", SEARCH)).toMatchObject({ok: true, plan: {spaceId: normalizeUuid(SPACE)}})
	})

	it.each([
		["a filter", SEARCH.replace("first: 10", "first: 10, filter: {typeIds: {in: []}}"), "filter"],
		["an offset", SEARCH.replace("first: 10", "first: 10, offset: 3"), "args"],
		["a similarity threshold", SEARCH.replace("first: 10", "first: 10, similarityThreshold: 0.3"), "args"],
		["no id selected", SEARCH.replace("id name", "name"), "selection"],
		["a directive", SEARCH.replace("id name", "id @include(if: true) name"), "directives"],
	])("refuses %s", (_, query, reason) => {
		expect(analyze("search", query)).toEqual({ok: false, reason})
	})

	it("refuses a query geo-db cannot take, and a bad space", () => {
		expect(analyze("search", SEARCH, {q: "a"})).toEqual({ok: false, reason: "args"})
		expect(analyze("search", SEARCH, {q: "--limit"})).toEqual({ok: false, reason: "args"})
		expect(analyze("search", SEARCH, {q: "x".repeat(251)})).toEqual({ok: false, reason: "args"})
		expect(analyze("search", SEARCH, {spaceId: "nope"})).toEqual({ok: false, reason: "args"})
	})

	it("refuses the connection without nodes { id }, aliases twice, and another root", () => {
		expect(analyze("searchConnection", CONNECTION.replace("nodes { id }", "totalCount"))).toEqual({
			ok: false,
			reason: "selection",
		})
		expect(analyze("searchConnection", CONNECTION.replace("{ nodes { id } }", "{ nodes { name } }"))).toEqual({
			ok: false,
			reason: "selection",
		})
		expect(
			analyze(
				"search",
				"query S($q: String!, $spaceId: UUID) { a: search(query: $q, spaceId: $spaceId) { id } b: search(query: $q, spaceId: $spaceId) { id } }",
			),
		).toEqual({ok: false, reason: "not_single_root"})
		expect(analyze("search", CONNECTION)).toEqual({ok: false, reason: "not_single_root"})
	})
})

describe("ranking metrics", () => {
	it("buckets", () => {
		expect([0, 0.1, 0.5, 0.99, 1].map(overlapBucket)).toEqual(["0", "1-49", "50-99", "50-99", "100"])
		expect([0, 3, 9, 12, 40].map(resultCountBucket)).toEqual(["0", "1-4", "5-9", "10-19", "20+"])
	})

	it("measures overlap of PostGraphile's top ten in geo-db's top ten and top twenty", () => {
		const pg = ids(10)
		const geoList = [...ids(5), ...ids(30).slice(20, 25), ...ids(10).slice(5)]
		const v = compareRanked(pg, geoList)
		expect(v.outcome).toBe("compared")
		expect(v.top10Overlap).toBe(0.5)
		expect(v.recallInGeoDb).toBe(1)
		expect(v.top1Same).toBe(true)
		expect(compareRanked([], [])).toEqual({outcome: "both_empty"})
		expect(compareRanked([], ["a"])).toEqual({outcome: "postgres_empty"})
		expect(compareRanked(["a"], [])).toEqual({outcome: "geodb_empty"})
	})
})

describe("extractRankedIds", () => {
	it("reads a list and a connection in rank order, conformed, distinct", () => {
		const plan = {field: "search", key: "search", query: "ada", spaceId: SPACE} as const
		const [a, b] = ids(2) as [string, string]
		expect(extractRankedIds({data: {search: [{id: a}, {id: b}, {id: a}]}}, plan, "id")).toEqual([geo(a), geo(b)])
		expect(extractRankedIds({data: {search: {nodes: [{id: b}]}}}, plan, "id", "nodes")).toEqual([geo(b)])
		expect(extractRankedIds({errors: []}, plan, "id")).toBeNull()
		expect(extractRankedIds({data: {search: [{id: 1}]}}, plan, "id")).toBeNull()
	})
})

describe("search shadow", () => {
	it("records overlap and counts for a compared search, and asks geo-db for the query in the mapped space", async () => {
		const pg = ids(10)
		const {shadows, searchLexical} = setup([...pg.slice(0, 7), ...ids(30).slice(20, 23)].map(geo))
		await run(shadows, "search", SEARCH, {data: {search: pg.map((id) => ({id, name: "x"}))}})
		expect(searchLexical).toHaveBeenCalledWith({
			geoSpaceIds: [GEO_SPACE],
			text: "ada",
			mode: "typeahead",
			limit: 20,
		})
		expect(metric("gaia_api_gateway_shadow_search_total")).toEqual({
			"field=search,outcome=compared": 1,
		})
		expect(metric("gaia_api_gateway_shadow_search_overlap_total")).toEqual({
			"field=search,kind=top10,bucket=50-99": 1,
			"field=search,kind=recall_top20,bucket=50-99": 1,
		})
		expect(metric("gaia_api_gateway_shadow_search_top1_total")).toEqual({"field=search,result=same": 1})
		expect(metric("gaia_api_gateway_shadow_search_results_total")).toEqual({
			"field=search,source=postgres,bucket=10-19": 1,
			"field=search,source=geodb,bucket=10-19": 1,
		})
		expect(metric("gaia_api_gateway_shadow_total")).toEqual({})
	})

	it("compares a connection through nodes", async () => {
		const pg = ids(3)
		const {shadows} = setup(pg.map(geo))
		await run(shadows, "searchConnection", CONNECTION, {data: {searchConnection: {nodes: pg.map((id) => ({id}))}}})
		expect(metric("gaia_api_gateway_shadow_search_overlap_total")).toMatchObject({
			"field=searchConnection,kind=top10,bucket=100": 1,
		})
	})

	it("counts an empty side without an overlap figure", async () => {
		const {shadows} = setup([])
		await run(shadows, "search", SEARCH, {data: {search: [{id: ids(1)[0]}]}})
		expect(metric("gaia_api_gateway_shadow_search_total")).toEqual({"field=search,outcome=geodb_empty": 1})
		expect(metric("gaia_api_gateway_shadow_search_overlap_total")).toEqual({})
	})

	it("asks geo-db once over every mapped Space for a search without a space", async () => {
		const pg = ids(4)
		const {shadows, searchLexical, log} = setup(pg.map(geo))
		const query = SEARCH.replace("spaceId: $spaceId, ", "")
		await run(shadows, "search", query, {data: {search: pg.map((id) => ({id}))}}, {q: "ada"})
		expect(searchLexical).toHaveBeenCalledTimes(1)
		expect(searchLexical).toHaveBeenCalledWith({
			geoSpaceIds: [GEO_SPACE, GEO_SPACE_2],
			text: "ada",
			mode: "typeahead",
			limit: 20,
		})
		expect(metric("gaia_api_gateway_shadow_search_total")).toEqual({"field=search,outcome=compared": 1})
		expect(metric("gaia_api_gateway_shadow_total")).toEqual({})
		expect(log.info).toHaveBeenCalledWith(
			expect.stringContaining("compared"),
			expect.objectContaining({spaceId: null, spaces: 2}),
		)
	})

	it("counts an unscoped search unsupported (no_space) when the space map is empty", async () => {
		const {shadows, searchLexical} = setup([], true, [])
		await run(shadows, "search", SEARCH.replace("spaceId: $spaceId, ", ""), {data: {search: []}}, {q: "ada"})
		expect(searchLexical).not.toHaveBeenCalled()
		expect(metric("gaia_api_gateway_shadow_total")).toEqual({"field=search,result=unsupported_selection": 1})
		expect(metric("gaia_api_gateway_shadow_unsupported_total")).toEqual({"field=search,reason=no_space": 1})
	})

	it("counts failed when geo-db errors or the space is not loaded, and logs", async () => {
		const down = setup(new GeoDbError("timeout", "slow"))
		await run(down.shadows, "search", SEARCH, {data: {search: []}})
		const unmapped = setup([], false)
		await run(unmapped.shadows, "search", SEARCH, {data: {search: []}})
		expect(metric("gaia_api_gateway_shadow_total")).toEqual({"field=search,result=failed": 2})
		expect(down.log.warn).toHaveBeenCalled()
		expect(metric("gaia_api_gateway_shadow_search_total")).toEqual({})
	})

	it("skips an answer that is not a search result", async () => {
		const {shadows, searchLexical, log} = setup([])
		await run(shadows, "search", SEARCH, {errors: [{message: "boom"}]})
		expect(searchLexical).not.toHaveBeenCalled()
		expect(log.info).toHaveBeenCalled()
	})

	it("runs off the response path through the gateway and never changes the response", async () => {
		const {shadows, searchLexical} = setup(new Error("geo-db down"))
		const handler = createGateway({
			config: readGatewayConfig({
				GRAPHQL_GATEWAY: "on",
				GRAPHQL_GATEWAY_FIELDS: "search=shadow",
				GRAPHQL_GATEWAY_SHADOW_SAMPLE: "1",
			}),
			shadows,
			delegate: async () => Response.json({data: {search: [{id: ids(1)[0], name: "x"}]}}),
		})
		const request = new Request("http://x/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query: SEARCH, variables: {q: "ada", spaceId: SPACE}}),
		})
		const response = await handler(request, {} as never)
		expect(await response.json()).toEqual({data: {search: [{id: ids(1)[0], name: "x"}]}})
		await new Promise((r) => setTimeout(r, 20))
		expect(searchLexical).toHaveBeenCalledTimes(1)
		expect(metric("gaia_api_gateway_shadow_total")).toEqual({"field=search,result=failed": 1})
	})
})

describe("geo-db search client", () => {
	const config = {
		mode: "shadow" as const,
		clientBin: "geodb-client",
		keyPath: "k",
		bindingPath: "b",
		spaceMapPath: "",
		timeoutMs: 500,
		poolSize: 1,
	}
	const e = (n: number) => n.toString(16).padStart(32, "0")

	it("parses hits in order, each entity once, ignoring partial lines", () => {
		expect(
			parseSearchOutput(`${e(1)}\t12\ts\tAda\t1\n${e(2)}\t9\ts\tBob\t1\n${e(1)}\t8\ts\tAda\t1\npartial: 3\n`),
		).toEqual([e(1), e(2)])
		expect(parseSearchOutput("")).toEqual([])
	})

	it("runs one search verb with the space, mode and limit, and refuses unusable input", async () => {
		const run = vi.fn(async () => `${e(5)}\t1\ts\tAda\t1\n`)
		const client = createCliGeoDbSearchClient(config, {run})
		expect(await client.searchLexical({geoSpaceIds: [GEO_SPACE], text: "ada", mode: "bm25", limit: 20})).toEqual([
			e(5),
		])
		expect(run).toHaveBeenCalledWith(
			config,
			["search", "--space", GEO_SPACE, "--mode", "bm25", "--limit", "20", "--first", "20", "--pages", "1", "ada"],
			3000,
		)
		await expect(
			client.searchLexical({geoSpaceIds: [GEO_SPACE], text: "-x", mode: "bm25", limit: 1}),
		).rejects.toMatchObject({reason: "bad_request"})
		await expect(
			client.searchLexical({geoSpaceIds: ["nope"], text: "x", mode: "bm25", limit: 1}),
		).rejects.toMatchObject({reason: "bad_request"})
	})

	it("sends one search with a --space per Space, each once, and bounds the list", async () => {
		const run = vi.fn(async () => "")
		const client = createCliGeoDbSearchClient(config, {run})
		await client.searchLexical({
			geoSpaceIds: [GEO_SPACE, GEO_SPACE_2, GEO_SPACE],
			text: "ada",
			mode: "typeahead",
			limit: 20,
		})
		expect(run).toHaveBeenCalledTimes(1)
		expect(run).toHaveBeenCalledWith(
			config,
			[
				"search",
				"--space",
				GEO_SPACE,
				"--space",
				GEO_SPACE_2,
				"--mode",
				"typeahead",
				"--limit",
				"20",
				"--first",
				"20",
				"--pages",
				"1",
				"ada",
			],
			3000,
		)
		await expect(client.searchLexical({geoSpaceIds: [], text: "x", mode: "bm25", limit: 1})).rejects.toMatchObject({
			reason: "bad_request",
		})
		await expect(
			client.searchLexical({geoSpaceIds: Array(1025).fill(GEO_SPACE), text: "x", mode: "bm25", limit: 1}),
		).rejects.toMatchObject({reason: "bad_request"})
	})

	it("refuses past the concurrency cap instead of queueing", async () => {
		const releases: ((v: string) => void)[] = []
		const run = vi.fn(
			() =>
				new Promise<string>((r) => {
					releases.push(r)
				}),
		)
		const client = createCliGeoDbSearchClient(config, {run, maxConcurrent: 1})
		const first = client.searchLexical({geoSpaceIds: [GEO_SPACE], text: "ada", mode: "typeahead", limit: 5})
		await expect(
			client.searchLexical({geoSpaceIds: [GEO_SPACE], text: "bob", mode: "typeahead", limit: 5}),
		).rejects.toMatchObject({reason: "overloaded"})
		releases[0]?.("")
		expect(await first).toEqual([])
	})
})
