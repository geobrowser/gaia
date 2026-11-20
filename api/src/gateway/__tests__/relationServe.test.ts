import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow} from "../../geodb/card"
import {GeoDbError} from "../../geodb/client"
import {readGatewayConfig} from "../config"
import {createGateway, type ResolveContext} from "../gateway"
import {createRelationResolver, planRelationServe, type RelationServeDeps} from "../relationServe"
import {__resetServeMetricsForTests, renderServeMetrics} from "../serveMetrics"

const REL = "990a4147a92855e88e630e542faf1c72"
const FROM = "aa000000000040008000000000000001"
const LEGACY_FROM = "00000000000000000000000000000002"
const PROP = "44444444444444448444444444444444"

const context = (query: string, variables: Record<string, unknown> = {}): ResolveContext => ({
	classification: {operationName: null, operationType: "query", rootFields: ["relation"]},
	params: {query, operationName: null, variables},
	document: parse(query),
	signal: new AbortController().signal,
})

const identity = async (ids: readonly string[]) => new Map(ids.map((id) => [id, id]))

const make = (rows: GeoCardRow[], overrides: Partial<RelationServeDeps> = {}) => {
	const readRelationEntity = vi.fn(async (_args: {relationId: string}) => rows)
	const deps: RelationServeDeps = {
		geoDb: {readRelationEntity} as unknown as RelationServeDeps["geoDb"],
		gaiaIdsOf: identity,
		...overrides,
	}
	return {deps, readRelationEntity}
}

const Q = `{ relation(id: "${REL}") { id fromEntityId __typename } }`

describe("planRelationServe", () => {
	const reason = (query: string, variables: Record<string, unknown> = {}) => {
		try {
			planRelationServe(parse(query), null, variables)
			return "ok"
		} catch (error) {
			return (error as {reason?: string}).reason
		}
	}

	it("accepts relation(id) selecting any subset of id, fromEntityId, __typename with aliases, fragments and variables", () => {
		expect(reason(Q)).toBe("ok")
		expect(reason(`{ relation(id: "${REL}") { fromEntityId } }`)).toBe("ok")
		expect(
			reason(
				`query Q($id: UUID!) { r: relation(id: $id) { ...F } } fragment F on Relation { rid: id from: fromEntityId t: __typename }`,
				{id: REL},
			),
		).toBe("ok")
	})

	it("declines unsupported columns, nested objects, extra/bad args, directives, and non-single roots", () => {
		for (const field of ["spaceId", "typeId", "toEntityId", "entityId", "toSpaceId", "verified", "position"]) {
			expect(reason(`{ relation(id: "${REL}") { id ${field} } }`)).toBe("selection")
		}
		expect(reason(`{ relation(id: "${REL}") { id fromEntity { id } } }`)).toBe("selection")
		expect(reason(`{ relation(id: "${REL}") { id entity { id } } }`)).toBe("selection")
		expect(reason(`{ relation { id } }`)).toBe("args")
		expect(reason(`{ relation(id: "not-a-uuid") { id } }`)).toBe("args")
		expect(reason(`{ relation(id: "${REL}", extra: 1) { id } }`)).toBe("args")
		expect(reason(`{ relation(id: "${REL}") { id @include(if: true) } }`)).toBe("directives")
		expect(reason(`{ relation(id: "${REL}") { id } spaces { id } }`)).toBe("not_single_root")
		expect(reason(`{ relations(first: 1) { id } }`)).toBe("not_single_root")
	})
})

