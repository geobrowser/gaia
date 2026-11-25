import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, rowsFromGeoDbOutput} from "../../geodb/card"
import {createEntitiesConnectionShadow, ENTITIES_CONNECTION_FIELD} from "../entitiesConnectionShadow"
import {analyzeLightEntitiesConnection} from "../lightShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {analyzePointRead, createPointReadShadows, RELATIONS_CONNECTION_FIELD} from "../pointReadShadows"

const SPACE_A = "11111111-1111-4111-8111-111111111111"
const GEO_SPACE_A = "77777777777747778777777777777777"
const ENTITY_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
const TARGET_ID = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
const REL_TYPE = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
const NAME_PROP = "a126ca53-0c8e-48d5-b888-82c734c38935"
const DESC_PROP = "9b1f76ff-9711-404c-861e-59dc3fa7d037"

const hex = (uuid: string) => uuid.replaceAll("-", "")

function metricsFor(field: string): Record<string, number> {
	const out: Record<string, number> = {}
	for (const line of renderGatewayMetrics().split("\n")) {
		const m = line.match(
			new RegExp(`^gaia_api_gateway_shadow_total\\{field="${field}",result="([^"]+)"\\} (\\d+)$`),
		)
		if (m?.[1] && m[2] && Number(m[2]) > 0) out[m[1]] = Number(m[2])
	}
	return out
}

function unsupportedFor(field: string): Record<string, number> {
	const out: Record<string, number> = {}
	for (const line of renderGatewayMetrics().split("\n")) {
		const m = line.match(
			new RegExp(`^gaia_api_gateway_shadow_unsupported_total\\{field="${field}",reason="([^"]+)"\\} (\\d+)$`),
		)
		if (m?.[1] && m[2] && Number(m[2]) > 0) out[m[1]] = Number(m[2])
	}
	return out
}

function makeDeps(geoRowsByEntity: Record<string, string>) {
	const readCard = vi.fn(async ({entityId}: {entityId: string}) =>
		rowsFromGeoDbOutput(geoRowsByEntity[entityId] ?? "", entityId),
	)
	return {
		deps: {
			geoDb: {readCard},
			geoSpaceOf: async (space: string) => (space === hex(SPACE_A) ? GEO_SPACE_A : null),
			log: {info: vi.fn(), warn: vi.fn()},
		},
		readCard,
	}
}

