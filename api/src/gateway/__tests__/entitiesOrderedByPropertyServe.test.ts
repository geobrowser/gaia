import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import {conformingId, type GeoPageRow} from "../../geodb/card"
import {type EntitiesPageRequest, MAX_PAGE} from "../../geodb/client"
import {readGatewayConfig} from "../config"
import {
	createEntitiesOrderedByPropertyResolver,
	type EntitiesOrderedByPropertyServeDeps,
	planEntitiesOrderedByPropertyServe,
} from "../entitiesOrderedByPropertyServe"
import type {ResolveContext} from "../gateway"
import {createGateway} from "../gateway"
import {__resetServeMetricsForTests, renderServeMetrics} from "../serveMetrics"

const SPACE = "25a8a6b8c33b4b6ab8f3c2e0a2b1c0d9"
const SPACE_2 = "35a8a6b8c33b4b6ab8f3c2e0a2b1c0d9"
const TYPE = "8f151ba4de204e3c9cb499ddf96f48f1"
const TYPE_2 = "9f151ba4de204e3c9cb499ddf96f48f1"
const PROP = "000000009962496299620000000000bb"
const OTHER_PROP = "111111119962496299620000000000bb"
const A = "1b9f649e38b24b9a8e3c1b6a0b3c3c3a"
const B = "2b9f649e38b24b9a8e3c1b6a0b3c3c3a"
const C = "3b9f649e38b24b9a8e3c1b6a0b3c3c3a"

const context = (
	query: string,
	variables: Record<string, unknown> = {},
	signal: AbortSignal = new AbortController().signal,
): ResolveContext => ({
	classification: {operationName: null, operationType: "query", rootFields: ["entitiesOrderedByProperty"]},
	params: {query, operationName: null, variables},
	document: parse(query),
	signal,
})

const identity = async (ids: readonly string[]) => new Map(ids.map((id) => [id, id]))

const row = (entity: string, raw: string, prop: string = PROP): GeoPageRow => ({
	entity,
	prop: conformingId("entity", prop),
	raw,
})

const make = (rows: GeoPageRow[], overrides: Partial<EntitiesOrderedByPropertyServeDeps> = {}) => {
	const readEntitiesPage = vi.fn(async (_args: EntitiesPageRequest) => rows)
	const deps: EntitiesOrderedByPropertyServeDeps = {
		geoDb: {readEntitiesPage} as unknown as EntitiesOrderedByPropertyServeDeps["geoDb"],
		geoSpaceOf: async (s) => s,
		gaiaIdsOf: identity,
		orderPick: true,
		...overrides,
	}
	return {deps, readEntitiesPage}
}

const Q = `{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", sortDirection: DESC, first: 10) { id __typename } }`