describe("createRelationResolver", () => {
	it("serves relation(id) from geo-db's readRelationEntity in PostGraphile's shape", async () => {
		const {deps, readRelationEntity} = make([{prop: PROP, raw: "", entity: FROM}])
		expect(await createRelationResolver(deps)(context(Q))).toEqual({
			ok: true,
			data: {
				relation: {
					id: REL,
					fromEntityId: FROM,
					__typename: "Relation",
				},
			},
		})
		expect(readRelationEntity).toHaveBeenCalledWith({relationId: conformingId("entity", REL)})
	})

	it("maps a legacy fromEntityId back through the reverse id map and preserves aliases", async () => {
		const geoFrom = conformingId("entity", LEGACY_FROM)
		const q = `query Q($id: UUID!) { r: relation(id: $id) { rid: id from: fromEntityId t: __typename } }`
		const {deps} = make([{prop: PROP, raw: "", entity: geoFrom}], {
			gaiaIdsOf: async () => new Map([[geoFrom, LEGACY_FROM]]),
		})
		expect(await createRelationResolver(deps)(context(q, {id: REL}))).toEqual({
			ok: true,
			data: {
				r: {
					rid: REL,
					from: LEGACY_FROM,
					t: "Relation",
				},
			},
		})
	})

	it("declines empty, missing readRelationEntity, missing/unmapped legacy id, aborted signal, and propagates GeoDbError", async () => {
		expect(await createRelationResolver(make([]).deps)(context(Q))).toEqual({ok: false, reason: "empty"})
		expect(
			await createRelationResolver(
				make([{prop: PROP, raw: "", entity: FROM}], {
					geoDb: {} as unknown as RelationServeDeps["geoDb"],
				}).deps,
			)(context(Q)),
		).toEqual({ok: false, reason: "not_enabled"})
		expect(
			await createRelationResolver(make([{prop: PROP, raw: "", entity: FROM}], {gaiaIdsOf: undefined}).deps)(
				context(Q),
			),
		).toEqual({ok: false, reason: "legacy_id"})
		expect(
			await createRelationResolver(
				make([{prop: PROP, raw: "", entity: FROM}], {gaiaIdsOf: async () => new Map()}).deps,
			)(context(Q)),
		).toEqual({ok: false, reason: "legacy_id"})

		const controller = new AbortController()
		const {deps: abortDeps} = make([], {
			geoDb: {
				readRelationEntity: async () => {
					controller.abort()
					return [{prop: PROP, raw: "", entity: FROM}]
				},
			} as unknown as RelationServeDeps["geoDb"],
		})
		expect(await createRelationResolver(abortDeps)({...context(Q), signal: controller.signal})).toEqual({
			ok: false,
			reason: "timeout",
		})

		const {deps: failDeps} = make([], {
			geoDb: {
				readRelationEntity: async () => {
					throw new GeoDbError("timeout", "deadline exceeded")
				},
			} as unknown as RelationServeDeps["geoDb"],
		})
		await expect(createRelationResolver(failDeps)(context(Q))).rejects.toThrow(GeoDbError)
	})

	it("wires through createGateway: serves supported shapes and falls back to PostGraphile on declines", async () => {
		__resetServeMetricsForTests()
		const post = (query: string) =>
			new Request("http://api/graphql", {
				method: "POST",
				headers: {"content-type": "application/json"},
				body: JSON.stringify({query}),
			})
		const config = readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "relation=on"})
		const {deps} = make([{prop: PROP, raw: "", entity: FROM}])
		const delegate = vi.fn(async () => new Response("pg"))
		const run = createGateway({
			config,
			delegate,
			resolvers: new Map([["relation", createRelationResolver(deps)]]),
		})

		const served = await run(post(Q), {})
		expect(served.headers.get("x-gaia-source")).toBe("geodb")
		expect(delegate).not.toHaveBeenCalled()
		expect(await served.json()).toEqual({
			data: {relation: {id: REL, fromEntityId: FROM, __typename: "Relation"}},
		})

		const declined = await run(post(`{ relation(id: "${REL}") { id spaceId } }`), {})
		expect(delegate).toHaveBeenCalledTimes(1)
		expect(await declined.text()).toBe("pg")

		const metrics = renderServeMetrics()
		expect(metrics).toContain('gaia_api_gateway_served_total{field="relation",source="geodb"} 1')
		expect(metrics).toContain('gaia_api_gateway_served_total{field="relation",source="postgres"} 1')
		expect(metrics).toContain('gaia_api_gateway_serve_fallback_total{field="relation",reason="selection"} 1')
	})
})
