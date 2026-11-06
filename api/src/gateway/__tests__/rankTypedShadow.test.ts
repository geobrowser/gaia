import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import type {DerivedScan, DerivedScanReader} from "../../geodb/derived"
import {classifyRequestWithDocument} from "../classify"
import {readGatewayConfig} from "../config"
import {createGateway} from "../gateway"
import {
	__resetRankTypedMetricsForTests,
	checkOp,
	comparePage,
	compareRows,
	createRankTypedObserver,
	rankOps,
	readRankTypedConfig,
	renderRankTypedMetrics,
} from "../rankTypedShadow"

const TYPE = "11".repeat(16)
const id = (n: number) => n.toString(16).padStart(32, "0")
const row = (n: number, score: number) => ({score, keyHex: TYPE + id(n), valueHex: ""})
const scan = (rows: ReturnType<typeof row>[], more = false): DerivedScan => ({rows, more})
const node = (n: number, score: number | null | undefined) => ({id: id(n), score})
const uuid = (hex: string) =>
	`${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`

const ops = (query: string, variables: Record<string, unknown> = {}, name: string | null = null) =>
	rankOps(parse(query), name, variables)

describe("config", () => {
	it("is off unless set", () => {
		expect(readRankTypedConfig({}).mode).toBe("off")
		expect(readRankTypedConfig({GEODB_RANK_TYPED_SHADOW: "count"}).mode).toBe("count")
		expect(readRankTypedConfig({GEODB_RANK_TYPED_SHADOW: "on"}).mode).toBe("on")
		expect(
			readRankTypedConfig({GEODB_RANK_TYPED_SHADOW: "on", GEODB_RANK_TYPED_SHADOW_SAMPLE: "7"}).sampleRate,
		).toBe(0.25)
	})
})

describe("rankOps", () => {
	const T = uuid(TYPE)
	it("reads a typed first page", () => {
		const found = ops(
			`query { entitiesConnection(typeId: "${T}", orderBy: [RANKING_SCORE_DESC], first: 20) { nodes { id } } }`,
		)
		expect(found).toHaveLength(1)
		expect(found[0]).toMatchObject({
			typed: "single",
			typeHex: TYPE,
			cursor: false,
			narrowed: false,
			first: 20,
			order: "RANKING_SCORE_DESC",
		})
	})
	it("resolves variables, aliases, fragments and skip/include", () => {
		const q = `query Q($t: UUID!, $a: Cursor, $no: Boolean!) {
			...F
			hidden: entities(typeId: $t, orderBy: RANKING_SCORE_DESC) @skip(if: $no) { id }
		}
		fragment F on Query { top: entitiesConnection(typeIds: {in: [$t]}, orderBy: [QUALITY_SCORE_ASC], after: $a, filter: {name: {is: "x"}}) { nodes { id } } }`
		const found = ops(q, {t: T, a: "c", no: true}, "Q")
		expect(found.map((o) => o.key)).toEqual(["top"])
		expect(found[0]).toMatchObject({typed: "single", cursor: true, narrowed: true, order: "QUALITY_SCORE_ASC"})
	})
	it("classifies type-less and multi-type, and ignores other orders and mutations", () => {
		expect(ops(`query { entities(orderBy: RANKING_SCORE_DESC) { id } }`)[0]?.typed).toBe("none")
		expect(
			ops(`query { entities(typeIds: {in: ["${T}", "${uuid(id(2))}"]}, orderBy: RANKING_SCORE_DESC) { id } }`)[0]
				?.typed,
		).toBe("multi")
		expect(ops(`query { entities(typeId: "${T}", orderBy: NAME_ASC) { id } }`)).toEqual([])
		expect(ops(`mutation { entities(orderBy: RANKING_SCORE_DESC) { id } }`)).toEqual([])
	})
	it("treats an empty filter as no narrowing", () => {
		expect(
			ops(`query { entities(typeId: "${T}", orderBy: RANKING_SCORE_DESC, filter: {}) { id } }`)[0]?.narrowed,
		).toBe(false)
	})
})

