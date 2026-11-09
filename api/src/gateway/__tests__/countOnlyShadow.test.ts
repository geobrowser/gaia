import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {typeCountFromGeoDbOutput} from "../../geodb/card"
import {GeoDbError, typeCountDocument} from "../../geodb/client"
import {checkCountOnly, isCountRefusal, totalCountsOf} from "../countOnlyShadow"
import {createEntitiesConnectionShadow} from "../entitiesConnectionShadow"
import type {EntityShadowDeps} from "../entityShadow"
import {analyzeCountOnly, type CountOnlyPlan} from "../lightShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"

const SPACE = "0123456789ab4bcd8ef0123456789abc"
const TYPE = "e550fe517e904b2c8fffdf13408f5634"
const TYPE_B = "d7b3e5a1c2f44a6e9b8d0f1a2b3c4d5e"
const EDGE = "8f151ba4de204e3c9cb499ddf96f48f1"
const T1 = "11111111111141118111111111111111"
const T2 = "22222222222242228222222222222222"
const T3 = "33333333333343338333333333333333"
const T4 = "44444444444444448444444444444444"
const T5 = "55555555555545558555555555555555"

const counter = (alias: string, extra: string, targets: string, edgeOp = "is") =>
	`${alias}: entitiesConnection(${extra}filter: {and: [{relations: {some: {typeId: {is: "${EDGE}"}, toEntityId: {${edgeOp}: ${targets}}}}}, {typeIds: {overlaps: ["${TYPE}"]}}]}) { totalCount }`
const plan = (body: string, variables: Record<string, unknown> = {}) =>
	analyzeCountOnly(parse(`query { ${body} }`), null, variables)

describe("analyzeCountOnly", () => {
	it("lowers the topic page counter: relation some + type overlaps, totalCount only", () => {
		const a = plan(counter("n", `spaceId: "${SPACE}", `, `"${T1}"`))
		expect(a).toEqual({
			ok: true,
			plan: {roots: [{key: "n", spaceIds: [SPACE], typeId: TYPE, edge: {typeId: EDGE, targets: [T1]}}]},
		})
	})

	it("lowers several aliased roots, an `in` target list and no Space", () => {
		const a = plan(`${counter("a", "", `["${T1}", "${T2}"]`, "in")} ${counter("b", "", `"${T3}"`)}`)
		expect(a.ok).toBe(true)
		if (a.ok) {
			expect(a.plan.roots.map((r) => r.key)).toEqual(["a", "b"])
			expect(a.plan.roots[0]?.spaceIds).toBeNull()
			expect(a.plan.roots[0]?.edge?.targets).toEqual([T1, T2])
		}
	})

	it("lowers a type-only count (no relation)", () => {
		const a = plan(`n: entitiesConnection(typeIds: {in: ["${TYPE}"]}) { totalCount }`)
		expect(a.ok && a.plan.roots[0]?.edge).toBeNull()
	})

	it("reads variables", () => {
		const a = analyzeCountOnly(
			parse(`query ($t: UUID!) { ${counter("n", `spaceId: "${SPACE}", `, "$t")} }`),
			null,
			{t: T1},
		)
		expect(a.ok && a.plan.roots[0]?.edge?.targets).toEqual([T1])
	})

	it("refuses what the count form cannot state", () => {
		const refused = {ok: false, reason: "count_only"}
		// five targets (the form holds four)
		expect(plan(counter("n", "", `["${T1}", "${T2}", "${T3}", "${T4}", "${T5}"]`, "in"))).toEqual(refused)
		// two types: a union
		expect(
			plan(`n: entitiesConnection(filter: {typeIds: {overlaps: ["${TYPE}", "${TYPE_B}"]}}) { totalCount }`),
		).toEqual(refused)
		// two relation types
		expect(
			plan(
				`n: entitiesConnection(filter: {and: [{relations: {some: {typeId: {in: ["${EDGE}", "${T1}"]}, toEntityId: {is: "${T2}"}}}}, {typeIds: {overlaps: ["${TYPE}"]}}]}) { totalCount }`,
			),
		).toEqual(refused)
		// an id list
		expect(
			plan(
				`n: entitiesConnection(filter: {id: {in: ["${T1}"]}, typeIds: {overlaps: ["${TYPE}"]}}) { totalCount }`,
			),
		).toEqual(refused)
		// no type at all
		expect(plan(`n: entitiesConnection(spaceId: "${SPACE}") { totalCount }`)).toEqual(refused)
		// a nodes selection is not count-only
		expect(
			plan(`n: entitiesConnection(filter: {typeIds: {overlaps: ["${TYPE}"]}}) { totalCount nodes { id } }`),
		).toEqual(refused)
	})
})

