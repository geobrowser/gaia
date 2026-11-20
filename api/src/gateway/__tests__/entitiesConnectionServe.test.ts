import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import {conformingId} from "../../geodb/card"
import {MAX_TYPE_SCAN} from "../../geodb/client"
import {readGatewayConfig} from "../config"
import {createEntitiesConnectionResolver, planEntitiesConnectionServe} from "../entitiesConnectionServe"
import type {EntitiesServeDeps} from "../entitiesServe"
import {createGateway, type ResolveContext} from "../gateway"
import {__resetServeMetricsForTests, renderServeMetrics} from "../serveMetrics"

const SPACE = "25a8a6b8c33b4b6ab8f3c2e0a2b1c0d9"
const SPACE_2 = "35a8a6b8c33b4b6ab8f3c2e0a2b1c0d9"
const TYPE = "8f151ba4de204e3c9cb499ddf96f48f1"
const A = "1b9f649e38b24b9a8e3c1b6a0b3c3c3a"
const B = "2b9f649e38b24b9a8e3c1b6a0b3c3c3a"

const context = (query: string, variables: Record<string, unknown> = {}): ResolveContext => ({
	classification: {operationName: null, operationType: "query", rootFields: ["entitiesConnection"]},
	params: {query, operationName: null, variables},
	document: parse(query),
	signal: new AbortController().signal,
})

const identity = async (ids: readonly string[]) => new Map(ids.map((id) => [id, id]))

const make = (scanned: string[], overrides: Partial<EntitiesServeDeps> = {}) => {
	const readTypeScan = vi.fn(async (_args: {geoSpaceIds: string[]; typeIds: string[]; first: number}) => scanned)
	const deps: EntitiesServeDeps = {
		geoDb: {readTypeScan} as unknown as EntitiesServeDeps["geoDb"],
		geoSpaceOf: async (s) => s,
		gaiaIdsOf: identity,
		typeScan: true,
		multiSpace: true,
		...overrides,
	}
	return {deps, readTypeScan}
}

const Q = `{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { nodes { id __typename } } }`

describe("planEntitiesConnectionServe", () => {
	const reason = (query: string, variables: Record<string, unknown> = {}) => {
		try {
			planEntitiesConnectionServe(parse(query), null, variables)
			return "ok"
		} catch (error) {
			return (error as {reason?: string}).reason
		}
	}

	it("accepts the typed, spaced, unordered connection page selecting nodes { id __typename }", () => {
		expect(reason(Q)).toBe("ok")
		expect(
			reason(
				`{ entitiesConnection(spaceIds: {in: ["${SPACE}", "${SPACE_2}"]}, typeIds: {in: ["${TYPE}"]}, first: 5, offset: 2) { __typename nodes { id } } }`,
			),
		).toBe("ok")
		expect(
			reason(
				`query Q($s: UUID!, $t: UUID!) { ec: entitiesConnection(spaceId: $s, typeId: $t, first: 3) { items: nodes { i: id } } }`,
				{s: SPACE, t: TYPE},
			),
		).toBe("ok")
	})

	it("declines totalCount, pageInfo, edges, extra node fields, unsupported args, and non-single roots", () => {
		expect(
			reason(
				`{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { totalCount nodes { id } } }`,
			),
		).toBe("selection")
		expect(
			reason(
				`{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { pageInfo { hasNextPage } nodes { id } } }`,
			),
		).toBe("selection")
		expect(
			reason(
				`{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { edges { node { id } } } }`,
			),
		).toBe("selection")
		expect(
			reason(`{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { nodes { id name } } }`),
		).toBe("selection")
		expect(
			reason(
				`{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10, after: "cur") { nodes { id } } }`,
			),
		).toBe("args")
		expect(
			reason(
				`{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10, orderBy: CREATED_AT_DESC) { nodes { id } } }`,
			),
		).toBe("args")
		expect(reason(`{ entitiesConnection(typeId: "${TYPE}", first: 10) { nodes { id } } }`)).toBe("no_space")
		expect(reason(`{ entitiesConnection(spaceId: "${SPACE}", first: 10) { nodes { id } } }`)).toBe("filter")
		expect(
			reason(
				`{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10, filter: {id: {in: ["${A}"]}}) { nodes { id } } }`,
			),
		).toBe("filter")
		expect(reason(`{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { id } }`)).toBe("not_single_root")
		expect(
			reason(
				`{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { nodes { id } } spaces { id } }`,
			),
		).toBe("not_single_root")
	})
})

