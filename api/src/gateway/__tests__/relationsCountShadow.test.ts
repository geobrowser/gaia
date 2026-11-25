import {readFileSync} from "node:fs"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {GeoDbError, relationCountDocument} from "../../geodb/client"
import {readGeoDbCardConfig} from "../../geodb/config"
import type {EntityShadowDeps} from "../entityShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {createPointReadShadows} from "../pointReadShadows"
import {analyzeRelationsCount, checkRelationsCount, type RelationsCountPlan} from "../relationsCountShadow"

const SPACE = "0123456789ab4bcd8ef0123456789abc"
const EDGE = "8f151ba4de204e3c9cb499ddf96f48f1"
const EDGE_B = "d7b3e5a1c2f44a6e9b8d0f1a2b3c4d5e"
const OTHER = "e550fe517e904b2c8fffdf13408f5634"
const FROM = "aaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa"
const T1 = "11111111111141118111111111111111"
const T2 = "22222222222242228222222222222222"
const T3 = "33333333333343338333333333333333"
const target = (n: number) => `${n.toString(16).padStart(8, "0")}000040008000000000000000`

const analyze = (query: string, variables: Record<string, unknown> = {}) =>
	analyzeRelationsCount(parse(query), null, variables)

describe("analyzeRelationsCount: a target set", () => {
	it("lowers `typeId is` + `toEntityId in` (totalCount only) to one count read a target", () => {
		const a = analyze(
			`query { relationsConnection(filter: {typeId: {is: "${EDGE}"}, toEntityId: {in: ["${T1}", "${T2}"]}, spaceId: {is: "${SPACE}"}}) { totalCount } }`,
		)
		expect(a).toEqual({
			ok: true,
			plan: {
				roots: [
					{key: "relationsConnection", route: "targets", spaceIds: [SPACE], typeId: EDGE, targets: [T1, T2]},
				],
			},
		})
	})

	it("lowers one target, no Space (counted across every Space), aliases and variables", () => {
		const a = analyze(
			`query ($t: UUID!, $r: UUID!) { supported: relationsConnection(first: 5, filter: {typeId: {equalTo: $r}, toEntityId: {is: $t}}) { totalCount __typename } }`,
			{t: T3, r: EDGE},
		)
		expect(a).toEqual({
			ok: true,
			plan: {roots: [{key: "supported", route: "targets", spaceIds: null, typeId: EDGE, targets: [T3]}]},
		})
	})

	it("stays unsupported past MAX_BATCH_IDS targets (20), in one root or summed over roots", () => {
		const many = Array.from({length: 21}, (_, i) => `"${target(i + 1)}"`).join(", ")
		expect(
			analyze(
				`query { relationsConnection(filter: {typeId: {is: "${EDGE}"}, toEntityId: {in: [${many}]}}) { totalCount } }`,
			),
		).toEqual({ok: false, reason: "batch_size"})
		const eleven = (from: number) => Array.from({length: 11}, (_, i) => `"${target(from + i)}"`).join(", ")
		expect(
			analyze(
				`query { a: relationsConnection(filter: {typeId: {is: "${EDGE}"}, toEntityId: {in: [${eleven(1)}]}}) { totalCount } b: relationsConnection(filter: {typeId: {is: "${EDGE}"}, toEntityId: {in: [${eleven(40)}]}}) { totalCount } }`,
			),
		).toEqual({ok: false, reason: "batch_size"})
	})
})

describe("analyzeRelationsCount: a from entity (the card's relations)", () => {
	it("lowers `fromEntityId is` + one `spaceId` + Types (and optional targets) to the entity's card", () => {
		const a = analyze(
			`query { n: relationsConnection(filter: {fromEntityId: {is: "${FROM}"}, spaceId: {is: "${SPACE}"}, typeId: {in: ["${EDGE}", "${EDGE_B}"]}}) { totalCount } }`,
		)
		expect(a).toEqual({
			ok: true,
			plan: {
				roots: [
					{
						key: "n",
						route: "card",
						spaceId: SPACE,
						fromEntityId: FROM,
						typeIds: [EDGE, EDGE_B],
						targets: null,
					},
				],
			},
		})
		const pinned = analyze(
			`query { n: relationsConnection(filter: {fromEntityId: {is: "${FROM}"}, spaceId: {is: "${SPACE}"}, typeId: {is: "${EDGE}"}, toEntityId: {in: ["${T1}"]}}) { totalCount } }`,
		)
		expect(pinned.ok && pinned.plan.roots[0]).toMatchObject({route: "card", targets: [T1]})
	})
})

