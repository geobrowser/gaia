import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {noteUnsupported, redactedQuery} from "../lightShadow"
import {__resetGatewayMetricsForTests} from "../metrics"

const UUID = "5093b6ab-1b48-4111-9dd3-1275b5413ddb"
const QUERY = `{ entities(filter: {id: {in: ["${UUID}", "x"]}}, first: 50) { id name(x: "secret") valuesList { text } } }`

describe("unsupported log", () => {
	beforeEach(() => __resetGatewayMetricsForTests())

	it("redacts every literal but keeps names, keys and small integers", () => {
		const operation = parse(QUERY).definitions[0]
		if (!operation) throw new Error("no operation")
		const text = redactedQuery(operation)
		expect(text).not.toContain(UUID)
		expect(text).not.toContain("secret")
		expect(text).toContain("filter: {id: {in: []}}")
		expect(text).toContain("first: 50")
		expect(text).toContain("valuesList")
	})

	it("logs anonymous operations of different shapes separately, once each, with the redacted query", () => {
		const log = {warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn()}
		const note = (query: string) => noteUnsupported({log} as never, "entities", "selection", parse(query), null)
		note(QUERY)
		note(QUERY)
		note(`{ entities(first: 5) { id } }`)
		expect(log.warn).toHaveBeenCalledTimes(2)
		const [, fields] = log.warn.mock.calls[0] ?? []
		expect(fields.query).toContain("entities(filter:")
		expect(JSON.stringify(fields)).not.toContain(UUID)
	})

	it("shows the inner keys of a nested filter in the shape, with no values", () => {
		const log = {warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn()}
		noteUnsupported(
			{log} as never,
			"entities",
			"selection",
			parse(
				`{ entities(first: 5, filter: {and: [{relations: {some: {typeId: {is: "${UUID}"}, toEntityId: {is: "${UUID}"}}}}], typeIds: {anyEqualTo: "${UUID}"}}) { id } }`,
			),
			null,
		)
		const [, fields] = log.warn.mock.calls[0] ?? []
		expect(fields.shape).toBe(
			"args=filter,first;filter=and([relations(some(toEntityId(is),typeId(is)))]),typeIds(anyEqualTo)",
		)
		expect(JSON.stringify(fields)).not.toContain(UUID)
	})

	it("gives later shapes the label `other` once the per-process shape budget is spent", () => {
		const log = {warn: vi.fn(), info: vi.fn(), error: vi.fn(), debug: vi.fn()}
		for (let i = 0; i < 400; i++)
			noteUnsupported(
				{log} as never,
				"entities",
				"selection",
				parse(`{ entities(first: 1, filter: {k${i}: {is: "x"}}) { id } }`),
				null,
			)
		const shapes = log.warn.mock.calls.map(([, f]) => f.shape)
		expect(shapes.length).toBeLessThanOrEqual(200)
		expect(new Set(shapes).size).toBeGreaterThan(1)
	})
})
