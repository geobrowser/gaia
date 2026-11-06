import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {classifyDocument, classifyRequest} from "../classify"
import {parseFieldModes, parseGatewayMode, parseServeTimeoutMs, readGatewayConfig} from "../config"
import {createGateway, type FieldResolver, routeOperation} from "../gateway"
import {__resetServeMetricsForTests, renderServeMetrics} from "../serveMetrics"

const json = (body: unknown) =>
	new Request("http://api/graphql", {
		method: "POST",
		headers: {"content-type": "application/json"},
		body: JSON.stringify(body),
	})

describe("config", () => {
	it("is off unless GRAPHQL_GATEWAY is exactly on", () => {
		expect(parseGatewayMode(undefined)).toBe("off")
		expect(parseGatewayMode("")).toBe("off")
		expect(parseGatewayMode("shadow")).toBe("off")
		expect(parseGatewayMode("true")).toBe("off")
		expect(parseGatewayMode(" ON ")).toBe("on")
	})

	it("parses per-field modes and treats anything unknown as off", () => {
		const modes = parseFieldModes("entity=shadow, entities=on,relation=bogus,=on,bad name=on,noequals")
		expect(Object.fromEntries(modes)).toEqual({entity: "shadow", entities: "on", relation: "off", noequals: "off"})
		expect(parseFieldModes(undefined).size).toBe(0)
	})

	it("reads both variables from the environment", () => {
		const config = readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "entity=on"})
		expect(config.mode).toBe("on")
		expect(config.fieldModes.get("entity")).toBe("on")
		expect(readGatewayConfig({}).mode).toBe("off")
	})
})

describe("classifyDocument", () => {
	it("lists distinct root fields by name, not alias, and skips __typename", () => {
		const doc = parse(
			"query Q { a: entity(id: 1) { id } b: entity(id: 2) { id } entitiesConnection { totalCount } __typename }",
		)
		expect(classifyDocument(doc, null)).toEqual({
			operationName: "Q",
			operationType: "query",
			rootFields: ["entity", "entitiesConnection"],
		})
	})

	it("follows root fragments and inline fragments", () => {
		const doc = parse(
			"query Q { ...F ... on Query { spaces { id } } } fragment F on Query { entity(id: 1) { id } ...F }",
		)
		expect(classifyDocument(doc, "Q").rootFields).toEqual(["entity", "spaces"])
	})

	it("picks the named operation, and refuses to guess among several", () => {
		const doc = parse("query A { entity(id: 1) { id } } query B { spaces { id } }")
		expect(classifyDocument(doc, "B").rootFields).toEqual(["spaces"])
		expect(classifyDocument(doc, null).rootFields).toEqual([])
		expect(classifyDocument(doc, "C").rootFields).toEqual([])
	})

	it("reports the operation type", () => {
		expect(classifyDocument(parse("mutation M { x }"), null).operationType).toBe("mutation")
		expect(classifyDocument(parse("{ entity(id: 1) { id } }"), null).operationName).toBeNull()
	})
})

describe("classifyRequest", () => {
	it("reads a POST body and leaves the request body unconsumed", async () => {
		const request = json({query: "query Q { entity(id: 1) { id } }", operationName: "Q"})
		expect((await classifyRequest(request)).rootFields).toEqual(["entity"])
		expect(await request.json()).toMatchObject({operationName: "Q"})
	})

	it("reads a GET query string", async () => {
		const url = `http://api/graphql?query=${encodeURIComponent("{ spaces { id } }")}`
		expect((await classifyRequest(new Request(url))).rootFields).toEqual(["spaces"])
	})

	it("returns no fields for garbage, batches, and non-GraphQL bodies", async () => {
		const bad = [
			new Request("http://api/graphql", {method: "POST", body: "not json"}),
			json([{query: "{ entity(id: 1) { id } }"}]),
			json({query: "{ entity("}),
			json({nothing: true}),
			new Request("http://api/graphql", {method: "PUT", body: "{}"}),
		]
		for (const request of bad) expect((await classifyRequest(request)).rootFields).toEqual([])
	})
})

describe("routeOperation", () => {
	const on = readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "entity=on"})
	const query = {operationName: "Q", operationType: "query" as const, rootFields: ["entity"]}

	it("delegates for every reason while no resolver exists", () => {
		const none = new Set<string>()
		expect(routeOperation(readGatewayConfig({}), none, query)).toEqual({kind: "delegate", reason: "gateway_off"})
		expect(routeOperation(on, none, {...query, rootFields: []})).toEqual({kind: "delegate", reason: "unclassified"})
		expect(routeOperation(on, none, {...query, operationType: "mutation"}).reason).toBe("unclassified")
		expect(routeOperation(on, none, query)).toEqual({kind: "delegate", reason: "no_resolver"})
	})

	it("still delegates a flagged field once it has a resolver but its mode is off", () => {
		const off = readGatewayConfig({GRAPHQL_GATEWAY: "on"})
		expect(routeOperation(off, new Set(["entity"]), query).reason).toBe("field_off")
	})
})

