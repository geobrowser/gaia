import {readFileSync} from "node:fs"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {relationGroupsFromGeoDbOutput} from "../../geodb/card"
import {GeoDbError, RELATION_GROUPS_FIRST, relationGroupsDocument} from "../../geodb/client"
import {readGeoDbCardConfig} from "../../geodb/config"
import type {EntityShadowDeps} from "../entityShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {createPointReadShadows} from "../pointReadShadows"
import {analyzeRelationFacets, checkRelationFacets, type RelationFacetsPlan} from "../relationFacetsShadow"

const SPACE = "0123456789ab4bcd8ef0123456789abc"
const EDGE = "8f151ba4de204e3c9cb499ddf96f48f1"
const F1 = "aaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa"
const F2 = "bbbbbbbbbbbb4bbb8bbbbbbbbbbbbbbb"
const T1 = "11111111111141118111111111111111"
const T2 = "22222222222242228222222222222222"
const DASHED_T1 = "11111111-1111-4111-8111-111111111111"
const GEO_SPACE = `geo${SPACE}`.slice(0, 32)

const FACET = "groupedAggregates(groupBy: [TO_ENTITY_ID]) { keys distinctCount { fromEntityId } }"
const facetQuery = (filter: string, selection = FACET) =>
	`query { facets: relationsConnection(filter: ${filter}) { ${selection} } }`
const FILTER = `{typeId: {is: "${EDGE}"}, fromEntityId: {in: ["${F2}", "${F1}"]}, spaceId: {is: "${SPACE}"}}`
const analyze = (query: string, variables: Record<string, unknown> = {}) =>
	analyzeRelationFacets(parse(query), null, variables)

describe("the grouped relations document", () => {
	it('is the `relations` root with `.type`, ascending `.from`, `.group_by = "to"` and `.first`', () => {
		expect(relationGroupsDocument({typeId: EDGE, fromEntityIds: [F2, F1], first: 4096})).toBe(
			`.{ .version = 1, .select = .{ .g = .{ .field = "relations", .args = .{ .type = "${EDGE}", .from = .{ "${F1}", "${F2}" }, .group_by = "to", .first = 4096 } } } }`,
		)
	})

	it("reads rows of a target and its count, the tool's trailing 1 dropped", () => {
		expect(relationGroupsFromGeoDbOutput(`${T1}\t2\t1\n${T2}\t1\t1\n`)).toEqual([
			{target: T1, count: 2},
			{target: T2, count: 1},
		])
		expect(relationGroupsFromGeoDbOutput("")).toEqual([])
		expect(() => relationGroupsFromGeoDbOutput("nope\t2\t1\n")).toThrow()
	})
})

describe("analyzeRelationFacets", () => {
	it("lowers the grouped distinct-count facet over a type and a set of from entities", () => {
		expect(analyze(facetQuery(FILTER))).toEqual({
			ok: true,
			plan: {
				roots: [
					{
						key: "facets",
						aggregatesKey: "groupedAggregates",
						spaceIds: [SPACE],
						typeId: EDGE,
						fromEntityIds: [F2, F1],
					},
				],
			},
		})
	})

	it("takes no Space as every Space, an aliased aggregate and variables", () => {
		const a = analyze(
			`query ($r: UUID!, $f: [UUID!]) { facets: relationsConnection(filter: {typeId: {equalTo: $r}, fromEntityId: {in: $f}}) { by: groupedAggregates(groupBy: [TO_ENTITY_ID]) { keys distinctCount { fromEntityId } } } }`,
			{r: EDGE, f: [F1]},
		)
		expect(a).toEqual({
			ok: true,
			plan: {
				roots: [{key: "facets", aggregatesKey: "by", spaceIds: null, typeId: EDGE, fromEntityIds: [F1]}],
			},
		})
	})

	it("refuses what the form cannot state", () => {
		const refused = {ok: false, reason: "count_only"}
		// grouped by another column, or by two
		expect(analyze(facetQuery(FILTER, FACET.replace("TO_ENTITY_ID", "SPACE_ID")))).toEqual(refused)
		expect(analyze(facetQuery(FILTER, FACET.replace("[TO_ENTITY_ID]", "[TO_ENTITY_ID, SPACE_ID]")))).toEqual(
			refused,
		)
		// another aggregate, or none of the counted column
		expect(analyze(facetQuery(FILTER, FACET.replace("fromEntityId", "id")))).toEqual(refused)
		expect(analyze(facetQuery(FILTER, "groupedAggregates(groupBy: [TO_ENTITY_ID]) { keys }"))).toEqual(refused)
		// nodes beside it, a `having`
		expect(analyze(facetQuery(FILTER, `${FACET} nodes { id }`))).toEqual(refused)
		expect(analyze(facetQuery(FILTER, FACET.replace("TO_ENTITY_ID])", "TO_ENTITY_ID], having: {})")))).toEqual(
			refused,
		)
		// no Type, no from entities, a target, a nested object, no filter pinning
		expect(analyze(facetQuery(`{fromEntityId: {in: ["${F1}"]}}`))).toEqual(refused)
		expect(analyze(facetQuery(`{typeId: {is: "${EDGE}"}}`))).toEqual(refused)
		expect(
			analyze(facetQuery(`{typeId: {is: "${EDGE}"}, fromEntityId: {is: "${F1}"}, toEntityId: {is: "${T1}"}}`)),
		).toEqual(refused)
		expect(
			analyze(
				facetQuery(
					`{typeId: {is: "${EDGE}"}, fromEntityId: {is: "${F1}"}, fromEntity: {typeIds: {anyEqualTo: "${T1}"}}}`,
				),
			),
		).toEqual(refused)
	})

	it("is batch_size past the 32 from entities one read takes", () => {
		const many = Array.from({length: 33}, (_, i) => `"${i.toString(16).padStart(8, "0")}000040008000000000000000"`)
		expect(analyze(facetQuery(`{typeId: {is: "${EDGE}"}, fromEntityId: {in: [${many.join(", ")}]}}`))).toEqual({
			ok: false,
			reason: "batch_size",
		})
	})
})