describe("planEntitiesOrderedByPropertyServe", () => {
	const reason = (query: string, variables: Record<string, unknown> = {}) => {
		try {
			planEntitiesOrderedByPropertyServe(parse(query), null, variables)
			return "ok"
		} catch (error) {
			return (error as {reason?: string}).reason
		}
	}

	it("accepts the single-Space, single-Type, exact-order page that selects only id and __typename", () => {
		expect(reason(Q)).toBe("ok")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: "${SPACE}", typeIds: "${TYPE}", dataType: "INTEGER", first: 5, offset: 2) { id } }`,
			),
		).toBe("ok")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}", "${SPACE}"], typeIds: ["${TYPE}", "${TYPE}"], dataType: "datetime", includeWithoutValue: false, first: 5) { id } }`,
			),
		).toBe("ok")
		expect(
			reason(
				`query Q($p: UUID!, $s: [UUID!], $t: [UUID!]) { e: entitiesOrderedByProperty(propertyId: $p, spaceIds: $s, typeIds: $t, dataType: "decimal", first: 3) { i: id } }`,
				{p: PROP, s: [SPACE], t: [TYPE]},
			),
		).toBe("ok")
		for (const dt of ["integer", "float", "decimal", "boolean", "date", "datetime"]) {
			expect(
				reason(
					`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "${dt}", first: 5) { id } }`,
				),
			).toBe("ok")
		}
	})

	it("defaults sortDirection to asc and offset to 0", () => {
		const plan = planEntitiesOrderedByPropertyServe(
			parse(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", first: 5) { id } }`,
			),
			null,
			{},
		)
		expect(plan.direction).toBe("asc")
		expect(plan.offset).toBe(0)
	})

	it("names the gap for every other shape", () => {
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", first: 10) { id name } }`,
			),
		).toBe("selection")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", typeIds: ["${TYPE}"], dataType: "integer", first: 10) { id } }`,
			),
		).toBe("no_space")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: [], typeIds: ["${TYPE}"], dataType: "integer", first: 10) { id } }`,
			),
		).toBe("no_space")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}", "${SPACE_2}"], typeIds: ["${TYPE}"], dataType: "integer", first: 10) { id } }`,
			),
		).toBe("many_spaces")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], dataType: "integer", first: 10) { id } }`,
			),
		).toBe("filter")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: [], dataType: "integer", first: 10) { id } }`,
			),
		).toBe("filter")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}", "${TYPE_2}"], dataType: "integer", first: 10) { id } }`,
			),
		).toBe("args")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], first: 10) { id } }`,
			),
		).toBe("args")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "text", first: 10) { id } }`,
			),
		).toBe("args")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "point", first: 10) { id } }`,
			),
		).toBe("args")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", includeWithoutValue: true, first: 10) { id } }`,
			),
		).toBe("args")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", sortDirection: SIDEWAYS, first: 10) { id } }`,
			),
		).toBe("args")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer") { id } }`,
			),
		).toBe("args")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", first: 10, votedBy: "${A}") { id } }`,
			),
		).toBe("filter")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", first: 10, entityIds: ["${A}"]) { id } }`,
			),
		).toBe("filter")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", first: 10, filter: {id: {in: ["${A}"]}}) { id } }`,
			),
		).toBe("filter")
		expect(
			reason(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", first: 10) { id } spaces { id } }`,
			),
		).toBe("not_single_root")
		expect(
			reason(
				`{ __typename entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", first: 10) { id } }`,
			),
		).toBe("not_single_root")
		expect(reason(`{ entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: 10) { id } }`)).toBe("not_single_root")
	})
})

