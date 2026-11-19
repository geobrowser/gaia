import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {rowsFromGeoDbOutput} from "../../geodb/card"
import {readGatewayConfig} from "../config"
import {createEntityShadow} from "../entityShadow"
import {createGateway} from "../gateway"
import {
	__resetGatewayMetricsForTests,
	activateGatewayMetrics,
	MISMATCH_PARTS,
	recordGatewayMismatchParts,
	renderGatewayMetrics,
} from "../metrics"
import {mismatchPartsOf} from "../mismatchParts"

const series = (name: string) =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${name}{`) && !l.endsWith(" 0"))
			.map((l) => [l.slice(name.length + 1, l.lastIndexOf("}")), Number(l.split(" ").pop())]),
	)

beforeEach(() => __resetGatewayMetricsForTests())

describe("mismatchPartsOf", () => {
	it("names statements only one side holds", () => {
		expect(mismatchPartsOf({onlyPostgres: 1, onlyGeoDb: 0, derivedMismatches: []})).toEqual(["statements_missing"])
		expect(mismatchPartsOf({onlyPostgres: 0, onlyGeoDb: 2})).toEqual(["statements_extra"])
		expect(mismatchPartsOf({missingInGeoDb: 1, geoOnlyRows: 1})).toEqual(["statements_missing", "statements_extra"])
	})

	it("maps derived field checks to fixed parts", () => {
		expect(mismatchPartsOf({derivedMismatches: ["name", "spaceIds", "updatedAt", "typesNames"]})).toEqual([
			"name",
			"space_ids",
			"updated_at",
			"types_names",
		])
	})

	it("names the extra checks, an absent entity and slice reads", () => {
		expect(
			mismatchPartsOf({
				nestedMismatches: {a: 1},
				propertyInfoMismatches: {b: 1},
				relationsMembership: {c: 1},
				relationFacets: [{x: 1}],
			}),
		).toEqual(["nested_entity", "property_info", "relations_membership", "relation_facets"])
		expect(mismatchPartsOf({absentInPostgres: true, holders: 2})).toEqual(["absent_in_postgres"])
		expect(mismatchPartsOf({edge: {typeId: "t"}})).toEqual(["relation_edge"])
		expect(mismatchPartsOf({facets: {diff: {}}})).toEqual(["relation_facets"])
		expect(mismatchPartsOf({slice: {}, onlyPostgres: [{a: 1}], onlyGeoDb: []})).toEqual(["statements_missing"])
	})

	it("reads an operation's own parts and its first entity's detail", () => {
		expect(mismatchPartsOf({parts: ["ordered_page", "type_scan", "bogus"]})).toEqual(["ordered_page", "type_scan"])
		expect(mismatchPartsOf({filterBreach: {strayIds: ["x"]}, first: {derivedMismatches: ["createdAt"]}})).toEqual([
			"filter_breach",
			"created_at",
		])
	})

	it("falls back to other, never to a free-text label", () => {
		expect(mismatchPartsOf({})).toEqual(["other"])
		expect(mismatchPartsOf({derivedMismatches: ["somethingNew"]})).toEqual(["other"])
	})
})

describe("gaia_api_gateway_shadow_mismatch_total", () => {
	it("counts each part once per operation, every series at zero, parts from the fixed set", () => {
		activateGatewayMetrics("entities")
		const lines = renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith("gaia_api_gateway_shadow_mismatch_total{"))
		expect(lines).toHaveLength(MISMATCH_PARTS.length)
		recordGatewayMismatchParts("entities", ["name", "name", "types"])
		recordGatewayMismatchParts("entities", [])
		expect(series("gaia_api_gateway_shadow_mismatch_total")).toEqual({
			'field="entities",part="name"': 1,
			'field="entities",part="types"': 1,
			'field="entities",part="other"': 1,
		})
	})
})

const ENTITY = "073ff16b-e7fe-4591-a984-c325d629d92d"
const ENTITY_HEX = "073ff16be7fe4591a984c325d629d92d"
const SPACE = "11111111-1111-4111-8111-111111111111"
const GEO_SPACE = "22222222222242228222222222222222"
const POINTS = "5ab2b8a5a1a14e5d9a6e8fb2d4f0c9a1"
const nullValues = {integer: null, float: null, boolean: null, datetime: null, date: null, decimal: null}
const QUERY = `query Card($id: UUID!, $spaceId: UUID) {
  entity(id: $id) {
    id
    valuesList(first: 1000, filter: {spaceId: {is: $spaceId}}) { spaceId propertyId text integer float boolean datetime date decimal }
    relationsList(first: 1000, filter: {spaceId: {is: $spaceId}}) { typeId toEntityId toSpaceId position }
  }
}`
const BODY = {
	data: {
		entity: {
			id: ENTITY,
			valuesList: [{spaceId: SPACE, propertyId: POINTS, text: null, ...nullValues, integer: "42"}],
			relationsList: [],
		},
	},
}

describe("entity shadow names the differing part", () => {
	it("counts statements_missing and statements_extra for a changed value", async () => {
		const log = {info: vi.fn(), warn: vi.fn()}
		const shadow = createEntityShadow({
			geoDb: {
				readCard: async ({entityId}) => rowsFromGeoDbOutput(`${POINTS}\t43\ts2\t${ENTITY_HEX}\t1\n`, entityId),
			},
			geoSpaceOf: async (space) => (space === SPACE.replaceAll("-", "") ? GEO_SPACE : null),
			log,
		})
		await shadow({
			classification: {operationName: "Card", operationType: "query", rootFields: ["entity"]},
			params: {query: QUERY, operationName: "Card", variables: {id: ENTITY, spaceId: SPACE}},
			document: parse(QUERY),
			response: Response.json(BODY),
		})
		expect(series("gaia_api_gateway_shadow_mismatch_total")).toEqual({
			'field="entity",part="statements_missing"': 1,
			'field="entity",part="statements_extra"': 1,
		})
		expect(log.warn).toHaveBeenCalledWith("gateway entity shadow mismatch", expect.anything())
	})
})

describe("gaia_api_gateway_shadow_requests_total", () => {
	const post = (query: string) =>
		new Request("http://api/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query}),
		})
	const delegate = async () =>
		new Response(JSON.stringify({data: {}}), {headers: {"content-type": "application/json"}})

	it("counts operations before the sample rate drops them", async () => {
		activateGatewayMetrics("entitiesConnection")
		const shadow = vi.fn(async () => {})
		const gateway = createGateway({
			config: readGatewayConfig({
				GRAPHQL_GATEWAY: "on",
				GRAPHQL_GATEWAY_FIELDS: "entitiesConnection=shadow",
				GRAPHQL_GATEWAY_SHADOW_SAMPLE: "0.5",
			}),
			delegate,
			shadows: new Map([["entitiesConnection", shadow]]),
			random: () => 0.9,
		})
		await gateway(post("{ entitiesConnection { totalCount } }"), {})
		await gateway(post("{ entitiesConnection { totalCount } }"), {})
		await new Promise((resolve) => setTimeout(resolve, 5))
		expect(shadow).not.toHaveBeenCalled()
		expect(series("gaia_api_gateway_shadow_requests_total")).toEqual({'field="entitiesConnection"': 2})
	})

	it("does not count an operation no shadow takes", async () => {
		activateGatewayMetrics("entitiesConnection")
		const gateway = createGateway({
			config: readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "entitiesConnection=shadow"}),
			delegate,
			shadows: new Map([["entitiesConnection", vi.fn(async () => {})]]),
			random: () => 0,
		})
		await gateway(post("{ spaces { id } }"), {})
		expect(series("gaia_api_gateway_shadow_requests_total")).toEqual({})
	})
})

describe("gaia_api_gateway_shadow_postgres_seconds and gaia_api_gateway_shadow_geodb_seconds", () => {
	const post = (query: string) =>
		new Request("http://api/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query}),
		})
	const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))
	const count = (name: string, field: string) => {
		const line = renderGatewayMetrics()
			.split("\n")
			.find((l) => l.startsWith(`${name}_count{field="${field}"}`))
		return line === undefined ? undefined : Number(line.split(" ").pop())
	}
	const bucket = (name: string, field: string, le: string) => {
		const line = renderGatewayMetrics()
			.split("\n")
			.find((l) => l.startsWith(`${name}_bucket{field="${field}",le="${le}"}`))
		return line === undefined ? undefined : Number(line.split(" ").pop())
	}

	it("registers both families at zero for an active field", () => {
		activateGatewayMetrics("entity")
		expect(count("gaia_api_gateway_shadow_postgres_seconds", "entity")).toBe(0)
		expect(count("gaia_api_gateway_shadow_geodb_seconds", "entity")).toBe(0)
		expect(bucket("gaia_api_gateway_shadow_geodb_seconds", "entity", "+Inf")).toBe(0)
	})

	it("times PostgreSQL for every shadowable operation and geo-db for each compared one", async () => {
		activateGatewayMetrics("entity")
		const gateway = createGateway({
			config: readGatewayConfig({
				GRAPHQL_GATEWAY: "on",
				GRAPHQL_GATEWAY_FIELDS: "entity=shadow",
				GRAPHQL_GATEWAY_SHADOW_SAMPLE: "0.5",
			}),
			delegate: async () => {
				await sleep(30)
				return new Response(JSON.stringify({data: {}}), {headers: {"content-type": "application/json"}})
			},
			shadows: new Map([
				[
					"entity",
					async () => {
						await sleep(80)
					},
				],
			]),
			random: vi.fn().mockReturnValueOnce(0).mockReturnValueOnce(0.9),
		})
		await gateway(post('{ entity(id: "a") { id } }'), {})
		await gateway(post('{ entity(id: "a") { id } }'), {})
		await sleep(200)
		// Both operations were shadowable, only the first was sampled.
		expect(count("gaia_api_gateway_shadow_postgres_seconds", "entity")).toBe(2)
		expect(count("gaia_api_gateway_shadow_geodb_seconds", "entity")).toBe(1)
		// A 30 ms delegate is over the 25 ms edge and under 250 ms; an 80 ms shadow is over 50 ms and under 250 ms.
		expect(bucket("gaia_api_gateway_shadow_postgres_seconds", "entity", "0.025")).toBe(0)
		expect(bucket("gaia_api_gateway_shadow_postgres_seconds", "entity", "0.25")).toBe(2)
		expect(bucket("gaia_api_gateway_shadow_geodb_seconds", "entity", "0.05")).toBe(0)
		expect(bucket("gaia_api_gateway_shadow_geodb_seconds", "entity", "0.25")).toBe(1)
	})

	it("still times a shadow that throws, and does not time an unshadowed field", async () => {
		activateGatewayMetrics("entity")
		const gateway = createGateway({
			config: readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "entity=shadow"}),
			delegate: async () =>
				new Response(JSON.stringify({data: {}}), {headers: {"content-type": "application/json"}}),
			shadows: new Map([
				[
					"entity",
					async () => {
						throw new Error("boom")
					},
				],
			]),
			random: () => 0,
		})
		await gateway(post("{ spaces { id } }"), {})
		await gateway(post('{ entity(id: "a") { id } }'), {})
		await sleep(20)
		expect(count("gaia_api_gateway_shadow_postgres_seconds", "entity")).toBe(1)
		expect(count("gaia_api_gateway_shadow_geodb_seconds", "entity")).toBe(1)
	})
})