describe("comparePage", () => {
	it("matches the same order", () => {
		const r = comparePage(TYPE, [node(1, 3), node(2, 2), node(3, 1)], 20, scan([row(1, 3), row(2, 2), row(3, 1)]))
		expect(r).toMatchObject({result: "match"})
	})
	it("treats tied rows as a set", () => {
		const r = comparePage(TYPE, [node(2, 2), node(1, 2), node(3, 1)], 20, scan([row(1, 2), row(2, 2), row(3, 1)]))
		expect(r).toMatchObject({result: "match", uncompared: ["ties"]})
	})
	it("flags a different order", () => {
		expect(comparePage(TYPE, [node(2, 2), node(1, 3)], 20, scan([row(1, 3), row(2, 2)]))).toMatchObject({
			result: "mismatch",
		})
	})
	it("flags a differing score", () => {
		expect(comparePage(TYPE, [node(1, 3.5)], 20, scan([row(1, 3)]))).toMatchObject({result: "mismatch"})
	})
	it("accepts a tiny float difference", () => {
		expect(comparePage(TYPE, [node(1, 3 + 1e-12)], 20, scan([row(1, 3)]))).toMatchObject({result: "match"})
	})
	it("is the unscored tail after the scored rows, which geo-db does not hold", () => {
		expect(comparePage(TYPE, [node(1, 3), node(9, null)], 20, scan([row(1, 3)]))).toMatchObject({
			result: "match",
			uncompared: ["unscored_tail"],
		})
		expect(comparePage(TYPE, [node(1, 3), node(2, null)], 20, scan([row(1, 3), row(2, 1)]))).toMatchObject({
			result: "mismatch",
		})
		expect(comparePage(TYPE, [node(1, 3), node(9, 4)], 20, scan([row(1, 3)]))).toEqual({
			result: "unsupported",
			reason: "row_missing",
		})
	})
	it("flags a short page that geo-db has more rows than", () => {
		expect(comparePage(TYPE, [node(1, 3)], 20, scan([row(1, 3), row(2, 2)]))).toMatchObject({result: "mismatch"})
	})
	it("cannot place a tie group cut by the scan", () => {
		expect(comparePage(TYPE, [node(9, 2)], 1, scan([row(1, 2), row(2, 2)], true))).toEqual({
			result: "unsupported",
			reason: "ties",
		})
	})
	it("is not mirrored when the group is empty, and bad layout on a foreign key", () => {
		expect(comparePage(TYPE, [node(1, 1)], 20, scan([]))).toEqual({result: "unsupported", reason: "not_mirrored"})
		expect(comparePage(TYPE, [node(1, 1)], 20, scan([{score: 1, keyHex: "22".repeat(32), valueHex: ""}]))).toEqual({
			result: "unsupported",
			reason: "bad_layout",
		})
	})
	it("notes a page that did not select the score", () => {
		expect(comparePage(TYPE, [node(1, undefined)], 20, scan([row(1, 3)]))).toMatchObject({
			uncompared: ["no_score_field"],
		})
	})
})

describe("compareRows", () => {
	it("matches scores in descending order", () => {
		expect(compareRows([node(1, 3), node(2, 2)], [{score: 3}, {score: 2}], ["filter"])).toMatchObject({
			result: "match",
			uncompared: ["filter", "sampled_rows"],
		})
	})
	it("flags a rising order, a wrong score and a held unscored row", () => {
		expect(compareRows([node(1, 2), node(2, 3)], [{score: 2}, {score: 3}], [])).toMatchObject({result: "mismatch"})
		expect(compareRows([node(1, 2)], [{score: 5}], [])).toMatchObject({result: "mismatch"})
		expect(compareRows([node(1, null)], [{score: 5}], [])).toMatchObject({result: "mismatch"})
	})
	it("is unsupported for a missing scored row and allows a missing unscored one", () => {
		expect(compareRows([node(1, 2)], [null], [])).toEqual({result: "unsupported", reason: "row_missing"})
		expect(compareRows([node(1, null)], [null], [])).toMatchObject({result: "match"})
	})
})