const deps = (geoDb: Record<string, unknown>, extra: Partial<EntityShadowDeps> = {}) =>
	({
		relationGroupedFacets: true,
		multiSpace: true,
		geoDb,
		geoSpaceOf: async (space: string) => (space === "dead" ? null : `geo${space}`.slice(0, 32)),
		log: {info: vi.fn(), warn: vi.fn()},
		...extra,
	}) as unknown as EntityShadowDeps

const PLAN: RelationFacetsPlan = {
	roots: [
		{key: "facets", aggregatesKey: "groupedAggregates", spaceIds: [SPACE], typeId: EDGE, fromEntityIds: [F2, F1]},
	],
}
/** PostGraphile's answer: BigInt counts as strings, the key a dashed uuid. */
const answer = (rows: [string, string | number][]) => ({
	data: {
		facets: {
			groupedAggregates: rows.map(([key, n]) => ({keys: [key], distinctCount: {fromEntityId: n}})),
		},
	},
})
const groups = (rows: {target: string; count: number}[]) => vi.fn(async () => rows)

describe("checkRelationFacets", () => {
	it("asks the grouped form once and compares the count of each target", async () => {
		const read = groups([
			{target: T1, count: 2},
			{target: T2, count: 1},
		])
		const d = deps({readRelationGroups: read})
		expect(
			await checkRelationFacets(
				d,
				PLAN,
				answer([
					[T2, "1"],
					[DASHED_T1, "2"],
				]),
				"t",
			),
		).toMatchObject({
			result: "match",
			uncompared: [],
		})
		expect(read).toHaveBeenCalledTimes(1)
		expect(read).toHaveBeenCalledWith({
			geoSpaceIds: [GEO_SPACE],
			typeId: EDGE,
			fromEntityIds: [F2, F1],
			first: RELATION_GROUPS_FIRST,
		})
	})

	it("is a mismatch on a different count, a target only one side holds", async () => {
		const read = groups([
			{target: T1, count: 2},
			{target: T2, count: 1},
		])
		const d = deps({readRelationGroups: read})
		expect(
			await checkRelationFacets(
				d,
				PLAN,
				answer([
					[T1, "3"],
					[T2, "1"],
				]),
				"t",
			),
		).toMatchObject({
			result: "mismatch",
			detail: {relationFacets: true, differing: [{root: 0, targets: 1}]},
		})
		expect(await checkRelationFacets(d, PLAN, answer([[T1, "2"]]), "t")).toMatchObject({result: "mismatch"})
		expect(
			await checkRelationFacets(
				d,
				PLAN,
				answer([
					[T1, "2"],
					[T2, "1"],
					[F1, "1"],
				]),
				"t",
			),
		).toMatchObject({
			result: "mismatch",
		})
	})

	it("is count_refused on a serve without the form, the shadow's current path", async () => {
		const refused = vi.fn(async () => {
			throw new GeoDbError("client_failed", "error: MalformedRequest at 1:60")
		})
		expect(await checkRelationFacets(deps({readRelationGroups: refused}), PLAN, answer([[T1, "2"]]), "t")).toEqual({
			result: "unsupported",
			reason: "count_refused",
		})
		// a client with no such read at all
		expect(await checkRelationFacets(deps({}), PLAN, answer([[T1, "2"]]), "t")).toEqual({result: "skip"})
	})

	it("is failed on a read error, count_incomplete on a full page, skip on an answer it cannot read", async () => {
		const broken = vi.fn(async () => {
			throw new GeoDbError("timeout", "slow")
		})
		expect(await checkRelationFacets(deps({readRelationGroups: broken}), PLAN, answer([[T1, "2"]]), "t")).toEqual({
			result: "failed",
		})
		const full = groups(Array.from({length: RELATION_GROUPS_FIRST}, (_, i) => ({target: `${i}`, count: 1})))
		expect(await checkRelationFacets(deps({readRelationGroups: full}), PLAN, answer([[T1, "2"]]), "t")).toEqual({
			result: "unsupported",
			reason: "count_incomplete",
		})
		const read = groups([])
		expect(await checkRelationFacets(deps({readRelationGroups: read}), PLAN, {errors: [{}]}, "t")).toEqual({
			result: "skip",
		})
		expect(
			await checkRelationFacets(
				deps({readRelationGroups: read}),
				PLAN,
				{data: {facets: {groupedAggregates: [{keys: [T1]}]}}},
				"t",
			),
		).toEqual({
			result: "skip",
		})
		expect(read).not.toHaveBeenCalled()
	})

	it("is unmapped_space, no_space and throttled where the read cannot be made", async () => {
		const read = groups([])
		const dead: RelationFacetsPlan = {
			roots: [{...PLAN.roots[0], spaceIds: ["dead"]} as RelationFacetsPlan["roots"][0]],
		}
		expect(await checkRelationFacets(deps({readRelationGroups: read}), dead, answer([]), "t")).toEqual({
			result: "unsupported",
			reason: "unmapped_space",
		})
		const everywhere: RelationFacetsPlan = {
			roots: [{...PLAN.roots[0], spaceIds: null} as RelationFacetsPlan["roots"][0]],
		}
		expect(await checkRelationFacets(deps({readRelationGroups: read}), everywhere, answer([]), "t")).toEqual({
			result: "unsupported",
			reason: "no_space",
		})
		expect(
			await checkRelationFacets(
				deps({readRelationGroups: read}, {budget: {tryAcquire: () => false}} as never),
				PLAN,
				answer([]),
				"t",
			),
		).toEqual({result: "unsupported", reason: "throttled"})
		expect(read).not.toHaveBeenCalled()
	})
})

