import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId} from "../../geodb/card"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {analyzeSpaces, createSpacesShadow, runSpaces} from "../spacesShadow"

const NAME = conformingId("entity", "a126ca53-0c8e-48d5-b888-82c734c38935")
const SPACE_A = "32d38f87126c47678444b55859859fbc"
const SPACE_B = "6b8eecb16f2645c991f011b915370b84"
const PAGE_A = "7d81cb691cb349ee826870d56f8765ef"

const QUERY = `query S($ids: [UUID!]!) { spaces(filter: {id: {in: $ids}}) { id type topicId membersList { memberSpaceId } page { id name } } }`
const analyze = (query: string, variables: Record<string, unknown> = {}) => analyzeSpaces(parse(query), null, variables)

const cardRows = (name: string | null) => (name === null ? [] : [{prop: NAME, raw: name, ref: "-", entity: PAGE_A}])

function deps(options: {held?: string[]; name?: string | null; fail?: boolean} = {}) {
	const held = new Set(options.held ?? [SPACE_A])
	const readCard = vi.fn(async () => {
		if (options.fail) throw new Error("boom")
		return cardRows(options.name === undefined ? "Alpha" : options.name) as never
	})
	return {
		geoDb: {readCard} as never,
		geoSpaceOf: vi.fn(async (id: string) =>
			held.has(id) ? `${id.slice(0, 12)}4${id.slice(13, 16)}8${id.slice(17)}` : null,
		),
		log: {info: vi.fn(), warn: vi.fn()},
		readCard,
	}
}

const answer = (spaces: unknown[]) => ({data: {spaces}})
const space = (id: string, name: string | null = "Alpha") => ({
	id,
	type: "PERSONAL",
	topicId: null,
	membersList: [],
	page: {id: PAGE_A, name},
})

const metric = (name: string, labels: string) => {
	const line = renderGatewayMetrics()
		.split("\n")
		.find((l) => l.startsWith(`${name}{${labels}}`))
	return Number(line?.split(" ").pop())
}

beforeEach(() => __resetGatewayMetricsForTests())

describe("analyzeSpaces", () => {
	it("plans the id, the page name and the uncompared parts", () => {
		const a = analyze(QUERY)
		expect(a.ok).toBe(true)
		if (!a.ok) return
		expect(a.plan.parts.page).toEqual({key: "page", idKey: "id"})
		expect([...a.plan.uncompared].sort()).toEqual(["filter_completeness", "space_members", "space_settings"].sort())
	})

	it.each([
		["a second root", `{ spaces { id } metas { id } }`, "not_single_root"],
		["another root", `{ space(id: "x") { id } }`, "not_single_root"],
		["an unknown argument", `{ spaces(condition: {}) { id } }`, "args"],
		["an unknown field", `{ spaces { id wat } }`, "selection"],
		["no id", `{ spaces { type } }`, "selection"],
		["a name without its id", `{ spaces { id page { name } } }`, "selection"],
		["a directive", `{ spaces { id @skip(if: false) } }`, "directives"],
	])("refuses %s", (_label, query, reason) => {
		const a = analyze(query)
		expect(a).toEqual({ok: false, reason})
	})

	it("counts an ordering and nested parts it cannot check", () => {
		const a = analyze(`{ spaces(orderBy: ID_ASC) { id page { id avatar: relationsList { id } } } }`)
		expect(a.ok && a.plan.uncompared.sort()).toEqual(["nested_entity", "order_completeness"])
	})
})

