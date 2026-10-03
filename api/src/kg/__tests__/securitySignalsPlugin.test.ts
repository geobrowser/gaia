import {createYoga} from "graphql-yoga"
import {afterEach, beforeEach, describe, expect, it, vi} from "vitest"
import {log} from "../../services/telemetry"
import {graphqlServer, postgraphileSchema} from "../postgraphile"
import {renderSecuritySignalsPrometheus, useSecuritySignals} from "../securitySignalsPlugin"

/**
 * End to end through the production server: the real schema, the real plugin order, and the real
 * parse/validation cache in front of it.
 *
 * Counters are process-wide and accumulate across tests, so every assertion on a metric reads the
 * value before and after. Each test uses its own caller address so per-source log budgets from one
 * test never leak into another.
 */

async function post(body: string, ip: string, extraHeaders: Record<string, string> = {}) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json", "X-Forwarded-For": ip, ...extraHeaders},
			body,
		}),
		{},
	)
	return {status: response.status, body: await response.text()}
}

const query = (q: string, ip: string, headers?: Record<string, string>) => post(JSON.stringify({query: q}), ip, headers)

function metric(name: string, labels = ""): number {
	const line = renderSecuritySignalsPrometheus()
		.split("\n")
		.find((l) => l.startsWith(`${name}${labels} `))
	return line ? Number(line.split(" ")[1]) : 0
}

type Logged = [string, Record<string, unknown>]