describe("createEntitiesOrderedByPropertyResolver", () => {
	it("serves the page from geo-db's property-ordered page, in PostGraphile's shape", async () => {
		const {deps, readEntitiesPage} = make([row(A, "10"), row(B, "25")])
		expect(await createEntitiesOrderedByPropertyResolver(deps)(context(Q))).toEqual({
			ok: true,
			data: {
				entitiesOrderedByProperty: [
					{id: B, __typename: "Entity"},
					{id: A, __typename: "Entity"},
				],
			},
		})
		expect(readEntitiesPage).toHaveBeenCalledWith({
			geoSpaceIds: [SPACE],
			typeId: TYPE,
			field: PROP,
			direction: "desc",
			pick: "greatest",
			first: 10,
		})
	})

	it("picks least for asc and greatest for desc when an entity has multiple values", async () => {
		const ascQ = `{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", sortDirection: ASC, first: 10) { id } }`
		const rows = [row(A, "2"), row(A, "99"), row(B, "5")]
		const asc = make(rows)
		expect(await createEntitiesOrderedByPropertyResolver(asc.deps)(context(ascQ))).toEqual({
			ok: true,
			data: {entitiesOrderedByProperty: [{id: A}, {id: B}]},
		})
		expect(asc.readEntitiesPage).toHaveBeenCalledWith(expect.objectContaining({direction: "asc", pick: "least"}))

		const desc = make(rows)
		expect(await createEntitiesOrderedByPropertyResolver(desc.deps)(context(Q))).toEqual({
			ok: true,
			data: {
				entitiesOrderedByProperty: [
					{id: A, __typename: "Entity"},
					{id: B, __typename: "Entity"},
				],
			},
		})
	})

	it("serves an empty page as an empty list and excludes entities without a value of the property", async () => {
		expect(await createEntitiesOrderedByPropertyResolver(make([]).deps)(context(Q))).toEqual({
			ok: true,
			data: {entitiesOrderedByProperty: []},
		})
		const {deps} = make([row(A, "10"), row(B, "ignored", OTHER_PROP)])
		expect(await createEntitiesOrderedByPropertyResolver(deps)(context(Q))).toEqual({
			ok: true,
			data: {entitiesOrderedByProperty: [{id: A, __typename: "Entity"}]},
		})
	})

	it("applies offset and first to the whole set", async () => {
		const q = `{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", sortDirection: DESC, first: 3, offset: 1) { id } }`
		const {deps, readEntitiesPage} = make([row(A, "10"), row(B, "30"), row(C, "20")])
		expect(await createEntitiesOrderedByPropertyResolver(deps)(context(q))).toEqual({
			ok: true,
			data: {entitiesOrderedByProperty: [{id: C}, {id: A}]},
		})
		expect(readEntitiesPage).toHaveBeenCalledWith(expect.objectContaining({first: 4}))
	})

	it("declines a page cut at its window (counting distinct entities, not rows)", async () => {
		const q = `{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", sortDirection: DESC, first: 2) { id } }`
		expect(
			await createEntitiesOrderedByPropertyResolver(make([row(A, "5"), row(B, "1")]).deps)(context(q)),
		).toEqual({
			ok: false,
			reason: "truncated",
		})
		// One entity with two rows is only 1 distinct entity (< 2), so it is not truncated.
		expect(
			await createEntitiesOrderedByPropertyResolver(make([row(A, "5"), row(A, "9")]).deps)(context(q)),
		).toEqual({
			ok: true,
			data: {entitiesOrderedByProperty: [{id: A}]},
		})
	})

	it("declines a window past geo-db's page cap without reading", async () => {
		const q = `{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", first: ${MAX_PAGE}, offset: 1) { id } }`
		const {deps, readEntitiesPage} = make([])
		expect(await createEntitiesOrderedByPropertyResolver(deps)(context(q))).toEqual({
			ok: false,
			reason: "truncated",
		})
		expect(readEntitiesPage).not.toHaveBeenCalled()
	})

	it("maps legacy ids back and breaks ties by gaia id ascending", async () => {
		const legacy = "0a0a4147a92855e88e630e542faf1c72"
		const geo = conformingId("entity", legacy)
		const back = new Map([
			[geo, legacy],
			[B, B],
		])
		const {deps} = make([row(B, "5"), row(geo, "5")], {
			gaiaIdsOf: async (ids) =>
				new Map(ids.flatMap((i) => (back.has(i) ? [[i, back.get(i) as string] as const] : []))),
		})
		expect(await createEntitiesOrderedByPropertyResolver(deps)(context(Q))).toEqual({
			ok: true,
			data: {
				entitiesOrderedByProperty: [
					{id: legacy, __typename: "Entity"},
					{id: B, __typename: "Entity"},
				],
			},
		})
	})

	it("orders decimal, boolean, date and datetime values accurately", async () => {
		const decQ = `{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "decimal", sortDirection: ASC, first: 10) { id } }`
		expect(
			await createEntitiesOrderedByPropertyResolver(make([row(A, "-1.25"), row(B, "-1.5"), row(C, "10")]).deps)(
				context(decQ),
			),
		).toEqual({ok: true, data: {entitiesOrderedByProperty: [{id: B}, {id: A}, {id: C}]}})

		const dtQ = `{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "datetime", sortDirection: DESC, first: 10) { id } }`
		expect(
			await createEntitiesOrderedByPropertyResolver(
				make([row(A, "instant:10.000000000"), row(B, "instant:10.000000001")]).deps,
			)(context(dtQ)),
		).toEqual({ok: true, data: {entitiesOrderedByProperty: [{id: B}, {id: A}]}})
	})

	it("declines an unparseable non-blank property value", async () => {
		expect(await createEntitiesOrderedByPropertyResolver(make([row(A, "not-a-number")]).deps)(context(Q))).toEqual({
			ok: false,
			reason: "args",
		})
	})

	it("declines an id Postgres cannot place, and a failed map read passes through", async () => {
		const unmapped = make([row(A, "5"), row(B, "1")], {
			gaiaIdsOf: async (ids) => new Map(ids.filter((i) => i !== B).map((i) => [i, i])),
		}).deps
		expect(await createEntitiesOrderedByPropertyResolver(unmapped)(context(Q))).toEqual({
			ok: false,
			reason: "legacy_id",
		})
		const none = make([row(A, "5")], {gaiaIdsOf: undefined}).deps
		expect(await createEntitiesOrderedByPropertyResolver(none)(context(Q))).toEqual({
			ok: false,
			reason: "legacy_id",
		})
		const failing = make([row(A, "5")], {
			gaiaIdsOf: async () => Promise.reject(Object.assign(new Error("x"), {reason: "id_map"})),
		}).deps
		await expect(createEntitiesOrderedByPropertyResolver(failing)(context(Q))).rejects.toMatchObject({
			reason: "id_map",
		})
	})

	it("declines an unmapped Space and a client without orderPick", async () => {
		const unmappedSpace = make([row(A, "5")], {geoSpaceOf: async () => null})
		expect(await createEntitiesOrderedByPropertyResolver(unmappedSpace.deps)(context(Q))).toEqual({
			ok: false,
			reason: "unmapped_space",
		})
		const off = make([row(A, "5")], {orderPick: false})
		expect(await createEntitiesOrderedByPropertyResolver(off.deps)(context(Q))).toEqual({
			ok: false,
			reason: "not_enabled",
		})
		expect(off.readEntitiesPage).not.toHaveBeenCalled()
	})

	it("declines an unsupported shape before reading geo-db", async () => {
		const {deps, readEntitiesPage} = make([row(A, "5")])
		const q = `{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", first: 10) { id name } }`
		expect(await createEntitiesOrderedByPropertyResolver(deps)(context(q))).toEqual({
			ok: false,
			reason: "selection",
		})
		expect(readEntitiesPage).not.toHaveBeenCalled()
	})

	it("lets a geo-db failure through for the gateway to count", async () => {
		const deps = make([row(A, "5")]).deps
		deps.geoDb = {
			readEntitiesPage: async () => Promise.reject(Object.assign(new Error("x"), {reason: "timeout"})),
		} as never
		await expect(createEntitiesOrderedByPropertyResolver(deps)(context(Q))).rejects.toMatchObject({
			reason: "timeout",
		})
	})
})

