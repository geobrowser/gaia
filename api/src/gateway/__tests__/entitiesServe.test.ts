import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import {conformingId} from "../../geodb/card"
import {MAX_TYPE_SCAN} from "../../geodb/client"
import {readGatewayConfig} from "../config"
import {createEntitiesResolver, type EntitiesServeDeps, planEntitiesServe} from "../entitiesServe"
import type {ResolveContext} from "../gateway"
import {createGateway} from "../gateway"
import {__resetServeMetricsForTests, renderServeMetrics} from "../serveMetrics"

const SPACE = "25a8a6b8c33b4b6ab8f3c2e0a2b1c0d9"
const SPACE_2 = "35a8a6b8c33b4b6ab8f3c2e0a2b1c0d9"
const TYPE = "8f151ba4de204e3c9cb499ddf96f48f1"
const A = "1b9f649e38b24b9a8e3c1b6a0b3c3c3a"
const B = "2b9f649e38b24b9a8e3c1b6a0b3c3c3a"
const C = "3b9f649e38b24b9a8e3c1b6a0b3c3c3a"

const context = (query: string, variables: Record<string, unknown> = {}): ResolveContext => ({
	classification: {operationName: null, operationType: "query", rootFields: ["entities"]},
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

const Q = `{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { id __typename } }`

describe("planEntitiesServe", () => {
	const reason = (query: string, variables: Record<string, unknown> = {}) => {
		try {
			planEntitiesServe(parse(query), null, variables)
			return "ok"
		} catch (error) {
			return (error as {reason?: string}).reason
		}
	}
	it("accepts the typed, spaced, unordered page that selects only id", () => {
		expect(reason(Q)).toBe("ok")
		expect(
			reason(
				`{ entities(spaceIds: {in: ["${SPACE}", "${SPACE_2}"]}, typeIds: {in: ["${TYPE}"]}, first: 5, offset: 2) { id } }`,
			),
		).toBe("ok")
		expect(
			reason(`query Q($s: UUID!, $t: UUID!) { e: entities(spaceId: $s, typeId: $t, first: 3) { i: id } }`, {
				s: SPACE,
				t: TYPE,
			}),
		).toBe("ok")
	})
	it("names the gap for every other shape", () => {
		expect(reason(`{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { id name } }`)).toBe("selection")
		expect(
			reason(`{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10, orderBy: CREATED_AT_DESC) { id } }`),
		).toBe("args")
		expect(reason(`{ entities(typeId: "${TYPE}", first: 10) { id } }`)).toBe("no_space")
		expect(reason(`{ entities(spaceId: "${SPACE}", first: 10) { id } }`)).toBe("filter")
		expect(reason(`{ entities(spaceId: "${SPACE}", typeId: "${TYPE}") { id } }`)).toBe("args")
		expect(
			reason(
				`{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10, filter: {id: {in: ["${A}"]}}) { id } }`,
			),
		).toBe("filter")
		expect(reason(`{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { id } spaces { id } }`)).toBe(
			"not_single_root",
		)
		expect(reason(`{ __typename entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { id } }`)).toBe(
			"not_single_root",
		)
		expect(
			reason(`{ entitiesConnection(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { nodes { id } } }`),
		).toBe("not_single_root")
	})
})

describe("createEntitiesResolver", () => {
	it("serves the page from geo-db's type scan, in PostGraphile's shape", async () => {
		const {deps, readTypeScan} = make([A, B])
		expect(await createEntitiesResolver(deps)(context(Q))).toEqual({
			ok: true,
			data: {
				entities: [
					{id: A, __typename: "Entity"},
					{id: B, __typename: "Entity"},
				],
			},
		})
		expect(readTypeScan).toHaveBeenCalledWith({geoSpaceIds: [SPACE], typeIds: [TYPE], first: 10})
	})

	it("serves an empty page as an empty list", async () => {
		expect(await createEntitiesResolver(make([]).deps)(context(Q))).toEqual({ok: true, data: {entities: []}})
	})

	it("applies offset and first to the whole set", async () => {
		const q = `{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 3, offset: 1) { id } }`
		const {deps, readTypeScan} = make([A, B])
		expect(await createEntitiesResolver(deps)(context(q))).toEqual({ok: true, data: {entities: [{id: B}]}})
		expect(readTypeScan).toHaveBeenCalledWith(expect.objectContaining({first: 4}))
	})

	it("declines a page cut at its window, which legacy ids could make inexact", async () => {
		const q = `{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 2) { id } }`
		expect(await createEntitiesResolver(make([A, B]).deps)(context(q))).toEqual({ok: false, reason: "truncated"})
	})

	it("declines a window past geo-db's scan cap without reading", async () => {
		const q = `{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: ${MAX_TYPE_SCAN}, offset: 1) { id } }`
		const {deps, readTypeScan} = make([])
		expect(await createEntitiesResolver(deps)(context(q))).toEqual({ok: false, reason: "truncated"})
		expect(readTypeScan).not.toHaveBeenCalled()
	})

	it("maps legacy ids back and orders the whole set the way PostGraphile does, by gaia id", async () => {
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
		expect(await createEntitiesResolver(deps)(context(Q))).toEqual({
			ok: true,
			data: {
				entities: [
					{id: legacy, __typename: "Entity"},
					{id: B, __typename: "Entity"},
				],
			},
		})
	})

	it("declines an id Postgres cannot place, and a failed map read passes through", async () => {
		const unmapped = make([A, B], {
			gaiaIdsOf: async (ids) => new Map(ids.filter((i) => i !== B).map((i) => [i, i])),
		}).deps
		expect(await createEntitiesResolver(unmapped)(context(Q))).toEqual({ok: false, reason: "legacy_id"})
		const none = make([A], {gaiaIdsOf: undefined}).deps
		expect(await createEntitiesResolver(none)(context(Q))).toEqual({ok: false, reason: "legacy_id"})
		const failing = make([A], {
			gaiaIdsOf: async () => Promise.reject(Object.assign(new Error("x"), {reason: "id_map"})),
		}).deps
		await expect(createEntitiesResolver(failing)(context(Q))).rejects.toMatchObject({reason: "id_map"})
	})

	it("declines an unmapped Space, several Spaces without multiSpace, and a client without type scans", async () => {
		const unmappedSpace = make([A], {geoSpaceOf: async () => null})
		expect(await createEntitiesResolver(unmappedSpace.deps)(context(Q))).toEqual({
			ok: false,
			reason: "unmapped_space",
		})
		const many = `{ entities(spaceIds: {in: ["${SPACE}", "${SPACE_2}"]}, typeId: "${TYPE}", first: 10) { id } }`
		const single = make([A], {multiSpace: false})
		expect(await createEntitiesResolver(single.deps)(context(many))).toEqual({ok: false, reason: "no_space"})
		expect(single.readTypeScan).not.toHaveBeenCalled()
		const off = make([A], {typeScan: false})
		expect(await createEntitiesResolver(off.deps)(context(Q))).toEqual({ok: false, reason: "not_enabled"})
		expect(off.readTypeScan).not.toHaveBeenCalled()
	})

	it("declines an unsupported shape before reading geo-db", async () => {
		const {deps, readTypeScan} = make([A])
		const q = `{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { id name } }`
		expect(await createEntitiesResolver(deps)(context(q))).toEqual({ok: false, reason: "selection"})
		expect(readTypeScan).not.toHaveBeenCalled()
		void C
	})

	it("lets a geo-db failure through for the gateway to count", async () => {
		const deps = make([A]).deps
		deps.geoDb = {
			readTypeScan: async () => Promise.reject(Object.assign(new Error("x"), {reason: "timeout"})),
		} as never
		await expect(createEntitiesResolver(deps)(context(Q))).rejects.toMatchObject({reason: "timeout"})
	})
})

describe("entities=on through the gateway", () => {
	const post = (query: string) =>
		new Request("http://api/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query}),
		})
	const gateway = (scanned: string[]) => {
		__resetServeMetricsForTests()
		const delegate = vi.fn(async () => new Response("pg"))
		const resolvers = new Map([["entities", createEntitiesResolver(make(scanned).deps)]])
		const config = readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "entities=on"})
		return {delegate, run: createGateway({config, delegate, resolvers})}
	}

	it("answers a supported shape from geo-db without asking PostGraphile", async () => {
		const {delegate, run} = gateway([A])
		const response = await run(post(Q), {})
		expect(response.headers.get("x-gaia-source")).toBe("geodb")
		expect(await response.json()).toEqual({data: {entities: [{id: A, __typename: "Entity"}]}})
		expect(delegate).not.toHaveBeenCalled()
		expect(renderServeMetrics()).toContain('gaia_api_gateway_served_total{field="entities",source="geodb"} 1')
	})

	it("falls back to PostGraphile for any other shape and counts the reason", async () => {
		const {delegate, run} = gateway([A])
		const response = await run(
			post(`{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { id name } }`),
			{},
		)
		expect(await response.text()).toBe("pg")
		expect(delegate).toHaveBeenCalledTimes(1)
		expect(renderServeMetrics()).toContain(
			'gaia_api_gateway_serve_fallback_total{field="entities",reason="selection"} 1',
		)
	})
})