describe("security signals (e2e)", () => {
	let warn: ReturnType<typeof vi.spyOn>
	let error: ReturnType<typeof vi.spyOn>

	beforeEach(() => {
		warn = vi.spyOn(log, "warn").mockImplementation(() => {})
		error = vi.spyOn(log, "error").mockImplementation(() => {})
	})
	afterEach(() => vi.restoreAllMocks())

	const signals = (): Record<string, unknown>[] =>
		(warn.mock.calls as Logged[]).filter(([m]) => m === "GraphQL security signal").map(([, d]) => d)
	const probesRaised = (): Record<string, unknown>[] =>
		(error.mock.calls as Logged[]).filter(([m]) => m === "Hidden GraphQL surface probed").map(([, d]) => d)

	it("trips on a hidden table, counts it, logs who asked, and raises one Sentry issue", async () => {
		const before = metric("gaia_api_graphql_hidden_surface_probe_total", '{target="Query.appWebhook"}')
		const res = await query(`{ appWebhooksConnection { nodes { secret url } } }`, "203.0.113.10", {
			"User-Agent": "scanner/1.0",
		})
		expect(res.status).toBe(200) // GraphQL-over-HTTP returns validation errors with 200 here
		expect(res.body).toContain("Cannot query field")

		expect(metric("gaia_api_graphql_hidden_surface_probe_total", '{target="Query.appWebhook"}')).toBe(before + 1)
		const [s] = signals()
		expect(s).toMatchObject({
			signal: "hidden_surface_probe",
			clientIp: "203.0.113.10",
			userAgent: "scanner/1.0",
			hiddenSurface: ["Query.appWebhook"],
			unknownFields: ["Query.appWebhooksConnection"],
			newEpisode: true,
		})
		expect(probesRaised()).toHaveLength(1)
		expect(probesRaised()[0]).toMatchObject({clientIp: "203.0.113.10"})
	})

	it("trips on a hidden mutation now that the schema has none", async () => {
		await query(`mutation { sampleFeedComposition(input: {}) { clientMutationId } }`, "203.0.113.11")
		expect(signals()[0]).toMatchObject({
			signal: "hidden_surface_probe",
			hiddenSurface: ["Mutation.sampleFeedComposition"],
		})
	})

	it("trips on a node(nodeId) lookup cast to a hidden type", async () => {
		await query(`{ node(nodeId: "x") { ... on AppWebhook { secret } } }`, "203.0.113.12")
		expect(signals()[0]).toMatchObject({signal: "hidden_surface_probe", hiddenSurface: ["type:AppWebhook"]})
	})

	it("counts every repeat even when the parse/validation cache serves it", async () => {
		const before = metric("gaia_api_graphql_rejected_total", '{reason="unknown_field"}')
		for (let i = 0; i < 3; i++) await query(`{ notificationOutboxes { payload } }`, "203.0.113.13")
		expect(metric("gaia_api_graphql_rejected_total", '{reason="unknown_field"}')).toBe(before + 3)
		expect(signals()).toHaveLength(3)
	})

	it("raises the Sentry issue once per source, not once per request", async () => {
		for (let i = 0; i < 5; i++) await query(`{ appWebhooks { secret } }`, "203.0.113.14")
		expect(probesRaised()).toHaveLength(1)
	})

	it("rations log lines per source and reports what it withheld", async () => {
		const suppressedBefore = metric("gaia_api_security_signal_log_suppressed_total")
		for (let i = 0; i < 25; i++) await query(`{ doesNotExist${i} }`, "203.0.113.15")
		// 20 logged in full, 5 withheld but still counted.
		expect(signals()).toHaveLength(20)
		expect(metric("gaia_api_security_signal_log_suppressed_total")).toBe(suppressedBefore + 5)
	})

	it("classifies an ordinary mistake as a plain validation failure, without tripping", async () => {
		const before = metric("gaia_api_graphql_rejected_total", '{reason="unknown_field"}')
		await query(`{ spaces(first: 1) { id notAField } }`, "203.0.113.16")
		expect(metric("gaia_api_graphql_rejected_total", '{reason="unknown_field"}')).toBe(before + 1)
		expect(signals()[0]).toMatchObject({signal: "validation_failed", hiddenSurface: []})
		expect(probesRaised()).toHaveLength(0)
	})

	it("counts unparseable documents", async () => {
		const before = metric("gaia_api_graphql_rejected_total", '{reason="parse"}')
		await query(`{ this is not graphql`, "203.0.113.17")
		expect(metric("gaia_api_graphql_rejected_total", '{reason="parse"}')).toBe(before + 1)
		expect(signals()[0]).toMatchObject({signal: "parse_failed", clientIp: "203.0.113.17"})
	})

	it("records introspection and still answers it", async () => {
		const before = metric("gaia_api_graphql_introspection_total", '{kind="schema"}')
		const res = await query(`{ __schema { queryType { name } } }`, "203.0.113.18")
		expect(res.body).toContain('"queryType"')
		expect(metric("gaia_api_graphql_introspection_total", '{kind="schema"}')).toBe(before + 1)
		expect(signals()[0]).toMatchObject({signal: "introspection", introspection: ["schema"]})
	})

	it("does not count a rejection when the operation that runs succeeds beside an unused mutation", async () => {
		const rejectedBefore = metric("gaia_api_graphql_rejected_total", '{reason="unknown_field"}')
		const introBefore = metric("gaia_api_graphql_introspection_total", '{kind="schema"}')
		const res = await post(
			JSON.stringify({
				query: "query Read { __schema { queryType { name } } } mutation Unused { bogus }",
				operationName: "Read",
			}),
			"203.0.113.23",
		)
		expect(res.body).toContain('"queryType"')
		expect(metric("gaia_api_graphql_rejected_total", '{reason="unknown_field"}')).toBe(rejectedBefore)
		expect(metric("gaia_api_graphql_introspection_total", '{kind="schema"}')).toBe(introBefore + 1)
		expect(signals().map((s) => s.signal)).toEqual(["introspection"])
	})

	it("still trips on a hidden mutation in an operation that never runs, without counting a rejection", async () => {
		const rejectedBefore = metric("gaia_api_graphql_rejected_total", '{reason="unknown_field"}')
		const probesBefore = metric(
			"gaia_api_graphql_hidden_surface_probe_total",
			'{target="Mutation.sampleFeedComposition"}',
		)
		await post(
			JSON.stringify({
				query: "query Read { spaces(first: 1) { id } } mutation Unused { sampleFeedComposition(input: {}) { clientMutationId } }",
				operationName: "Read",
			}),
			"203.0.113.24",
		)
		expect(metric("gaia_api_graphql_rejected_total", '{reason="unknown_field"}')).toBe(rejectedBefore)
		expect(metric("gaia_api_graphql_hidden_surface_probe_total", '{target="Mutation.sampleFeedComposition"}')).toBe(
			probesBefore + 1,
		)
		expect(signals()[0]).toMatchObject({signal: "hidden_surface_probe", clientIp: "203.0.113.24"})
	})

	it("sees probes and introspection through PostGraphile's nested query field", async () => {
		const introBefore = metric("gaia_api_graphql_introspection_total", '{kind="schema"}')
		const res = await query(`{ query { __schema { queryType { name } } } }`, "203.0.113.25")
		expect(res.body).toContain('"queryType"')
		expect(metric("gaia_api_graphql_introspection_total", '{kind="schema"}')).toBe(introBefore + 1)

		await query(`{ query { appWebhooks { id } } }`, "203.0.113.26")
		expect(signals().at(-1)).toMatchObject({signal: "hidden_surface_probe", hiddenSurface: ["Query.appWebhook"]})
	})

	it("stays silent for valid, non-introspection traffic", async () => {
		await query(`{ spaces(first: 1) { id } }`, "203.0.113.19")
		expect(signals()).toHaveLength(0)
		expect(probesRaised()).toHaveLength(0)
	})

	it("attributes to the Gateway's address, not a forged X-Real-IP", async () => {
		await query(`{ appWebhook(id: "x") { secret } }`, "198.51.100.1, 203.0.113.20", {"X-Real-IP": "192.0.2.99"})
		expect(signals()[0]).toMatchObject({clientIp: "203.0.113.20"})
	})

	it("never changes the response when detection itself fails", async () => {
		// A server with the same schema, whose analyzer always throws — the failure the realm bug caused.
		const broken = createYoga({
			schema: postgraphileSchema,
			plugins: [
				useSecuritySignals({
					analyze: () => {
						throw new Error("analyzer exploded")
					},
				}),
			],
		})
		const before = metric("gaia_api_security_signal_internal_errors_total")
		const res = await broken.fetch(
			new Request("http://localhost/graphql", {
				method: "POST",
				headers: {"Content-Type": "application/json", "X-Forwarded-For": "203.0.113.21"},
				body: JSON.stringify({query: "{ appWebhooks { secret } }"}),
			}),
		)
		const body = await res.text()
		expect(res.status).not.toBe(500)
		expect(body).toContain("Cannot query field")
		expect(body).not.toContain("analyzer exploded")
		expect(metric("gaia_api_security_signal_internal_errors_total")).toBe(before + 1)
		expect((warn.mock.calls as Logged[]).some(([m]) => m.startsWith("Security signal detection failed"))).toBe(true)
	})

	it("never changes the response when the logger itself throws", async () => {
		const broken = createYoga({
			schema: postgraphileSchema,
			plugins: [
				useSecuritySignals({
					analyze: () => {
						throw {
							toString: () => {
								throw new Error("hostile toString")
							},
						}
					},
				}),
			],
		})
		warn.mockImplementation(() => {
			throw new Error("logger down")
		})
		const res = await broken.fetch(
			new Request("http://localhost/graphql", {
				method: "POST",
				headers: {"Content-Type": "application/json", "X-Forwarded-For": "203.0.113.22"},
				body: JSON.stringify({query: "{ appWebhooks { secret } }"}),
			}),
		)
		expect(res.status).not.toBe(500)
		expect(await res.text()).toContain("Cannot query field")
	})
})
