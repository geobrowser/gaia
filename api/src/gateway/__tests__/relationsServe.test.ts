import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow} from "../../geodb/card"
import {readGatewayConfig} from "../config"
import {createGateway, type ResolveContext} from "../gateway"
import {createRelationsResolver, planRelationsServe, type RelationsServeDeps} from "../relationsServe"
import {__resetServeMetricsForTests, renderServeMetrics} from "../serveMetrics"

const FROM = "6b9f649e38b24b9a8e3c1b6a0b3c3c3a"
const SPACE = "25a8a6b8c33b4b6ab8f3c2e0a2b1c0d9"
const TYPE = "8f151ba4de204e3c9cb499ddf96f48f1"
const TYPE_2 = "7f151ba4de204e3c9cb499ddf96f48f1"
const T1 = "0d4f1a2b3c4d4e5f8a6b7c8d9e0f1a2b"
const T2 = "1d4f1a2b3c4d4e5f8a6b7c8d9e0f1a2b"
const MEMBER = "dbe3bdf637a5458d9206d737fb104793"

const SEL = "typeId toEntityId fromEntityId spaceId __typename"
const Q = `{ relations(filter: {fromEntityId: {is: "${FROM}"}, spaceId: {is: "${SPACE}"}, typeId: {in: ["${TYPE}", "${TYPE_2}"]}}, first: 50) { ${SEL} } }`

const context = (query: string, variables: Record<string, unknown> = {}): ResolveContext => ({
	classification: {operationName: null, operationType: "query", rootFields: ["relations"]},
	params: {query, operationName: null, variables},
	document: parse(query),
	signal: new AbortController().signal,
})
const identity = async (ids: readonly string[]) => new Map(ids.map((id) => [id, id]))

const make = (rows: GeoCardRow[], overrides: Partial<RelationsServeDeps> = {}) => {
	const readCard = vi.fn(async (_args: unknown) => rows)
	const deps: RelationsServeDeps = {
		geoDb: {readCard} as unknown as RelationsServeDeps["geoDb"],
		geoSpaceOf: async (s) => s,
		gaiaIdsOf: identity,
		...overrides,
	}
	return {deps, readCard}
}

describe("planRelationsServe", () => {
	const reason = (query: string, variables: Record<string, unknown> = {}) => {
		try {
			planRelationsServe(parse(query), null, variables)
			return "ok"
		} catch (error) {
			return (error as {reason?: string}).reason
		}
	}
	const f = (filter: string, rest = "first: 50", sel = SEL) =>
		`{ relations(filter: {${filter}}, ${rest}) { ${sel} } }`
	const from = `fromEntityId: {is: "${FROM}"}`
	const space = `spaceId: {is: "${SPACE}"}`
	const type = `typeId: {is: "${TYPE}"}`

	it("accepts one from entity's relations of some types in one space", () => {
		expect(reason(Q)).toBe("ok")
		expect(reason(f(`${from}, ${space}, ${type}`))).toBe("ok")
		expect(reason(f(`${from}, ${space}, typeId: {equalTo: "${TYPE}"}`, "first: 5", "t: typeId"))).toBe("ok")
		expect(
			reason(
				`query Q($e: UUID!, $s: UUID!, $t: UUID!) { r: relations(filter: {fromEntityId: {is: $e}, spaceId: {is: $s}, typeId: {is: $t}}, first: 9) { toEntityId } }`,
				{e: FROM, s: SPACE, t: TYPE},
			),
		).toBe("ok")
	})
	it("names the gap for every other shape", () => {
		expect(reason(f(`${from}, ${space}, ${type}`, "first: 50", "id"))).toBe("selection")
		expect(reason(f(`${from}, ${space}, ${type}`, "first: 50", "position"))).toBe("selection")
		expect(reason(f(`${from}, ${space}`))).toBe("filter")
		expect(reason(f(`${from}, ${type}`))).toBe("no_space")
		expect(reason(f(`${space}, ${type}`))).toBe("filter")
		expect(reason(f(`${from}, ${space}, ${type}, toEntityId: {is: "${T1}"}`))).toBe("filter")
		expect(reason(f(`${from}, spaceId: {in: ["${SPACE}"]}, ${type}`))).toBe("no_space")
		expect(reason(f(`${from}, ${space}, ${type}`, "first: 50, offset: 5"))).toBe("args")
		expect(reason(f(`${from}, ${space}, ${type}`, "first: 50, orderBy: ID_DESC"))).toBe("args")
		expect(reason(f(`${from}, ${space}, ${type}`, "first: 0"))).toBe("args")
		expect(reason(f(`${from}, ${space}, ${type}`, 'after: "x"'))).toBe("args")
		expect(reason(`{ relations(first: 5) { toEntityId } }`)).toBe("filter")
		expect(reason(`{ relations(filter: {id: {in: ["${T1}"]}}) { toEntityId } }`)).toBe("filter")
		expect(reason(`{ relationsConnection(first: 5) { nodes { toEntityId } } }`)).toBe("not_single_root")
	})
})