const totals = (field: string, metric = "gaia_api_gateway_shadow_total") =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${metric}{field="${field}"`) && !l.endsWith(" 0"))
			.map((l) => [l.match(/(?:result|reason)="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
	)

describe("the relationsConnection shadow with GEODB_RELATION_GROUPED_FACETS_SHADOW", () => {
	beforeEach(() => __resetGatewayMetricsForTests())
	const query = facetQuery(FILTER)
	const runShadow = async (
		d: EntityShadowDeps,
		body: unknown,
		q = query,
		field: "relationsConnection" | "relation" = "relationsConnection",
	) => {
		const shadow = createPointReadShadows(d, [field]).get(field)
		await shadow?.({
			classification: {operationName: null, operationType: "query", rootFields: [field]},
			params: {query: q, operationName: null, variables: {}},
			document: parse(q),
			response: Response.json(body),
		})
	}

	const REL_ID = "cccccccccccc4ccc8ccccccccccccccc"
	const cardClaimsQuery = `query { relation(id: "${REL_ID}") { id spaceId fromEntityId typeId toEntityId verified } }`
	const cardClaimsBody = {
		data: {
			relation: {
				id: REL_ID,
				spaceId: SPACE,
				fromEntityId: F1,
				typeId: EDGE,
				toEntityId: T1,
				verified: false,
			},
		},
	}

	it("compares the grouped facet when on, one geo-db read", async () => {
		const read = groups([{target: T1, count: 2}])
		await runShadow(deps({readRelationGroups: read}), answer([[T1, "2"]]))
		expect(totals("relationsConnection")).toEqual({match: 1})
		expect(read).toHaveBeenCalledTimes(1)
	})

	it("counts a mismatch on a different count", async () => {
		await runShadow(deps({readRelationGroups: groups([{target: T1, count: 5}])}), answer([[T1, "2"]]))
		expect(totals("relationsConnection")).toEqual({mismatch: 1})
	})

	it("falls back to unsupported_selection when the serve lacks the form", async () => {
		const refused = vi.fn(async () => {
			throw new GeoDbError("client_failed", "error: MalformedRequest at 1:60")
		})
		await runShadow(deps({readRelationGroups: refused}), answer([[T1, "2"]]))
		expect(totals("relationsConnection")).toEqual({unsupported_selection: 1})
		expect(totals("relationsConnection", "gaia_api_gateway_shadow_unsupported_total")).toEqual({count_refused: 1})
	})

	it("stays unsupported_selection{count_only} and reads nothing when off", async () => {
		const read = vi.fn()
		await runShadow(deps({readRelationGroups: read}, {relationGroupedFacets: false}), answer([[T1, "2"]]))
		expect(totals("relationsConnection")).toEqual({unsupported_selection: 1})
		expect(totals("relationsConnection", "gaia_api_gateway_shadow_unsupported_total")).toEqual({count_only: 1})
		expect(read).not.toHaveBeenCalled()
	})

	it("enables grouped facets without enabling card-claims compare when GEODB_RELATION_FACETS_SHADOW is off", async () => {
		const readGroups = groups([{target: T1, count: 2}])
		const readCard = vi.fn(async () => [{prop: EDGE, raw: T1}])
		const readClaims = vi.fn(async () => [])
		const d = deps(
			{readRelationGroups: readGroups, readCard, readRelationClaims: readClaims},
			{relationGroupedFacets: true, relationFacets: false},
		)
		await runShadow(d, answer([[T1, "2"]]))
		expect(totals("relationsConnection")).toEqual({match: 1})
		expect(readGroups).toHaveBeenCalledTimes(1)

		await runShadow(d, cardClaimsBody, cardClaimsQuery, "relation")
		expect(totals("relation")).toEqual({match: 1})
		expect(readClaims).not.toHaveBeenCalled()
	})

	it("keeps grouped facets off while enabling card-claims compare when only GEODB_RELATION_FACETS_SHADOW is on", async () => {
		const readGroups = vi.fn()
		const readCard = vi.fn(async () => [{prop: EDGE, raw: T1}])
		const readClaims = vi.fn(async () => [])
		const d = deps(
			{readRelationGroups: readGroups, readCard, readRelationClaims: readClaims},
			{relationGroupedFacets: false, relationFacets: true},
		)
		await runShadow(d, answer([[T1, "2"]]))
		expect(totals("relationsConnection")).toEqual({unsupported_selection: 1})
		expect(totals("relationsConnection", "gaia_api_gateway_shadow_unsupported_total")).toEqual({count_only: 1})
		expect(readGroups).not.toHaveBeenCalled()

		await runShadow(d, cardClaimsBody, cardClaimsQuery, "relation")
		expect(totals("relation")).toEqual({match: 1})
		expect(readClaims).toHaveBeenCalledTimes(1)
	})

	it("runs neither grouped facets nor card-claims compare when both flags are off", async () => {
		const readGroups = vi.fn()
		const readCard = vi.fn(async () => [{prop: EDGE, raw: T1}])
		const readClaims = vi.fn(async () => [])
		const d = deps(
			{readRelationGroups: readGroups, readCard, readRelationClaims: readClaims},
			{relationGroupedFacets: false, relationFacets: false},
		)
		await runShadow(d, answer([[T1, "2"]]))
		expect(totals("relationsConnection")).toEqual({unsupported_selection: 1})
		expect(totals("relationsConnection", "gaia_api_gateway_shadow_unsupported_total")).toEqual({count_only: 1})
		expect(readGroups).not.toHaveBeenCalled()

		await runShadow(d, cardClaimsBody, cardClaimsQuery, "relation")
		expect(totals("relation")).toEqual({match: 1})
		expect(readClaims).not.toHaveBeenCalled()
	})
})

describe("GEODB_RELATION_GROUPED_FACETS_SHADOW", () => {
	it("is off by default, on only for `on`, and independent of GEODB_RELATION_FACETS_SHADOW", () => {
		expect(readGeoDbCardConfig({}).relationGroupedFacets).toBe(false)
		expect(readGeoDbCardConfig({GEODB_RELATION_GROUPED_FACETS_SHADOW: "true"}).relationGroupedFacets).toBe(false)
		expect(readGeoDbCardConfig({GEODB_RELATION_GROUPED_FACETS_SHADOW: " ON "}).relationGroupedFacets).toBe(true)
		expect(readGeoDbCardConfig({GEODB_RELATION_GROUPED_FACETS_SHADOW: "on"}).relationFacets).toBe(false)
		expect(readGeoDbCardConfig({GEODB_RELATION_FACETS_SHADOW: "on"}).relationGroupedFacets).toBe(false)
	})

	it("reaches the point-read shadows' deps in main.ts", () => {
		const main = readFileSync(new URL("../../../main.ts", import.meta.url), "utf8")
		const start = main.indexOf("...createPointReadShadows(")
		expect(start).toBeGreaterThan(0)
		expect(main.slice(start, start + 1500)).toContain(
			"relationGroupedFacets: geoDbCardConfig.relationGroupedFacets === true",
		)
	})
})