describe("the count document and its output", () => {
	it("states one eq term and one in term", () => {
		expect(typeCountDocument({typeId: TYPE, where: {property: EDGE, targets: [T1]}})).toBe(
			`.{ .version = 1, .select = .{ .n = .{ .field = "entities", .args = .{ .type = "${TYPE}", .count = true, .where = .{ .@"${EDGE}" = .{ .eq = "${T1}" } } } } } }`,
		)
		expect(typeCountDocument({typeId: TYPE, where: {property: EDGE, targets: [T1, T2]}})).toContain(
			`.in = .{ "${T1}", "${T2}" }`,
		)
		expect(typeCountDocument({typeId: TYPE})).not.toContain(".where")
	})

	it("sums the pages and takes the last page's complete", () => {
		expect(typeCountFromGeoDbOutput("# plan x\n3\t10\t0\npartial: 1\n2\t5\t1\n")).toEqual({
			members: 5,
			scanned: 15,
			complete: true,
		})
		expect(typeCountFromGeoDbOutput("3\t10\t0\n")).toEqual({members: 3, scanned: 10, complete: false})
	})

	it("refuses no row and a bad row", () => {
		expect(() => typeCountFromGeoDbOutput("")).toThrow()
		expect(() => typeCountFromGeoDbOutput("1\t2\n")).toThrow()
		expect(() => typeCountFromGeoDbOutput("1\t2\t7\n")).toThrow()
		expect(() => typeCountFromGeoDbOutput("a\tb\tc\n")).toThrow()
	})
})

const deps = (read: ReturnType<typeof vi.fn>, extra: Partial<EntityShadowDeps> = {}) =>
	({
		countOnly: true,
		multiSpace: true,
		allGeoSpaces: async () => [],
		geoDb: {readTypeCount: read},
		geoSpaceOf: async (space: string) => (space === "dead" ? null : `geo${space}`),
		log: {info: vi.fn(), warn: vi.fn()},
		...extra,
	}) as unknown as EntityShadowDeps

const ONE: CountOnlyPlan = {roots: [{key: "n", spaceIds: [SPACE], typeId: TYPE, edge: {typeId: EDGE, targets: [T1]}}]}
const body = (n: unknown) => ({data: {n: {totalCount: n}}})

