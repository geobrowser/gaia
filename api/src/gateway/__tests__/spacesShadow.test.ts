import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId} from "../../geodb/card"
import {createMetasShadow} from "../metasShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {analyzeSpaces, createSpaceShadow, createSpacesShadow, runSpaces} from "../spacesShadow"

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

describe("space(id)", () => {
	const SQ = `query S($id: UUID!) { space(id: $id) { id type page { id name } } }`
	const run = async (d: ReturnType<typeof deps>, query: string, body: unknown) => {
		await createSpaceShadow(d)({
			classification: {operationName: null, operationType: "query", rootFields: ["space"]},
			params: {query, operationName: null, variables: {id: SPACE_A}},
			document: parse(query),
			response: new Response(JSON.stringify(body), {headers: {"content-type": "application/json"}}),
		})
	}

	it("plans one Space with no page-completeness parts, and refuses other args or roots", () => {
		const a = analyzeSpaces(parse(SQ), null, {id: SPACE_A}, "space")
		expect(a.ok && a.plan.single).toBe(true)
		expect(a.ok && a.plan.uncompared).toEqual(["space_settings"])
		expect(analyzeSpaces(parse(`{ space(id: "x", first: 1) { id } }`), null, {}, "space")).toEqual({
			ok: false,
			reason: "args",
		})
		expect(analyzeSpaces(parse(`{ space { id } }`), null, {}, "space")).toEqual({ok: false, reason: "args"})
		expect(analyzeSpaces(parse(SQ), null, {id: SPACE_A})).toEqual({ok: false, reason: "not_single_root"})
	})

	it("counts a match and a mismatch under its own field", async () => {
		await run(deps(), SQ, {data: {space: space(SPACE_A)}})
		expect(metric("gaia_api_gateway_shadow_total", 'field="space",result="match"')).toBe(1)
		await run(deps({name: "Beta"}), SQ, {data: {space: space(SPACE_A)}})
		expect(metric("gaia_api_gateway_shadow_total", 'field="space",result="mismatch"')).toBe(1)
	})

	it("is unsupported for a Space geo-db does not hold, and skips a null answer", async () => {
		await run(deps({held: []}), SQ, {data: {space: space(SPACE_A)}})
		expect(metric("gaia_api_gateway_shadow_unsupported_total", 'field="space",reason="unmapped_space"')).toBe(1)
		await run(deps(), SQ, {data: {space: null}})
		expect(metric("gaia_api_gateway_shadow_total", 'field="space",result="match"')).toBe(0)
	})
})

describe("createMetasShadow", () => {
	it("counts every operation unsupported (no_geodb_home) and reads nothing", async () => {
		const q = `{ metas { nodes { id blockNumber } } }`
		const d = deps()
		await createMetasShadow(d)({
			classification: {operationName: null, operationType: "query", rootFields: ["metas"]},
			params: {query: q, operationName: null, variables: {}},
			document: parse(q),
			response: new Response("{}"),
		})
		expect(metric("gaia_api_gateway_shadow_total", 'field="metas",result="unsupported_selection"')).toBe(1)
		expect(metric("gaia_api_gateway_shadow_unsupported_total", 'field="metas",reason="no_geodb_home"')).toBe(1)
		expect(d.readCard).not.toHaveBeenCalled()
	})
})

describe("createMetasShadow with a derived reader", () => {
	const q = `{ metas { nodes { id cursor blockNumber } } }`
	const value = (text: string) => Buffer.from(text, "utf8")
	const run = async (held: Record<string, string>, nodes: unknown[], query = q, fail = false) => {
		const get = vi.fn(async (_c: number, keyHex: string) => {
			if (fail) throw new Error("boom")
			const text = held[Buffer.from(keyHex, "hex").toString("utf8")]
			return text === undefined ? null : {group: "0".repeat(32), score: 0, value: value(text)}
		})
		const log = {info: vi.fn(), warn: vi.fn()}
		await createMetasShadow({log, reader: {get, close: vi.fn()}})({
			classification: {operationName: null, operationType: "query", rootFields: ["metas"]},
			params: {query, operationName: null, variables: {}},
			document: parse(query),
			response: new Response(JSON.stringify({data: {metas: {nodes}}})),
		})
		return {get, log}
	}
	const row = (id: string, cursor: string, blockNumber: string) => ({id, cursor, blockNumber})
	const result = (r: string) => metric("gaia_api_gateway_shadow_total", `field="metas",result="${r}"`)

	it("matches a row held at the same block and cursor, reading collection 10 by the id's bytes", async () => {
		const {get} = await run({w: "7:abc"}, [row("w", "abc", "7")])
		expect(result("match")).toBe(1)
		expect(get).toHaveBeenCalledWith(10, Buffer.from("w").toString("hex"))
	})

	it("counts a later block as a match checked for freshness only", async () => {
		await run({w: "9:zzz"}, [row("w", "abc", "7")])
		expect(result("match")).toBe(1)
		expect(metric("gaia_api_gateway_shadow_uncompared_total", 'field="metas",reason="mirror_newer"')).toBe(1)
	})

	it("is a mismatch when the mirror is behind, or the cursor differs at the same block", async () => {
		const behind = await run({w: "5:abc"}, [row("w", "abc", "7")])
		expect(result("mismatch")).toBe(1)
		expect(JSON.stringify(behind.log.warn.mock.calls)).not.toContain("abc")
		await run({w: "7:other"}, [row("w", "abc", "7")])
		expect(result("mismatch")).toBe(2)
	})

	it("counts an unmirrored row, a malformed value and a failed read apart", async () => {
		await run({}, [row("w", "abc", "7")])
		expect(metric("gaia_api_gateway_shadow_unsupported_total", 'field="metas",reason="not_mirrored"')).toBe(1)
		await run({w: "nocolon"}, [row("w", "abc", "7")])
		expect(metric("gaia_api_gateway_shadow_unsupported_total", 'field="metas",reason="bad_layout"')).toBe(1)
		await run({w: "7:abc"}, [row("w", "abc", "7")], q, true)
		expect(result("failed")).toBe(1)
	})

	it("needs the id selected, and marks a filtered page's membership uncompared", async () => {
		await run({w: "7:abc"}, [{cursor: "abc"}], `{ metas { nodes { cursor } } }`)
		expect(metric("gaia_api_gateway_shadow_unsupported_total", 'field="metas",reason="selection"')).toBe(1)
		await run(
			{w: "7:abc"},
			[row("w", "abc", "7")],
			`{ metas(condition: {id: "w"}) { nodes { id cursor blockNumber } } }`,
		)
		expect(result("match")).toBe(1)
		expect(metric("gaia_api_gateway_shadow_uncompared_total", 'field="metas",reason="meta_filter"')).toBe(1)
	})
})