describe("analyzeRelationsCount: what neither route can state", () => {
	const refused = {ok: false, reason: "count_only"}
	const root = (filter: string, selection = "totalCount") =>
		analyze(`query { relationsConnection(filter: ${filter}) { ${selection} } }`)

	it("refuses filters that only narrow Postgres' rows, or pin nothing countable", () => {
		// a nested object filter (the facet counters) and a non-pinning operator
		expect(
			root(
				`{typeId: {is: "${EDGE}"}, toEntityId: {is: "${T1}"}, fromEntity: {typeIds: {anyEqualTo: "${OTHER}"}}}`,
			),
		).toEqual(refused)
		expect(root(`{typeId: {is: "${EDGE}"}, toEntityId: {is: "${T1}"}, spaceId: {isNull: false}}`)).toEqual(refused)
		// two relation Types under a target set (one count read states one Type)
		expect(root(`{typeId: {in: ["${EDGE}", "${EDGE_B}"]}, toEntityId: {is: "${T1}"}}`)).toEqual(refused)
		// no Type, no target
		expect(root(`{toEntityId: {is: "${T1}"}}`)).toEqual(refused)
		expect(root(`{typeId: {is: "${EDGE}"}}`)).toEqual(refused)
		// a from entity with no Space, two Spaces, or two from entities
		expect(root(`{fromEntityId: {is: "${FROM}"}, typeId: {is: "${EDGE}"}}`)).toEqual(refused)
		expect(
			root(`{fromEntityId: {is: "${FROM}"}, spaceId: {in: ["${SPACE}", "${OTHER}"]}, typeId: {is: "${EDGE}"}}`),
		).toEqual(refused)
		expect(
			root(`{fromEntityId: {in: ["${FROM}", "${OTHER}"]}, spaceId: {is: "${SPACE}"}, typeId: {is: "${EDGE}"}}`),
		).toEqual(refused)
		// an id or own-entity column
		expect(root(`{id: {in: ["${T1}"]}}`)).toEqual(refused)
		expect(root(`{entityId: {is: "${T1}"}, typeId: {is: "${EDGE}"}, toEntityId: {is: "${T2}"}}`)).toEqual(refused)
	})

	it("refuses a derived (Postgres-only) relation Type", () => {
		const derived = "890deffb384349fa826974000e3dcef6"
		expect(root(`{typeId: {is: "${derived}"}, toEntityId: {is: "${T1}"}}`).ok).toBe(false)
	})

	it("refuses a selection beside totalCount, and a `condition` argument", () => {
		const filter = `{typeId: {is: "${EDGE}"}, toEntityId: {is: "${T1}"}}`
		expect(root(filter, "aggregates { distinctCount { fromEntityId } }")).toEqual(refused)
		expect(root(filter, "totalCount nodes { id }")).toEqual(refused)
		expect(root(filter, "totalCount pageInfo { hasNextPage }")).toEqual(refused)
		expect(
			analyze(`query { relationsConnection(condition: {typeId: "${EDGE}"}, filter: ${filter}) { totalCount } }`),
		).toEqual(refused)
	})
})

describe("the relations count document", () => {
	it("is the `relations` root with `.type`, `.to` and `.count = true`, nothing else", () => {
		expect(relationCountDocument({typeId: EDGE, toEntityId: T1})).toBe(
			`.{ .version = 1, .select = .{ .n = .{ .field = "relations", .args = .{ .type = "${EDGE}", .to = "${T1}", .count = true } } } }`,
		)
	})
})

const deps = (geoDb: Record<string, unknown>, extra: Partial<EntityShadowDeps> = {}) =>
	({
		relationsCount: true,
		multiSpace: true,
		geoDb,
		geoSpaceOf: async (space: string) => (space === "dead" ? null : `geo${space}`.slice(0, 32)),
		log: {info: vi.fn(), warn: vi.fn()},
		...extra,
	}) as unknown as EntityShadowDeps

const GEO_SPACE = `geo${SPACE}`.slice(0, 32)
const TARGETS: RelationsCountPlan = {
	roots: [{key: "n", route: "targets", spaceIds: [SPACE], typeId: EDGE, targets: [T1, T2]}],
}
const body = (n: unknown) => ({data: {n: {totalCount: n}}})
/** One count read a target: `members` distinct from-entities, `scanned` statements (rows). */
const perTarget = (counts: Record<string, {members: number; scanned: number; complete?: boolean}>) =>
	vi.fn(async ({toEntityId}: {toEntityId: string}) => ({complete: true, ...counts[toEntityId]}))