describe("entitiesConnection shadow @include/@skip directives", () => {
	const entityGeoRows = [
		`${conformingId("entity", NAME_PROP)}\tAda\ts1\t${hex(ENTITY_ID)}\t1`,
		`${conformingId("entity", DESC_PROP)}\tPioneer\ts2\t${hex(ENTITY_ID)}\t1`,
	].join("\n")

	beforeEach(() => {
		__resetGatewayMetricsForTests()
	})

	async function runEntitiesShadow(
		query: string,
		variables: Record<string, unknown>,
		body: unknown,
		geo: Record<string, string> = {[hex(ENTITY_ID)]: `${entityGeoRows}\n`},
	) {
		const {deps, readCard} = makeDeps(geo)
		const shadow = createEntitiesConnectionShadow(deps)
		await shadow({
			params: {query, operationName: null, variables},
			document: parse(query),
			classification: {operationName: null, operationType: "query", rootFields: [ENTITIES_CONNECTION_FIELD]},
			response: Response.json(body),
		})
		return {readCard}
	}

	it("evaluates static @include and @skip on fields, inline fragments, and fragment spreads", async () => {
		const query = `
			fragment IncludedFrag on Entity {
				name
			}
			fragment ExcludedFrag on Entity {
				bogusFragmentField
			}
			query Q {
				entitiesConnection(first: 10, spaceId: "${SPACE_A}") {
					nodes {
						id
						spaceIds @skip(if: false)
						description @include(if: false)
						bogusSkipped @skip(if: true)
						...IncludedFrag @include(if: true)
						...ExcludedFrag @include(if: false)
						... on Entity @skip(if: false) {
							updatedAt
						}
						... on Entity @include(if: false) {
							unsupportedInlineField
						}
					}
					totalCount @include(if: true)
					ignoredField @skip(if: true)
				}
			}
		`
		const planned = analyzeLightEntitiesConnection(parse(query), null, {})
		expect(planned.ok).toBe(true)
		if (planned.ok) {
			expect(planned.plan.plans).toHaveLength(1)
			const card = planned.plan.plans[0]?.card
			expect(card?.idKey).toBe("id")
			expect(card?.name).toBe("name")
			expect(card?.spaceIds).toBe("spaceIds")
			expect(card?.description).toBeNull()
			expect(card?.updatedAt).toBe("updatedAt")
		}

		await runEntitiesShadow(
			query,
			{},
			{
				data: {
					entitiesConnection: {
						nodes: [{id: ENTITY_ID, name: "Ada", spaceIds: [SPACE_A], updatedAt: "100"}],
						totalCount: 1,
					},
				},
			},
		)
		expect(metricsFor(ENTITIES_CONNECTION_FIELD)).toEqual({match: 1})
	})

	it("evaluates variable-bound @include/@skip with explicit variables and GraphQL default values", async () => {
		const query = `
			fragment DescFrag on Entity {
				description
			}
			query Q($withDesc: Boolean = false, $skipName: Boolean = false) {
				entitiesConnection(first: 10, spaceId: "${SPACE_A}") {
					nodes {
						id
						spaceIds
						name @skip(if: $skipName)
						...DescFrag @include(if: $withDesc)
					}
				}
			}
		`
		// Defaults: withDesc = false, skipName = false -> name included, description excluded.
		const defaultPlan = analyzeLightEntitiesConnection(parse(query), null, {})
		expect(defaultPlan.ok).toBe(true)
		if (defaultPlan.ok) {
			expect(defaultPlan.plan.plans[0]?.card.name).toBe("name")
			expect(defaultPlan.plan.plans[0]?.card.description).toBeNull()
		}
		await runEntitiesShadow(
			query,
			{},
			{
				data: {entitiesConnection: {nodes: [{id: ENTITY_ID, name: "Ada", spaceIds: [SPACE_A]}]}},
			},
		)
		expect(metricsFor(ENTITIES_CONNECTION_FIELD)).toEqual({match: 1})

		// Explicit overrides: withDesc = true, skipName = true -> description included, name excluded.
		__resetGatewayMetricsForTests()
		const overridePlan = analyzeLightEntitiesConnection(parse(query), null, {withDesc: true, skipName: true})
		expect(overridePlan.ok).toBe(true)
		if (overridePlan.ok) {
			expect(overridePlan.plan.plans[0]?.card.name).toBeNull()
			expect(overridePlan.plan.plans[0]?.card.description).toBe("description")
		}
		await runEntitiesShadow(
			query,
			{withDesc: true, skipName: true},
			{data: {entitiesConnection: {nodes: [{id: ENTITY_ID, description: "Pioneer", spaceIds: [SPACE_A]}]}}},
		)
		expect(metricsFor(ENTITIES_CONNECTION_FIELD)).toEqual({match: 1})
	})

	it("handles combined @include and @skip on the same selection per GraphQL spec", () => {
		const query = `
			query Q($inc: Boolean!, $sk: Boolean!) {
				entitiesConnection(first: 10, spaceId: "${SPACE_A}") {
					nodes {
						id
						spaceIds
						name
						description @include(if: $inc) @skip(if: $sk)
					}
				}
			}
		`
		const bothInclude = analyzeLightEntitiesConnection(parse(query), null, {inc: true, sk: false})
		expect(bothInclude.ok).toBe(true)
		if (bothInclude.ok) expect(bothInclude.plan.plans[0]?.card.description).toBe("description")

		const skipped = analyzeLightEntitiesConnection(parse(query), null, {inc: true, sk: true})
		expect(skipped.ok).toBe(true)
		if (skipped.ok) expect(skipped.plan.plans[0]?.card.description).toBeNull()

		const notIncluded = analyzeLightEntitiesConnection(parse(query), null, {inc: false, sk: false})
		expect(notIncluded.ok).toBe(true)
		if (notIncluded.ok) expect(notIncluded.plan.plans[0]?.card.description).toBeNull()
	})

	it("drops excluded root selections in multi-root and single-root documents without mismatching", async () => {
		const multiQuery = `
			query Q($withSecond: Boolean!) {
				space(id: "${SPACE_A}") { id }
				primary: entitiesConnection(first: 10, spaceId: "${SPACE_A}") {
					nodes { id name spaceIds }
				}
				secondary: entitiesConnection(first: 10, spaceId: "${SPACE_A}") @include(if: $withSecond) {
					nodes { id name spaceIds }
				}
			}
		`
		await runEntitiesShadow(
			multiQuery,
			{withSecond: false},
			{data: {space: {id: SPACE_A}, primary: {nodes: [{id: ENTITY_ID, name: "Ada", spaceIds: [SPACE_A]}]}}},
		)
		expect(metricsFor(ENTITIES_CONNECTION_FIELD)).toEqual({match: 1})

		// When the only entitiesConnection root is excluded, drop it cleanly (no mismatch, no unsupported, no geo-db call).
		__resetGatewayMetricsForTests()
		const singleExcluded = `
			query Q($show: Boolean = false) {
				entitiesConnection(first: 10, spaceId: "${SPACE_A}") @include(if: $show) {
					nodes { id name spaceIds }
				}
			}
		`
		const {readCard} = await runEntitiesShadow(singleExcluded, {}, {data: {}})
		expect(metricsFor(ENTITIES_CONNECTION_FIELD)).toEqual({})
		expect(unsupportedFor(ENTITIES_CONNECTION_FIELD)).toEqual({})
		expect(readCard).not.toHaveBeenCalled()
	})

	it.each([
		[
			"unknown @custom directive",
			`query Q { entitiesConnection(first: 10, spaceId: "${SPACE_A}") { nodes { id name @custom spaceIds } } }`,
			{},
		],
		[
			"@defer directive on inline fragment",
			`query Q { entitiesConnection(first: 10, spaceId: "${SPACE_A}") { nodes { id spaceIds ... on Entity @defer { name } } } }`,
			{},
		],
		[
			"unknown directive paired with @include(if: false)",
			`query Q { entitiesConnection(first: 10, spaceId: "${SPACE_A}") { nodes { id name spaceIds description @include(if: false) @custom } } }`,
			{},
		],
		[
			"missing if argument on @include",
			`query Q { entitiesConnection(first: 10, spaceId: "${SPACE_A}") { nodes { id name @include spaceIds } } }`,
			{},
		],
		[
			"extra argument on @skip",
			`query Q { entitiesConnection(first: 10, spaceId: "${SPACE_A}") { nodes { id name @skip(if: false, extra: 1) spaceIds } } }`,
			{},
		],
		[
			"non-boolean literal if argument",
			`query Q { entitiesConnection(first: 10, spaceId: "${SPACE_A}") { nodes { id name @include(if: "true") spaceIds } } }`,
			{},
		],
		[
			"unresolved variable in @include",
			`query Q($flag: Boolean!) { entitiesConnection(first: 10, spaceId: "${SPACE_A}") { nodes { id name @include(if: $flag) spaceIds } } }`,
			{},
		],
		[
			"non-boolean variable value in @skip",
			`query Q($flag: Boolean!) { entitiesConnection(first: 10, spaceId: "${SPACE_A}") { nodes { id name @skip(if: $flag) spaceIds } } }`,
			{flag: "nope"},
		],
		[
			"null variable value in @include",
			`query Q($flag: Boolean) { entitiesConnection(first: 10, spaceId: "${SPACE_A}") { nodes { id name @include(if: $flag) spaceIds } } }`,
			{flag: null},
		],
		[
			"directive on operation definition",
			`query Q @custom { entitiesConnection(first: 10, spaceId: "${SPACE_A}") { nodes { id name spaceIds } } }`,
			{},
		],
	])("still returns honest unsupported{directives} for %s", (_, query, variables) => {
		expect(analyzeLightEntitiesConnection(parse(query), null, variables)).toEqual({
			ok: false,
			reason: "directives",
		})
	})
})