describe("createEntitiesConnectionResolver", () => {
	it("serves the connection page from geo-db's type scan, in PostGraphile's shape", async () => {
		const {deps, readTypeScan} = make([A, B])
		expect(await createEntitiesConnectionResolver(deps)(context(Q))).toEqual({
			ok: true,
			data: {
				entitiesConnection: {
					nodes: [
						{id: A, __typename: "Entity"},
						{id: B, __typename: "Entity"},
					],
				},
			},
		})
		expect(readTypeScan).toHaveBeenCalledWith({geoSpaceIds: [SPACE], typeIds: [TYPE], first: 10})
	})

	it("preserves aliases and optional connection __typename", async () => {
		const q = `{ ec: entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { t: __typename items: nodes { i: id } } }`
		expect(await createEntitiesConnectionResolver(make([A]).deps)(context(q))).toEqual({
			ok: true,
			data: {
				ec: {
					t: "EntitiesConnection",
					items: [{i: A}],
				},
			},
		})
	})

	it("applies offset/first and declines truncated pages or windows past MAX_TYPE_SCAN", async () => {
		const q = `{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 3, offset: 1) { nodes { id } } }`
		expect(await createEntitiesConnectionResolver(make([A, B]).deps)(context(q))).toEqual({
			ok: true,
			data: {entitiesConnection: {nodes: [{id: B}]}},
		})

		const cut = `{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 2) { nodes { id } } }`
		expect(await createEntitiesConnectionResolver(make([A, B]).deps)(context(cut))).toEqual({
			ok: false,
			reason: "truncated",
		})

		const huge = `{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: ${MAX_TYPE_SCAN}, offset: 1) { nodes { id } } }`
		const {deps, readTypeScan} = make([])
		expect(await createEntitiesConnectionResolver(deps)(context(huge))).toEqual({ok: false, reason: "truncated"})
		expect(readTypeScan).not.toHaveBeenCalled()
	})

	it("maps legacy ids back and sorts by gaia id", async () => {
		const legacy = "0a0a4147a92855e88e630e542faf1c72"
		const geo = conformingId("entity", legacy)
		const back = new Map([
			[geo, legacy],
			[B, B],
		])
		const {deps} = make([B, geo], {
			gaiaIdsOf: async (ids) =>
				new Map(ids.flatMap((i) => (back.has(i) ? [[i, back.get(i) as string] as const] : []))),
		})
		expect(await createEntitiesConnectionResolver(deps)(context(Q))).toEqual({
			ok: true,
			data: {
				entitiesConnection: {
					nodes: [
						{id: legacy, __typename: "Entity"},
						{id: B, __typename: "Entity"},
					],
				},
			},
		})
	})

	it("declines legacy_id, unmapped_space, no_space without multiSpace, and not_enabled without typeScan", async () => {
		expect(await createEntitiesConnectionResolver(make([A], {gaiaIdsOf: undefined}).deps)(context(Q))).toEqual({
			ok: false,
			reason: "legacy_id",
		})
		expect(
			await createEntitiesConnectionResolver(make([A], {geoSpaceOf: async () => null}).deps)(context(Q)),
		).toEqual({
			ok: false,
			reason: "unmapped_space",
		})
		const many = `{ entitiesConnection(spaceIds: {in: ["${SPACE}", "${SPACE_2}"]}, typeId: "${TYPE}", first: 10) { nodes { id } } }`
		expect(await createEntitiesConnectionResolver(make([A], {multiSpace: false}).deps)(context(many))).toEqual({
			ok: false,
			reason: "no_space",
		})
		expect(await createEntitiesConnectionResolver(make([A], {typeScan: false}).deps)(context(Q))).toEqual({
			ok: false,
			reason: "not_enabled",
		})
	})
})

describe("entitiesConnection=on through the gateway", () => {
	const post = (query: string) =>
		new Request("http://api/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query}),
		})
	const gateway = (scanned: string[]) => {
		__resetServeMetricsForTests()
		const delegate = vi.fn(async () => new Response("pg"))
		const resolvers = new Map([["entitiesConnection", createEntitiesConnectionResolver(make(scanned).deps)]])
		const config = readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "entitiesConnection=on"})
		return {delegate, run: createGateway({config, delegate, resolvers})}
	}

	it("answers a supported shape from geo-db without asking PostGraphile", async () => {
		const {delegate, run} = gateway([A])
		const response = await run(post(Q), {})
		expect(response.headers.get("x-gaia-source")).toBe("geodb")
		expect(await response.json()).toEqual({
			data: {entitiesConnection: {nodes: [{id: A, __typename: "Entity"}]}},
		})
		expect(delegate).not.toHaveBeenCalled()
		expect(renderServeMetrics()).toContain(
			'gaia_api_gateway_served_total{field="entitiesConnection",source="geodb"} 1',
		)
	})

	it("falls back to PostGraphile for totalCount and counts the reason", async () => {
		const {delegate, run} = gateway([A])
		const response = await run(
			post(
				`{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { totalCount nodes { id } } }`,
			),
			{},
		)
		expect(await response.text()).toBe("pg")
		expect(delegate).toHaveBeenCalledTimes(1)
		expect(renderServeMetrics()).toContain(
			'gaia_api_gateway_serve_fallback_total{field="entitiesConnection",reason="selection"} 1',
		)
	})
})