describe("checkRelationsCount: a target set", () => {
	it("sums `scanned` over the targets and compares it with totalCount, never `members`", async () => {
		// One from-entity holds two statements to T1 (members 1, scanned 2); T2 has one (members 1, scanned 1).
		const read = perTarget({[T1]: {members: 1, scanned: 2}, [T2]: {members: 1, scanned: 1}})
		const d = deps({readRelationCount: read})
		expect(await checkRelationsCount(d, TARGETS, body(3), "t")).toMatchObject({result: "match", uncompared: []})
		expect(read).toHaveBeenCalledTimes(2)
		expect(read).toHaveBeenCalledWith({geoSpaceIds: [GEO_SPACE], typeId: EDGE, toEntityId: T1})
		expect(read).toHaveBeenCalledWith({geoSpaceIds: [GEO_SPACE], typeId: EDGE, toEntityId: T2})
		// The summed members (2) is not the row count: totalCount 2 is a mismatch.
		const off = await checkRelationsCount(d, TARGETS, body(2), "t")
		expect(off).toMatchObject({
			result: "mismatch",
			detail: {relationsCount: true, differing: [{root: 0, route: "targets", postgres: 2, geoDb: 3, members: 2}]},
		})
	})

	it("is count_refused on a serve without the form, count_incomplete on an unfinished walk, failed on an error", async () => {
		const refused = vi.fn(async () => {
			throw new GeoDbError("client_failed", "error: MalformedRequest at 1:40")
		})
		expect(await checkRelationsCount(deps({readRelationCount: refused}), TARGETS, body(3), "t")).toEqual({
			result: "unsupported",
			reason: "count_refused",
		})
		const unfinished = perTarget({[T1]: {members: 1, scanned: 2, complete: false}, [T2]: {members: 1, scanned: 1}})
		expect(await checkRelationsCount(deps({readRelationCount: unfinished}), TARGETS, body(3), "t")).toEqual({
			result: "unsupported",
			reason: "count_incomplete",
		})
		const broken = vi.fn(async () => {
			throw new GeoDbError("timeout", "slow")
		})
		expect(await checkRelationsCount(deps({readRelationCount: broken}), TARGETS, body(3), "t")).toEqual({
			result: "failed",
		})
	})

	it("takes one budget unit a read and is throttled when the bucket runs dry", async () => {
		const read = perTarget({[T1]: {members: 1, scanned: 1}, [T2]: {members: 1, scanned: 1}})
		let left = 1
		const budget = {
			tryAcquire: vi.fn((n: number) => {
				if (left < n) return false
				left -= n
				return true
			}),
		}
		expect(await checkRelationsCount(deps({readRelationCount: read}, {budget}), TARGETS, body(2), "t")).toEqual({
			result: "unsupported",
			reason: "throttled",
		})
		expect(budget.tryAcquire).toHaveBeenCalledWith(1)
	})

	it("reads every Space (`*`) for a root with no Space only with the every-Space reads on", async () => {
		const everywhere: RelationsCountPlan = {
			roots: [{key: "n", route: "targets", spaceIds: null, typeId: EDGE, targets: [T1]}],
		}
		const read = perTarget({[T1]: {members: 4, scanned: 5}})
		expect(await checkRelationsCount(deps({readRelationCount: read}), everywhere, body(5), "t")).toEqual({
			result: "unsupported",
			reason: "no_space",
		})
		expect(
			await checkRelationsCount(
				deps({readRelationCount: read}, {allGeoSpaces: async () => []}),
				everywhere,
				body(5),
				"t",
			),
		).toMatchObject({result: "match"})
		expect(read).toHaveBeenCalledWith({geoSpaceIds: "every", typeId: EDGE, toEntityId: T1})
	})

	it("is unmapped_space for a Space geo-db has not loaded, and skips a client without the read or a bad body", async () => {
		const dead: RelationsCountPlan = {
			roots: [{key: "n", route: "targets", spaceIds: ["dead"], typeId: EDGE, targets: [T1]}],
		}
		expect(await checkRelationsCount(deps({readRelationCount: vi.fn()}), dead, body(1), "t")).toEqual({
			result: "unsupported",
			reason: "unmapped_space",
		})
		expect(await checkRelationsCount(deps({}), TARGETS, body(1), "t")).toEqual({result: "skip"})
		expect(await checkRelationsCount(deps({readRelationCount: vi.fn()}), TARGETS, {errors: [{}]}, "t")).toEqual({
			result: "skip",
		})
	})
})

const CARD: RelationsCountPlan = {
	roots: [{key: "n", route: "card", spaceId: SPACE, fromEntityId: FROM, typeIds: [EDGE], targets: null}],
}