describe("relationsConnection shadow @include/@skip directives", () => {
	const relGeoRows = `${conformingId("entity", REL_TYPE)}\t${conformingId("entity", TARGET_ID)}\ts1\t${hex(ENTITY_ID)}\t1\n`

	beforeEach(() => {
		__resetGatewayMetricsForTests()
	})

	async function runRelationsShadow(
		query: string,
		variables: Record<string, unknown>,
		body: unknown,
		geo: Record<string, string> = {[hex(ENTITY_ID)]: relGeoRows},
	) {
		const {deps, readCard} = makeDeps(geo)
		const shadow = createPointReadShadows(deps, [RELATIONS_CONNECTION_FIELD]).get(RELATIONS_CONNECTION_FIELD)
		expect(shadow).toBeDefined()
		await shadow?.({
			params: {query, operationName: null, variables},
			document: parse(query),
			classification: {operationName: null, operationType: "query", rootFields: [RELATIONS_CONNECTION_FIELD]},
			response: Response.json(body),
		})
		return {readCard}
	}

	it("evaluates static @include and @skip on fields, inline fragments, and fragment spreads", async () => {
		const query = `
			fragment RelTarget on Relation {
				toEntityId
			}
			fragment ExcludedRel on Relation {
				bogusField
			}
			query Q {
				relationsConnection(filter: {fromEntityId: {is: "${ENTITY_ID}"}, typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE_A}"}}) {
					nodes {
						id @include(if: true)
						...RelTarget @skip(if: false)
						...ExcludedRel @include(if: false)
						... on Relation @include(if: true) {
							spaceId
						}
						... on Relation @skip(if: true) {
							unsupportedInline
						}
					}
					totalCount @include(if: false)
				}
			}
		`
		const planned = analyzePointRead(RELATIONS_CONNECTION_FIELD, parse(query), null, {})
		expect(planned.ok).toBe(true)

		await runRelationsShadow(
			query,
			{},
			{
				data: {
					relationsConnection: {
						nodes: [{id: "dddddddd-dddd-dddd-dddd-dddddddddddd", toEntityId: TARGET_ID, spaceId: SPACE_A}],
					},
				},
			},
		)
		expect(metricsFor(RELATIONS_CONNECTION_FIELD)).toEqual({match: 1})
	})

	it("evaluates variable-bound @include/@skip with explicit variables and GraphQL default values", async () => {
		const query = `
			query Q($withPos: Boolean = false, $skipTarget: Boolean = false) {
				relationsConnection(filter: {fromEntityId: {is: "${ENTITY_ID}"}, typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE_A}"}}) {
					nodes {
						toEntityId @skip(if: $skipTarget)
						position @include(if: $withPos)
						bogusField @include(if: $skipTarget)
					}
				}
			}
		`
		const defaultPlan = analyzePointRead(RELATIONS_CONNECTION_FIELD, parse(query), null, {})
		expect(defaultPlan.ok).toBe(true)

		await runRelationsShadow(
			query,
			{},
			{
				data: {relationsConnection: {nodes: [{toEntityId: TARGET_ID}]}},
			},
		)
		expect(metricsFor(RELATIONS_CONNECTION_FIELD)).toEqual({match: 1})

		// With explicit variables: withPos = true, skipTarget = false -> still supported and matches.
		__resetGatewayMetricsForTests()
		await runRelationsShadow(
			query,
			{withPos: true, skipTarget: false},
			{data: {relationsConnection: {nodes: [{toEntityId: TARGET_ID, position: "a"}]}}},
		)
		expect(metricsFor(RELATIONS_CONNECTION_FIELD)).toEqual({match: 1})
	})

	it("drops excluded relationsConnection root selections cleanly without mismatching", async () => {
		const multiQuery = `
			query Q($withExtra: Boolean!) {
				main: relationsConnection(filter: {fromEntityId: {is: "${ENTITY_ID}"}, typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE_A}"}}) {
					nodes { toEntityId }
				}
				extra: relationsConnection(filter: {fromEntityId: {is: "${ENTITY_ID}"}, typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE_A}"}}) @include(if: $withExtra) {
					nodes { toEntityId }
				}
			}
		`
		await runRelationsShadow(multiQuery, {withExtra: false}, {data: {main: {nodes: [{toEntityId: TARGET_ID}]}}})
		expect(metricsFor(RELATIONS_CONNECTION_FIELD)).toEqual({match: 1})

		// Excluded single root: dropped cleanly without mismatch, unsupported, or geo-db call.
		__resetGatewayMetricsForTests()
		const excludedOnly = `
			query Q {
				relationsConnection(filter: {fromEntityId: {is: "${ENTITY_ID}"}, typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE_A}"}}) @include(if: false) {
					nodes { toEntityId }
				}
			}
		`
		const {readCard} = await runRelationsShadow(excludedOnly, {}, {data: {}})
		expect(metricsFor(RELATIONS_CONNECTION_FIELD)).toEqual({})
		expect(unsupportedFor(RELATIONS_CONNECTION_FIELD)).toEqual({})
		expect(readCard).not.toHaveBeenCalled()
	})

	it.each([
		[
			"unknown @custom directive",
			`query Q { relationsConnection(filter: {fromEntityId: {is: "${ENTITY_ID}"}, typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE_A}"}}) { nodes { toEntityId @custom } } }`,
			{},
		],
		[
			"@defer directive",
			`query Q { relationsConnection(filter: {fromEntityId: {is: "${ENTITY_ID}"}, typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE_A}"}}) { nodes { ... on Relation @defer { toEntityId } } } }`,
			{},
		],
		[
			"missing if argument",
			`query Q { relationsConnection(filter: {fromEntityId: {is: "${ENTITY_ID}"}, typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE_A}"}}) { nodes { toEntityId @include } } }`,
			{},
		],
		[
			"non-boolean if argument",
			`query Q { relationsConnection(filter: {fromEntityId: {is: "${ENTITY_ID}"}, typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE_A}"}}) { nodes { toEntityId @skip(if: 1) } } }`,
			{},
		],
		[
			"unresolved variable",
			`query Q($flag: Boolean!) { relationsConnection(filter: {fromEntityId: {is: "${ENTITY_ID}"}, typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE_A}"}}) { nodes { toEntityId @include(if: $flag) } } }`,
			{},
		],
	])("still returns honest unsupported{directives} for %s", (_, query, variables) => {
		expect(analyzePointRead(RELATIONS_CONNECTION_FIELD, parse(query), null, variables)).toEqual({
			ok: false,
			reason: "directives",
		})
	})
})