describe("checkCountOnly", () => {
	it("matches, and reads the conformed ids in the mapped Space", async () => {
		const read = vi.fn(async () => ({members: 7, scanned: 90, complete: true}))
		expect(await checkCountOnly(deps(read), ONE, body(7), "t")).toMatchObject({result: "match", uncompared: []})
		expect(read).toHaveBeenCalledWith({
			geoSpaceIds: [`geo${SPACE}`],
			typeId: TYPE,
			where: {property: EDGE, targets: [T1]},
		})
	})

	it("reads every Space for a root with no Space argument", async () => {
		const read = vi.fn(async (_: unknown) => ({members: 1, scanned: 1, complete: true}))
		const open: CountOnlyPlan = {roots: [{...(ONE.roots[0] as object), spaceIds: null} as never]}
		await checkCountOnly(deps(read), open, body(1), "t")
		expect(read.mock.calls[0]?.[0]).toMatchObject({geoSpaceIds: "every"})
		expect(await checkCountOnly(deps(read, {allGeoSpaces: undefined}), open, body(1), "t")).toEqual({
			result: "unsupported",
			reason: "no_space",
		})
	})

	it("flags a differing count with the counts and no ids", async () => {
		const read = vi.fn(async () => ({members: 6, scanned: 90, complete: true}))
		const out = await checkCountOnly(deps(read), ONE, body(7), "t")
		expect(out.result).toBe("mismatch")
		expect(JSON.stringify(out)).not.toContain(TYPE)
		expect(JSON.stringify(out)).toContain('"geoDb":6')
	})

	it("leaves an unfinished walk, a refusal, an unmapped Space and a throttle unsupported", async () => {
		const unfinished = vi.fn(async () => ({members: 1, scanned: 1, complete: false}))
		expect(await checkCountOnly(deps(unfinished), ONE, body(1), "t")).toEqual({
			result: "unsupported",
			reason: "count_incomplete",
		})
		const refused = vi.fn(async () => {
			throw new GeoDbError("client_failed", "geodb-client: MalformedRequest")
		})
		expect(await checkCountOnly(deps(refused), ONE, body(1), "t")).toEqual({
			result: "unsupported",
			reason: "count_refused",
		})
		const read = vi.fn()
		const unmapped: CountOnlyPlan = {roots: [{...(ONE.roots[0] as object), spaceIds: ["dead"]} as never]}
		expect(await checkCountOnly(deps(read), unmapped, body(1), "t")).toEqual({
			result: "unsupported",
			reason: "unmapped_space",
		})
		expect(
			await checkCountOnly(deps(read, {budget: {tryAcquire: () => false}} as never), ONE, body(1), "t"),
		).toEqual({result: "unsupported", reason: "throttled"})
		expect(read).not.toHaveBeenCalled()
	})

	it("fails on another read error and skips a response with no count", async () => {
		const broken = vi.fn(async () => {
			throw new Error("down")
		})
		expect(await checkCountOnly(deps(broken), ONE, body(1), "t")).toEqual({result: "failed"})
		const read = vi.fn()
		expect(await checkCountOnly(deps(read), ONE, {errors: [{}]}, "t")).toEqual({result: "skip"})
		expect(await checkCountOnly(deps(read), ONE, body(null), "t")).toEqual({result: "skip"})
		expect(read).not.toHaveBeenCalled()
	})

	it("recognises only the malformed-request refusal", () => {
		expect(isCountRefusal(new GeoDbError("client_failed", "x MalformedRequest"))).toBe(true)
		expect(isCountRefusal(new GeoDbError("timeout", "MalformedRequest"))).toBe(false)
		expect(isCountRefusal(new Error("MalformedRequest"))).toBe(false)
		expect(totalCountsOf(body(2), ONE)?.get("n")).toBe(2)
	})
})

describe("the entitiesConnection shadow with the count-only shape", () => {
	const text = `query { ${counter("n", `spaceId: "${SPACE}", `, `"${T1}"`)} }`
	const series = (prefix: string) =>
		Object.fromEntries(
			renderGatewayMetrics()
				.split("\n")
				.filter((l) => l.startsWith(`${prefix}{`) && !l.endsWith(" 0"))
				.map((l) => [l.match(/(?:result|reason)="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
		)
	const run = (shadowDeps: EntityShadowDeps, payload: unknown) =>
		createEntitiesConnectionShadow(shadowDeps)({
			classification: {operationName: null, operationType: "query", rootFields: []},
			params: {query: text, operationName: null, variables: {}},
			document: parse(text),
			response: Response.json(payload),
		} as never)

	beforeEach(() => __resetGatewayMetricsForTests())

	it("with the flag on compares the counts and records no count_only", async () => {
		const read = vi.fn(async () => ({members: 3, scanned: 3, complete: true}))
		await run(deps(read), body(3))
		expect(series("gaia_api_gateway_shadow_total")).toEqual({match: 1})
		expect(series("gaia_api_gateway_shadow_unsupported_total")).toEqual({})
		expect(read).toHaveBeenCalledTimes(1)
	})

	it("with the flag on records a differing count as a mismatch", async () => {
		const read = vi.fn(async () => ({members: 2, scanned: 3, complete: true}))
		await run(deps(read), body(3))
		expect(series("gaia_api_gateway_shadow_total")).toEqual({mismatch: 1})
	})

	it("with the flag off still counts it unsupported and reads nothing", async () => {
		const read = vi.fn()
		await run(deps(read, {countOnly: false}), body(3))
		expect(series("gaia_api_gateway_shadow_unsupported_total")).toEqual({count_only: 1})
		expect(read).not.toHaveBeenCalled()
	})
})