describe("createRelationsResolver", () => {
	it("serves the typed edges of the card in PostGraphile's shape, skipping other types, values and membership", async () => {
		const {deps, readCard} = make([
			{prop: TYPE, raw: T1},
			{prop: TYPE_2, raw: T2},
			{prop: "6f151ba4de204e3c9cb499ddf96f48f1", raw: T1},
			{prop: TYPE, raw: "plain text"},
			{prop: MEMBER, raw: T1},
		])
		expect(await createRelationsResolver(deps)(context(Q))).toEqual({
			ok: true,
			data: {
				relations: [
					{typeId: TYPE, toEntityId: T1, fromEntityId: FROM, spaceId: SPACE, __typename: "Relation"},
					{typeId: TYPE_2, toEntityId: T2, fromEntityId: FROM, spaceId: SPACE, __typename: "Relation"},
				],
			},
		})
		expect(readCard).toHaveBeenCalledWith(expect.objectContaining({entityId: conformingId("entity", FROM)}))
	})

	it("serves an entity that holds none of the types as an empty list only when the card is not empty", async () => {
		expect(await createRelationsResolver(make([{prop: MEMBER, raw: T1}]).deps)(context(Q))).toEqual({
			ok: true,
			data: {relations: []},
		})
		expect(await createRelationsResolver(make([]).deps)(context(Q))).toEqual({ok: false, reason: "empty"})
	})

	it("declines a page that may have been cut short", async () => {
		const q = Q.replace("first: 50", "first: 1")
		const {deps} = make([{prop: TYPE, raw: T1}])
		expect(await createRelationsResolver(deps)(context(q))).toEqual({ok: false, reason: "truncated"})
	})

	it("declines derived and membership relation types, which geo-db does not hold as edges", async () => {
		const derived = Q.replace(TYPE, "890deffb384349fa826974000e3dcef6")
		const {deps, readCard} = make([{prop: TYPE, raw: T1}])
		expect(await createRelationsResolver(deps)(context(derived))).toEqual({ok: false, reason: "selection"})
		expect(readCard).not.toHaveBeenCalled()
		expect(await createRelationsResolver(deps)(context(Q.replace(TYPE, MEMBER)))).toEqual({
			ok: false,
			reason: "selection",
		})
	})

	it("maps legacy ids back and declines what Postgres cannot place", async () => {
		const legacyFrom = "990a4147a92855e88e630e542faf1c72"
		const legacyTarget = "01a1a8c0000070008000000000000001"
		const back = new Map([
			[conformingId("entity", legacyFrom), legacyFrom],
			[conformingId("entity", legacyTarget), legacyTarget],
			[TYPE, TYPE],
			[TYPE_2, TYPE_2],
		])
		const gaiaIdsOf = async (ids: readonly string[]) =>
			new Map(ids.flatMap((id) => (back.has(id) ? [[id, back.get(id) as string] as const] : [])))
		const q = Q.replace(FROM, legacyFrom)
		const {deps} = make([{prop: TYPE, raw: conformingId("entity", legacyTarget)}], {gaiaIdsOf})
		expect(await createRelationsResolver(deps)(context(q))).toEqual({
			ok: true,
			data: {
				relations: [
					{
						typeId: TYPE,
						toEntityId: legacyTarget,
						fromEntityId: legacyFrom,
						spaceId: SPACE,
						__typename: "Relation",
					},
				],
			},
		})
		const unmapped = make([{prop: TYPE, raw: T1}], {
			gaiaIdsOf: async (ids) => new Map(ids.filter((i) => i !== T1).map((i) => [i, i])),
		})
		expect(await createRelationsResolver(unmapped.deps)(context(Q))).toEqual({ok: false, reason: "legacy_id"})
		// The asked from entity must come back as itself.
		const wrongFrom = make([{prop: TYPE, raw: T1}], {
			gaiaIdsOf: async (ids) => new Map(ids.map((i) => [i, i === FROM ? legacyFrom : i])),
		})
		expect(await createRelationsResolver(wrongFrom.deps)(context(Q))).toEqual({ok: false, reason: "legacy_id"})
		const none = make([{prop: TYPE, raw: T1}], {gaiaIdsOf: undefined})
		expect(await createRelationsResolver(none.deps)(context(Q))).toEqual({ok: false, reason: "legacy_id"})
		expect(none.readCard).not.toHaveBeenCalled()
	})

	it("lets geo-db and map failures through for the gateway to count", async () => {
		const failing = make([], {
			geoDb: {readCard: async () => Promise.reject(Object.assign(new Error("x"), {reason: "timeout"}))} as never,
		}).deps
		await expect(createRelationsResolver(failing)(context(Q))).rejects.toMatchObject({reason: "timeout"})
		const map = make([{prop: TYPE, raw: T1}], {
			gaiaIdsOf: async () => Promise.reject(Object.assign(new Error("x"), {reason: "id_map"})),
		}).deps
		await expect(createRelationsResolver(map)(context(Q))).rejects.toMatchObject({reason: "id_map"})
	})
})

describe("relations=on through the gateway", () => {
	const post = (query: string) =>
		new Request("http://api/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query}),
		})
	const gateway = () => {
		__resetServeMetricsForTests()
		const delegate = vi.fn(async () => new Response("pg"))
		const resolvers = new Map([["relations", createRelationsResolver(make([{prop: TYPE, raw: T1}]).deps)]])
		const config = readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "relations=on"})
		return {delegate, run: createGateway({config, delegate, resolvers})}
	}
	it("answers a supported shape from geo-db without asking PostGraphile", async () => {
		const {delegate, run} = gateway()
		const response = await run(post(Q), {})
		expect(response.headers.get("x-gaia-source")).toBe("geodb")
		expect(delegate).not.toHaveBeenCalled()
		expect(renderServeMetrics()).toContain('gaia_api_gateway_served_total{field="relations",source="geodb"} 1')
	})
	it("falls back to PostGraphile for any other shape and counts the reason", async () => {
		const {delegate, run} = gateway()
		const response = await run(post(Q.replace("first: 50", "first: 50, offset: 5")), {})
		expect(await response.text()).toBe("pg")
		expect(delegate).toHaveBeenCalledTimes(1)
		expect(renderServeMetrics()).toContain(
			'gaia_api_gateway_serve_fallback_total{field="relations",reason="args"} 1',
		)
	})
})
