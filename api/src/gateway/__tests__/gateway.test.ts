import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import {classifyDocument, classifyRequest} from "../classify"
import {parseFieldModes, parseGatewayMode, readGatewayConfig} from "../config"
import {createGateway, routeOperation} from "../gateway"

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