describe("checkRelationsCount: a from entity's card", () => {
	const rows = [
		{prop: EDGE, raw: T1},
		{prop: EDGE, raw: T1},
		{prop: EDGE, raw: T2},
		{prop: EDGE_B, raw: T1},
		{prop: OTHER, raw: "text:hello"},
	]

	it("counts the card's statements of the Types (and targets) and compares with totalCount", async () => {
		const readCard = vi.fn(async () => rows)
		const d = deps({readCard})
		expect(await checkRelationsCount(d, CARD, body(3), "t")).toMatchObject({result: "match"})
		expect(readCard).toHaveBeenCalledWith({geoSpaceId: GEO_SPACE, entityId: FROM})
		expect(await checkRelationsCount(d, CARD, body(4), "t")).toMatchObject({
			result: "mismatch",
			detail: {relationsCount: true, differing: [{root: 0, route: "card", postgres: 4, geoDb: 3}]},
		})
		const pinned: RelationsCountPlan = {
			roots: [
				{key: "n", route: "card", spaceId: SPACE, fromEntityId: FROM, typeIds: [EDGE, EDGE_B], targets: [T1]},
			],
		}
		expect(await checkRelationsCount(d, pinned, body(3), "t")).toMatchObject({result: "match"})
	})

	it("is unmapped_space for an unloaded Space, failed on a read error, throttled on an empty budget", async () => {
		const dead: RelationsCountPlan = {
			roots: [{key: "n", route: "card", spaceId: "dead", fromEntityId: FROM, typeIds: [EDGE], targets: null}],
		}
		expect(await checkRelationsCount(deps({readCard: vi.fn(async () => rows)}), dead, body(3), "t")).toEqual({
			result: "unsupported",
			reason: "unmapped_space",
		})
		const broken = vi.fn(async () => {
			throw new GeoDbError("timeout", "slow")
		})
		expect(await checkRelationsCount(deps({readCard: broken}), CARD, body(3), "t")).toEqual({result: "failed"})
		const budget = {tryAcquire: () => false}
		expect(
			await checkRelationsCount(deps({readCard: vi.fn(async () => rows)}, {budget}), CARD, body(3), "t"),
		).toEqual({
			result: "unsupported",
			reason: "throttled",
		})
	})
})

const totals = (field: string, metric = "gaia_api_gateway_shadow_total") =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${metric}{field="${field}"`) && !l.endsWith(" 0"))
			.map((l) => [l.match(/(?:result|reason)="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
	)

describe("the relationsConnection shadow with GEODB_RELATIONS_COUNT_SHADOW", () => {
	beforeEach(() => __resetGatewayMetricsForTests())
	const query = `query { n: relationsConnection(filter: {typeId: {is: "${EDGE}"}, toEntityId: {in: ["${T1}", "${T2}"]}, spaceId: {is: "${SPACE}"}}) { totalCount } }`
	const runShadow = async (d: EntityShadowDeps, answer: unknown) => {
		const shadow = createPointReadShadows(d, ["relationsConnection"]).get("relationsConnection")
		const response = Response.json(answer)
		await shadow?.({
			classification: {operationName: null, operationType: "query", rootFields: ["relationsConnection"]},
			params: {query, operationName: null, variables: {}},
			document: parse(query),
			response,
		})
	}

	it("compares a totalCount-only target set when on", async () => {
		const read = perTarget({[T1]: {members: 1, scanned: 2}, [T2]: {members: 1, scanned: 1}})
		await runShadow(deps({readRelationCount: read}), body(3))
		expect(totals("relationsConnection")).toEqual({match: 1})
		expect(read).toHaveBeenCalledTimes(2)
	})

	it("counts a mismatch on a different row count", async () => {
		const read = perTarget({[T1]: {members: 1, scanned: 2}, [T2]: {members: 1, scanned: 1}})
		await runShadow(deps({readRelationCount: read}), body(7))
		expect(totals("relationsConnection")).toEqual({mismatch: 1})
	})

	it("leaves it unsupported_selection{count_only} and reads nothing when off", async () => {
		const read = vi.fn()
		await runShadow(deps({readRelationCount: read}, {relationsCount: false}), body(3))
		expect(totals("relationsConnection")).toEqual({unsupported_selection: 1})
		expect(totals("relationsConnection", "gaia_api_gateway_shadow_unsupported_total")).toEqual({count_only: 1})
		expect(read).not.toHaveBeenCalled()
	})
})

describe("GEODB_RELATIONS_COUNT_SHADOW", () => {
	it("is off by default and on only for `on`", () => {
		expect(readGeoDbCardConfig({}).relationsCount).toBe(false)
		expect(readGeoDbCardConfig({GEODB_RELATIONS_COUNT_SHADOW: "true"}).relationsCount).toBe(false)
		expect(readGeoDbCardConfig({GEODB_RELATIONS_COUNT_SHADOW: " ON "}).relationsCount).toBe(true)
	})

	it("reaches the point-read shadows' deps in main.ts", () => {
		const main = readFileSync(new URL("../../../main.ts", import.meta.url), "utf8")
		const start = main.indexOf("...createPointReadShadows(")
		expect(start).toBeGreaterThan(0)
		expect(main.slice(start, start + 1500)).toContain("relationsCount: geoDbCardConfig.relationsCount === true")
	})
})