describe("entitiesOrderedByProperty=on through the gateway", () => {
	const post = (query: string) =>
		new Request("http://api/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query}),
		})
	const gateway = (rows: GeoPageRow[]) => {
		__resetServeMetricsForTests()
		const delegate = vi.fn(async () => new Response("pg"))
		const resolvers = new Map([
			["entitiesOrderedByProperty", createEntitiesOrderedByPropertyResolver(make(rows).deps)],
		])
		const config = readGatewayConfig({
			GRAPHQL_GATEWAY: "on",
			GRAPHQL_GATEWAY_FIELDS: "entitiesOrderedByProperty=on",
		})
		return {delegate, run: createGateway({config, delegate, resolvers})}
	}

	it("answers a supported shape from geo-db without asking PostGraphile", async () => {
		const {delegate, run} = gateway([row(A, "1"), row(B, "5")])
		const response = await run(post(Q), {})
		expect(response.headers.get("x-gaia-source")).toBe("geodb")
		expect(await response.json()).toEqual({
			data: {
				entitiesOrderedByProperty: [
					{id: B, __typename: "Entity"},
					{id: A, __typename: "Entity"},
				],
			},
		})
		expect(delegate).not.toHaveBeenCalled()
		expect(renderServeMetrics()).toContain(
			'gaia_api_gateway_served_total{field="entitiesOrderedByProperty",source="geodb"} 1',
		)
	})

	it("falls back to PostGraphile for any other shape and counts the reason", async () => {
		const {delegate, run} = gateway([row(A, "5")])
		const response = await run(
			post(
				`{ entitiesOrderedByProperty(propertyId: "${PROP}", spaceIds: ["${SPACE}"], typeIds: ["${TYPE}"], dataType: "integer", first: 10) { id name } }`,
			),
			{},
		)
		expect(await response.text()).toBe("pg")
		expect(delegate).toHaveBeenCalledTimes(1)
		expect(renderServeMetrics()).toContain(
			'gaia_api_gateway_serve_fallback_total{field="entitiesOrderedByProperty",reason="selection"} 1',
		)
	})
})