describe("createGateway", () => {
	const query = {query: "query Q { entity(id: 1) { id } }", operationName: "Q"}

	it("passes the original request and context to the delegate and returns its response", async () => {
		const response = new Response("pg")
		const delegate = vi.fn(async () => response)
		const ctx = {requestId: "r1"}
		const gateway = createGateway({config: readGatewayConfig({GRAPHQL_GATEWAY: "on"}), delegate})
		const request = json(query)
		expect(await gateway(request, ctx)).toBe(response)
		expect(delegate).toHaveBeenCalledWith(request, ctx)
		expect(await request.json()).toEqual(query)
	})

	it("is a plain pass-through when off, and does not read the body", async () => {
		const delegate = vi.fn(async () => new Response("pg"))
		const onClassified = vi.fn()
		const gateway = createGateway({config: readGatewayConfig({}), delegate, onClassified})
		await gateway(json(query), {})
		expect(delegate).toHaveBeenCalledTimes(1)
		expect(onClassified).not.toHaveBeenCalled()
	})

	it("reports the classification and survives a throwing observer", async () => {
		const delegate = vi.fn(async () => new Response("pg"))
		const seen: unknown[] = []
		const config = readGatewayConfig({GRAPHQL_GATEWAY: "on"})
		await createGateway({config, delegate, onClassified: (c, r) => seen.push([c.rootFields, r.reason])})(
			json(query),
			{},
		)
		expect(seen).toEqual([[["entity"], "no_resolver"]])
		const throwing = createGateway({
			config,
			delegate,
			onClassified: () => {
				throw new Error("boom")
			},
		})
		expect((await throwing(json(query), {})).status).toBe(200)
	})

	it("delegates a request that does not parse, leaving the error to PostGraphile", async () => {
		const delegate = vi.fn(async () => new Response("pg", {status: 400}))
		const gateway = createGateway({config: readGatewayConfig({GRAPHQL_GATEWAY: "on"}), delegate})
		expect((await gateway(json({query: "{ nope ("}), {})).status).toBe(400)
	})
})