describe("checkOp and the observer", () => {
	beforeEach(() => __resetRankTypedMetricsForTests())
	const T = uuid(TYPE)
	const reader = (rows: ReturnType<typeof row>[], more = false) =>
		({
			scan: vi.fn(async () => scan(rows, more)),
			get: vi.fn(async (_c: number, key: string) => {
				const found = rows.find((r) => r.keyHex === key)
				return found ? {group: TYPE, score: found.score, value: ""} : null
			}),
			close: vi.fn(async () => {}),
		}) as unknown as DerivedScanReader & {scan: ReturnType<typeof vi.fn>; get: ReturnType<typeof vi.fn>}
	const config = {mode: "on" as const, sampleRate: 1, perSecond: 100, burst: 100}
	const budget = {tryAcquire: () => true}
	const log = {warn: vi.fn()}
	const query = (args: string) =>
		`query Q { entitiesConnection(typeId: "${T}", orderBy: [RANKING_SCORE_DESC], ${args}) { nodes { id rankingScore } } }`
	const data = (nodes: {id: string; rankingScore: number | null}[]) => ({
		entitiesConnection: {nodes: nodes.map((n) => ({...n, id: uuid(n.id)}))},
	})
	const metric = (name: string) =>
		renderRankTypedMetrics()
			.split("\n")
			.filter((l) => l.startsWith(name))

	it("compares a plain page with one scan", async () => {
		const r = reader([row(1, 3), row(2, 2)])
		const [op] = ops(query("first: 2"))
		await checkOp(
			{config, reader: r, budget, log},
			op!,
			data([
				{id: id(1), rankingScore: 3},
				{id: id(2), rankingScore: 2},
			]),
		)
		expect(r.scan).toHaveBeenCalledWith(0, TYPE, 34)
		expect(renderRankTypedMetrics()).toBe("")
	})

	it("reads narrowed and cursor pages row by row", async () => {
		const r = reader([row(1, 3), row(2, 2)])
		const [op] = ops(query(`first: 2, after: "c"`))
		await checkOp(
			{config, reader: r, budget, log},
			op!,
			data([
				{id: id(1), rankingScore: 3},
				{id: id(2), rankingScore: 2},
			]),
		)
		expect(r.scan).not.toHaveBeenCalled()
		expect(r.get).toHaveBeenCalledTimes(2)
	})

	it("is inert when off, and counts only in count mode", async () => {
		const r = reader([row(1, 3)])
		const classified = async (q: string) =>
			classifyRequestWithDocument(
				new Request("http://api/graphql", {
					method: "POST",
					headers: {"content-type": "application/json"},
					body: JSON.stringify({query: q}),
				}),
			)
		const off = createRankTypedObserver({config: {...config, mode: "off"}, reader: r, budget, log})
		expect(off.inspect(await classified(query("first: 1")))).toBe(false)
		expect(renderRankTypedMetrics()).toBe("")
		const count = createRankTypedObserver({config: {...config, mode: "count"}, reader: r, budget, log})
		expect(count.inspect(await classified(query("first: 1")))).toBe(false)
		expect(metric("gaia_api_ranking_score_orders_total{").filter((l) => !l.endsWith(" 0"))).toEqual([
			'gaia_api_ranking_score_orders_total{field="entitiesConnection",order="RANKING_SCORE_DESC",typed="single",narrowed="no",page="first"} 1',
		])
		expect(renderRankTypedMetrics()).not.toContain("rank_typed_shadow")
		expect(r.scan).not.toHaveBeenCalled()
	})

	it("through the gateway: counts, shadows on a clone, never changes the response", async () => {
		const r = reader([row(1, 3)])
		const observer = createRankTypedObserver({config, reader: r, budget, log})
		const body = JSON.stringify({data: data([{id: id(1), rankingScore: 3}])})
		const delegate = vi.fn(async () => new Response(body, {headers: {"content-type": "application/json"}}))
		const gateway = createGateway({
			config: readGatewayConfig({GRAPHQL_GATEWAY: "on"}),
			delegate,
			observers: [observer],
		})
		const response = await gateway(
			new Request("http://api/graphql", {
				method: "POST",
				headers: {"content-type": "application/json"},
				body: JSON.stringify({query: query("first: 1"), operationName: "Q"}),
			}),
			{},
		)
		expect(await response.text()).toBe(body)
		await vi.waitFor(() =>
			expect(metric('gaia_api_rank_typed_shadow_total{mode="page",result="match"')[0]).toMatch(/ 1$/),
		)
		expect(
			metric("gaia_api_rank_typed_shadow_uncompared_total").every((l) => l.startsWith("#") || / 0$/.test(l)),
		).toBe(true)
	})

	it("counts the unsupported reasons", async () => {
		const r = reader([])
		const deps = {config, reader: r, budget, log}
		createRankTypedObserver(deps) // mounts the series
		await checkOp(deps, ops(`query { entities(orderBy: RANKING_SCORE_DESC) { id } }`)[0]!, {
			entities: [{id: uuid(id(1))}],
		})
		await checkOp(deps, ops(query("first: 1"))[0]!, data([{id: id(1), rankingScore: 1}]))
		await checkOp(
			{...deps, budget: {tryAcquire: () => false}},
			ops(query("first: 1"))[0]!,
			data([{id: id(1), rankingScore: 1}]),
		)
		const lines = metric("gaia_api_rank_typed_shadow_unsupported_total{")
		expect(lines).toContain('gaia_api_rank_typed_shadow_unsupported_total{reason="type_less"} 1')
		expect(lines).toContain('gaia_api_rank_typed_shadow_unsupported_total{reason="not_mirrored"} 1')
		expect(lines).toContain('gaia_api_rank_typed_shadow_unsupported_total{reason="throttled"} 1')
	})

	it("counts a failed read and does not throw", async () => {
		const r = reader([])
		r.scan.mockRejectedValue(new Error("boom"))
		await checkOp(
			{config, reader: r, budget, log},
			ops(query("first: 1"))[0]!,
			data([{id: id(1), rankingScore: 1}]),
		)
		expect(log.warn).toHaveBeenCalled()
	})
})
