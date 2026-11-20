import {beforeEach, describe, expect, it, vi} from "vitest"
import {readGatewayConfig} from "../config"
import {createGateway} from "../gateway"
import {__resetGatewayMetricsForTests, activateGatewayMetrics, renderGatewayMetrics} from "../metrics"
import {__resetServeMetricsForTests} from "../serveMetrics"

const NAME = "gaia_api_gateway_requests_total"
const series = () =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${NAME}{`))
			.map((l) => [l.slice(NAME.length + 1, l.lastIndexOf("}")), Number(l.split(" ").pop())]),
	)

const post = (query: string) =>
	new Request("http://api/graphql", {
		method: "POST",
		headers: {"content-type": "application/json"},
		body: JSON.stringify({query}),
	})
const delegate = async () => new Response(JSON.stringify({data: {}}), {headers: {"content-type": "application/json"}})

beforeEach(() => {
	__resetGatewayMetricsForTests()
	__resetServeMetricsForTests()
})

describe("gaia_api_gateway_requests_total", () => {
	it("is a series of its own, apart from the shadow-only counter", () => {
		activateGatewayMetrics("entity")
		const text = renderGatewayMetrics()
		expect(text).toContain(`# TYPE ${NAME} counter`)
		expect(text).toContain("gaia_api_gateway_shadow_requests_total")
		expect(series()).toEqual({'field="entity"': 0})
	})

	it("counts a shadowed field's operations before the sample rate drops them", async () => {
		activateGatewayMetrics("entity")
		const gateway = createGateway({
			config: readGatewayConfig({
				GRAPHQL_GATEWAY: "on",
				GRAPHQL_GATEWAY_FIELDS: "entity=shadow",
				GRAPHQL_GATEWAY_SHADOW_SAMPLE: "0.5",
			}),
			delegate,
			shadows: new Map([["entity", vi.fn(async () => {})]]),
			random: () => 0.9,
		})
		await gateway(post('{ entity(id: "x") { id } }'), {})
		await gateway(post('{ entity(id: "x") { id } }'), {})
		expect(series()).toEqual({'field="entity"': 2})
	})

	it("counts a served field's operations whether geo-db answers or PostgreSQL does", async () => {
		const declines = vi.fn(async () => ({ok: false as const, reason: "empty"}))
		const answers = vi.fn(async () => ({ok: true as const, data: {entity: null}}))
		const make = (resolver: typeof declines | typeof answers) =>
			createGateway({
				config: readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "entity=on"}),
				delegate,
				resolvers: new Map([["entity", resolver]]),
			})
		await make(answers)(post('{ entity(id: "x") { id } }'), {})
		await make(declines)(post('{ entity(id: "x") { id } }'), {})
		expect(series()).toEqual({'field="entity"': 2})
	})

	it("does not count an operation no shadow or resolver takes", async () => {
		activateGatewayMetrics("entity")
		const gateway = createGateway({
			config: readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "entity=shadow"}),
			delegate,
			shadows: new Map([["entity", vi.fn(async () => {})]]),
		})
		await gateway(post("{ spaces { id } }"), {})
		expect(series()).toEqual({'field="entity"': 0})
	})
})