describe("serving a field from geo-db", () => {
	const query = {query: "query Q { entity(id: 1) { id } }", operationName: "Q"}
	const served = {entity: {id: "e1"}}
	const config = (fields: string, timeout?: string) =>
		readGatewayConfig({
			GRAPHQL_GATEWAY: "on",
			GRAPHQL_GATEWAY_FIELDS: fields,
			GRAPHQL_GATEWAY_SERVE_TIMEOUT_MS: timeout,
		})
	const metric = (name: string, labels: string) =>
		Number(
			renderServeMetrics()
				.split("\n")
				.find((line) => line.startsWith(`${name}{${labels}}`))
				?.split(" ")[1],
		)

	beforeEach(() => __resetServeMetricsForTests())

	it("reads the serve timeout, defaulting out-of-range values", () => {
		expect(parseServeTimeoutMs(undefined)).toBe(250)
		expect(parseServeTimeoutMs("80")).toBe(80)
		for (const bad of ["0", "-5", "1.5", "nope", "60000"]) expect(parseServeTimeoutMs(bad)).toBe(250)
	})

	it("routes a lone root field in mode on with a resolver, and nothing else", () => {
		const resolvers = new Set(["entity", "relations"])
		const q = {operationName: "Q", operationType: "query" as const, rootFields: ["entity"]}
		expect(routeOperation(config("entity=on"), resolvers, q)).toEqual({
			kind: "serve",
			field: "entity",
			reason: "field_on",
		})
		expect(routeOperation(config("entity=shadow"), resolvers, q).kind).toBe("delegate")
		expect(
			routeOperation(config("entity=on,relations=on"), resolvers, {...q, rootFields: ["entity", "relations"]}),
		).toEqual({kind: "delegate", reason: "not_served"})
	})

	it("answers from the resolver without calling PostGraphile", async () => {
		const delegate = vi.fn(async () => new Response("pg"))
		const resolver = vi.fn(async () => ({ok: true as const, data: served}))
		const gateway = createGateway({
			config: config("entity=on"),
			delegate,
			resolvers: new Map([["entity", resolver]]),
		})
		const response = await gateway(json(query), {})
		expect(delegate).not.toHaveBeenCalled()
		expect(response.headers.get("x-gaia-source")).toBe("geodb")
		expect(response.headers.get("content-type")).toContain("application/json")
		expect(await response.json()).toEqual({data: served})
		expect(metric("gaia_api_gateway_served_total", 'field="entity",source="geodb"')).toBe(1)
		expect(metric("gaia_api_gateway_serve_seconds_count", 'field="entity",outcome="served"')).toBe(1)
	})

	it("falls back to PostGraphile on a decline, a throw and a timeout, counting each reason", async () => {
		const delegate = vi.fn(async () => new Response("pg"))
		const run = async (resolver: FieldResolver, timeout?: string) =>
			createGateway({config: config("entity=on", timeout), delegate, resolvers: new Map([["entity", resolver]])})(
				json(query),
				{},
			)
		expect(await (await run(async () => ({ok: false, reason: "selection"}))).text()).toBe("pg")
		expect(
			await (
				await run(async () => {
					throw Object.assign(new Error("x"), {reason: "overloaded"})
				})
			).text(),
		).toBe("pg")
		expect(
			await (
				await run(async () => {
					throw new Error("boom")
				})
			).text(),
		).toBe("pg")
		let aborted = false
		const slow: FieldResolver = ({signal}) =>
			new Promise((resolve) => {
				signal.addEventListener("abort", () => {
					aborted = true
				})
				setTimeout(() => resolve({ok: true, data: served}), 200)
			})
		expect(await (await run(slow, "5")).text()).toBe("pg")
		expect(aborted).toBe(true)
		expect(await (await run(async () => ({ok: false, reason: "not_a_reason"}))).text()).toBe("pg")
		expect(delegate).toHaveBeenCalledTimes(5)
		expect(metric("gaia_api_gateway_serve_fallback_total", 'field="entity",reason="selection"')).toBe(1)
		expect(metric("gaia_api_gateway_serve_fallback_total", 'field="entity",reason="overloaded"')).toBe(1)
		expect(metric("gaia_api_gateway_serve_fallback_total", 'field="entity",reason="timeout"')).toBe(1)
		expect(metric("gaia_api_gateway_serve_fallback_total", 'field="entity",reason="error"')).toBe(2)
		expect(metric("gaia_api_gateway_served_total", 'field="entity",source="postgres"')).toBe(5)
	})

	it("exports nothing and never calls a resolver while no field is on", async () => {
		const delegate = vi.fn(async () => new Response("pg"))
		const resolver = vi.fn(async () => ({ok: true as const, data: served}))
		const gateway = createGateway({
			config: config("entity=shadow"),
			delegate,
			resolvers: new Map([["entity", resolver]]),
		})
		expect(await (await gateway(json(query), {})).text()).toBe("pg")
		expect(resolver).not.toHaveBeenCalled()
		expect(renderServeMetrics()).toBe("")
	})
})

describe("shadow choice among several shadowed roots", () => {
	const post = (query: string) =>
		new Request("http://x/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query}),
		})
	const delegate = async () =>
		new Response(JSON.stringify({data: {}}), {headers: {"content-type": "application/json"}})
	const make = (tolerant: boolean) => {
		const metas = vi.fn(async () => {})
		const connection = Object.assign(
			vi.fn(async () => {}),
			tolerant ? {tolerates: ["metas"] as const} : {},
		)
		const gateway = createGateway({
			config: readGatewayConfig({
				GRAPHQL_GATEWAY: "on",
				GRAPHQL_GATEWAY_FIELDS: "metas=shadow,entitiesConnection=shadow",
				GRAPHQL_GATEWAY_SHADOW_SAMPLE: "1",
			}),
			delegate,
			shadows: new Map([
				["metas", metas],
				["entitiesConnection", connection],
			]),
			random: () => 0,
		})
		return {gateway, metas, connection}
	}
	const wait = () => new Promise((resolve) => setTimeout(resolve, 5))

	it("gives `metas` beside `entitiesConnection` to the shadow that tolerates it", async () => {
		const {gateway, metas, connection} = make(true)
		await gateway(post("{ metas { nodes { id } } entitiesConnection { totalCount } }"), {})
		await wait()
		expect(connection).toHaveBeenCalledTimes(1)
		expect(metas).not.toHaveBeenCalled()
	})

	it("runs `metas` alone, and an operation no shadow tolerates runs none", async () => {
		const {gateway, metas, connection} = make(true)
		await gateway(post("{ metas { nodes { id } } }"), {})
		await wait()
		expect(metas).toHaveBeenCalledTimes(1)
		const intolerant = make(false)
		await intolerant.gateway(post("{ metas { nodes { id } } entitiesConnection { totalCount } }"), {})
		await wait()
		expect(intolerant.metas).not.toHaveBeenCalled()
		expect(intolerant.connection).not.toHaveBeenCalled()
		expect(connection).toHaveBeenCalledTimes(0)
	})
})