describe("runSpaces", () => {
	const plan = () => {
		const a = analyze(QUERY)
		if (!a.ok) throw new Error("no plan")
		return a.plan
	}

	it("matches a held Space whose page name geo-db holds", async () => {
		const d = deps()
		const out = await runSpaces(d, plan(), answer([space(SPACE_A)]))
		expect(out).toMatchObject({result: "match"})
		expect(d.readCard).toHaveBeenCalledTimes(1)
	})

	it("mismatches when geo-db holds another name, and when it holds none", async () => {
		expect(await runSpaces(deps({name: "Beta"}), plan(), answer([space(SPACE_A)]))).toMatchObject({
			result: "mismatch",
		})
		expect(await runSpaces(deps({name: null}), plan(), answer([space(SPACE_A)]))).toMatchObject({
			result: "mismatch",
		})
	})

	it("treats an empty served name as no name", async () => {
		expect(await runSpaces(deps({name: null}), plan(), answer([space(SPACE_A, "")]))).toMatchObject({
			result: "match",
		})
	})

	it("counts a Space geo-db does not hold uncompared, and is unsupported when none is held", async () => {
		const mixed = await runSpaces(deps(), plan(), answer([space(SPACE_A), space(SPACE_B)]))
		expect(mixed).toMatchObject({result: "match"})
		expect((mixed as {uncompared: string[]}).uncompared).toContain("space_not_held")
		expect(await runSpaces(deps({held: []}), plan(), answer([space(SPACE_A)]))).toEqual({
			result: "unsupported",
			reason: "unmapped_space",
		})
	})

	it("reads at most the sample and notes the rest", async () => {
		const d = deps({held: [SPACE_A, SPACE_B]})
		const many = Array.from({length: 7}, (_, i) => space(i % 2 ? SPACE_A : SPACE_B))
		const out = await runSpaces(d, plan(), answer(many))
		expect(d.readCard).toHaveBeenCalledTimes(3)
		expect((out as {uncompared: string[]}).uncompared).toContain("sampled_rows")
	})

	it("skips an empty answer and fails on a geo-db error", async () => {
		expect(await runSpaces(deps(), plan(), answer([]))).toEqual({result: "skip"})
		expect(await runSpaces(deps(), plan(), {errors: []})).toEqual({result: "skip"})
		expect(await runSpaces(deps({fail: true}), plan(), answer([space(SPACE_A)]))).toEqual({result: "failed"})
	})
})

describe("createSpacesShadow", () => {
	const run = async (d: ReturnType<typeof deps>, query: string, body: unknown) => {
		const shadow = createSpacesShadow(d)
		await shadow({
			classification: {operationName: null, operationType: "query", rootFields: ["spaces"]},
			params: {query, operationName: null, variables: {ids: [SPACE_A]}},
			document: parse(query),
			response: new Response(JSON.stringify(body), {headers: {"content-type": "application/json"}}),
		})
	}

	it("exports its series at zero before any traffic", () => {
		createSpacesShadow(deps())
		expect(metric("gaia_api_gateway_shadow_total", 'field="spaces",result="match"')).toBe(0)
		expect(metric("gaia_api_gateway_shadow_uncompared_total", 'field="spaces",reason="space_members"')).toBe(0)
	})

	it("counts a match with its uncompared parts, and logs a mismatch as counts only", async () => {
		const d = deps()
		await run(d, QUERY, answer([space(SPACE_A)]))
		expect(metric("gaia_api_gateway_shadow_total", 'field="spaces",result="match"')).toBe(1)
		expect(metric("gaia_api_gateway_shadow_uncompared_total", 'field="spaces",reason="space_members"')).toBe(1)

		await run(deps({name: "Beta"}), QUERY, answer([space(SPACE_A)]))
		expect(metric("gaia_api_gateway_shadow_total", 'field="spaces",result="mismatch"')).toBe(1)
	})

	it("never logs an id or a name on a mismatch", async () => {
		const d = deps({name: "Beta"})
		await run(d, QUERY, answer([space(SPACE_A)]))
		const logged = JSON.stringify(d.log.warn.mock.calls)
		expect(logged).not.toContain(SPACE_A)
		expect(logged).not.toContain("Alpha")
		expect(logged).not.toContain("Beta")
	})

	it("counts an unsupported selection with its reason and never reads geo-db", async () => {
		const d = deps()
		await run(d, `{ spaces { id wat } }`, answer([]))
		expect(metric("gaia_api_gateway_shadow_total", 'field="spaces",result="unsupported_selection"')).toBe(1)
		expect(metric("gaia_api_gateway_shadow_unsupported_total", 'field="spaces",reason="selection"')).toBe(1)
		expect(d.readCard).not.toHaveBeenCalled()
	})

	it("counts a failed read and a malformed body without throwing", async () => {
		await run(deps({fail: true}), QUERY, answer([space(SPACE_A)]))
		expect(metric("gaia_api_gateway_shadow_total", 'field="spaces",result="failed"')).toBe(1)
		const shadow = createSpacesShadow(deps())
		await shadow({
			classification: {operationName: null, operationType: "query", rootFields: ["spaces"]},
			params: {query: QUERY, operationName: null, variables: {}},
			document: parse(QUERY),
			response: new Response("not json"),
		})
	})
})
