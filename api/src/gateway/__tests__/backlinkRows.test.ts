import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {backlinkRowsFromGeoDbOutput, conformingId, rowsFromGeoDbOutput} from "../../geodb/card"
import {readBacklinkFilter} from "../backlinksShadow"
import {createEntityShadow} from "../entityShadow"
import {analyzeLightEntities} from "../lightShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"

// `backlinks` / `backlinksList` rows of a light selection are compared with geo-db's card backlinks: the referrer in
// each Space, and the relation type when the selection names it.
const ENTITY = "aaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa"
const SPACE_A = "11111111111141118111111111111111"
const SPACE_B = "22222222222242228222222222222222"
const SPACE_C = "33333333333343338333333333333333"
const FROM_1 = "c1c1c1c1c1c14c1c8c1cc1c1c1c1c1c1"
const FROM_2 = "c2c2c2c2c2c24c2c8c2cc2c2c2c2c2c2"
const FROM_3 = "c3c3c3c3c3c34c3c8c3cc3c3c3c3c3c3"
const TYPE_1 = "e1e1e1e1e1e14e1e8e1ee1e1e1e1e1e1"
const TYPE_2 = "e2e2e2e2e2e24e2e8e2ee2e2e2e2e2e2"
const NAME = conformingId("entity", "a126ca53-0c8e-48d5-b888-82c734c38935")
const SCORE = conformingId("entity", "85a4668a-42fa-4f48-8969-c0a9de0c294b")
const geoSpace = (s: string) => s
const dashed = (hex: string) =>
	`${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`

const series = (prefix: string) =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${prefix}{`) && !l.endsWith(" 0"))
			.map((l) => [l.match(/(?:result|reason)="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
	)
const results = () => series("gaia_api_gateway_shadow_total")
const uncompared = () => series("gaia_api_gateway_shadow_uncompared_total")
const unsupported = () => series("gaia_api_gateway_shadow_unsupported_total")
beforeEach(() => __resetGatewayMetricsForTests())

/** A backlink row as geodb-client prints it: `from` holds `prop` with the entity as value. */
const ref = (space: string, from: string, prop: string) =>
	`${prop}\t${conformingId("entity", ENTITY)}\t${space}:s0\t${conformingId("entity", from)}\t1\n`

function setup(backlinks: string[][], options: {flag?: boolean; every?: string[]} = {}) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const card = `${NAME}\tAn entity\tsn\t${ENTITY}\t-\t-\t1\n`
	const readBacklinks = vi.fn(async ({geoSpaceIds}: {geoSpaceIds: string[]; entityId: string; limit: number}) => {
		const text = geoSpaceIds
			.flatMap((space) =>
				backlinks
					.filter(([s]) => s === space)
					.map(([s, from, prop]) => ref(s as string, from as string, prop as string)),
			)
			.join("")
		return backlinkRowsFromGeoDbOutput(text, ENTITY, {spaces: true})
	})
	const readCard = vi.fn(async ({entityId, names}: {entityId: string; names?: boolean}) =>
		rowsFromGeoDbOutput(card, entityId, {names}),
	)
	const readCardSpaces = vi.fn(async ({entityId, names}: {entityId: string; names?: boolean}) =>
		rowsFromGeoDbOutput(card.replace("\tsn\t", `\t${SPACE_A}:sn\t`), entityId, {names, spaces: true}),
	)
	const allGeoSpaces = vi.fn(async () => options.every ?? [SPACE_A, SPACE_B, SPACE_C])
	const shadow = createEntityShadow({
		multiSpace: true,
		backlinks: options.flag !== false,
		geoDb: {readCard, readCardSpaces, readBacklinks},
		geoSpaceOf: async (space: string) => geoSpace(space),
		allGeoSpaces,
		log,
	} as never)
	return {log, readBacklinks, readCard, shadow}
}

const QUERY = (list: string) =>
	`query ($id: UUID!, $spaceIds: [UUID!]!) { entity(id: $id) { id name spaceIds ${list} } }`
const run = (shadow: (c: never) => Promise<void>, list: string, body: unknown) => {
	const query = QUERY(list)
	return shadow({
		classification: {operationName: null, operationType: "query", rootFields: []},
		params: {
			query,
			operationName: null,
			variables: {id: dashed(ENTITY), spaceIds: [dashed(SPACE_A), dashed(SPACE_B)]},
		},
		document: parse(query),
		response: Response.json(body),
	} as never)
}
const answer = (rows: unknown[], key = "backlinksList") => ({
	data: {entity: {id: ENTITY, name: "An entity", spaceIds: [SPACE_A], [key]: rows}},
})
const row = (space: string, from: string, type?: string) => ({
	spaceId: space,
	...(type ? {typeId: type} : {}),
	fromEntity: {id: from},
})

const LIST = "backlinksList(filter: {spaceId: {in: $spaceIds}}) { spaceId typeId fromEntity { id } }"

describe("backlink rows of a light selection", () => {
	it("matches rows that geo-db holds, by Space, relation type and referrer, with no backlink_rows left", async () => {
		const s = setup([
			[SPACE_A, FROM_1, TYPE_1],
			[SPACE_B, FROM_2, TYPE_2],
		])
		await run(s.shadow as never, LIST, answer([row(SPACE_A, FROM_1, TYPE_1), row(SPACE_B, FROM_2, TYPE_2)]))
		expect(results()).toEqual({match: 1})
		expect(uncompared().backlink_rows).toBeUndefined()
		expect(s.readBacklinks).toHaveBeenCalledTimes(1)
		expect(s.readBacklinks.mock.calls[0]?.[0].geoSpaceIds).toEqual([SPACE_A, SPACE_B])
	})

	it("is a mismatch when PostGraphile lists a referrer geo-db lacks", async () => {
		const s = setup([[SPACE_A, FROM_1, TYPE_1]])
		await run(s.shadow as never, LIST, answer([row(SPACE_A, FROM_1, TYPE_1), row(SPACE_A, FROM_2, TYPE_1)]))
		expect(results()).toEqual({mismatch: 1})
	})

	it("is a mismatch when geo-db holds a row PostGraphile's whole page lacks", async () => {
		const s = setup([
			[SPACE_A, FROM_1, TYPE_1],
			[SPACE_A, FROM_3, TYPE_1],
		])
		await run(s.shadow as never, LIST, answer([row(SPACE_A, FROM_1, TYPE_1)]))
		expect(results()).toEqual({mismatch: 1})
	})

	it("tells a relation type, a Space and a referrer apart", async () => {
		const held = [[SPACE_A, FROM_1, TYPE_1]]
		for (const wrong of [
			row(SPACE_A, FROM_1, TYPE_2),
			row(SPACE_B, FROM_1, TYPE_1),
			row(SPACE_A, FROM_2, TYPE_1),
		]) {
			__resetGatewayMetricsForTests()
			await run(setup(held).shadow as never, LIST, answer([wrong]))
			expect(results()).toEqual({mismatch: 1})
		}
	})

	it("compares by Space and referrer alone when the selection names no type", async () => {
		const s = setup([[SPACE_A, FROM_1, TYPE_1]])
		const list = "backlinksList(filter: {spaceId: {in: $spaceIds}}) { spaceId fromEntity { id } }"
		await run(s.shadow as never, list, answer([row(SPACE_A, FROM_1)]))
		expect(results()).toEqual({match: 1})
	})

	it("applies a typeId filter to geo-db's rows, so rows of other types are not extras", async () => {
		const s = setup([
			[SPACE_A, FROM_1, TYPE_1],
			[SPACE_A, FROM_2, TYPE_2],
		])
		const list = `backlinksList(filter: {typeId: {is: "${dashed(TYPE_1)}"}, spaceId: {in: $spaceIds}}) { spaceId fromEntity { id } }`
		await run(s.shadow as never, list, answer([row(SPACE_A, FROM_1)]))
		expect(results()).toEqual({match: 1})
	})

	it("reads a connection's nodes and `fromEntityId`", async () => {
		const s = setup([[SPACE_A, FROM_1, TYPE_1]])
		const list =
			"backlinks(filter: {spaceId: {in: $spaceIds}}) { totalCount nodes { spaceId typeId fromEntityId } }"
		const body = {
			data: {
				entity: {
					id: ENTITY,
					name: "An entity",
					spaceIds: [SPACE_A],
					backlinks: {totalCount: 1, nodes: [{spaceId: SPACE_A, typeId: TYPE_1, fromEntityId: FROM_1}]},
				},
			},
		}
		await run(s.shadow as never, list, body)
		expect(results()).toEqual({match: 1})
		expect(uncompared().backlink_rows).toBeUndefined()
	})

	it("does not judge a full page against geo-db's extras, nor a row whose referrer is missing", async () => {
		const s = setup([
			[SPACE_A, FROM_1, TYPE_1],
			[SPACE_A, FROM_3, TYPE_1],
		])
		const list = "backlinksList(first: 1, filter: {spaceId: {in: $spaceIds}}) { spaceId typeId fromEntity { id } }"
		await run(s.shadow as never, list, answer([row(SPACE_A, FROM_1, TYPE_1)]))
		expect(results()).toEqual({match: 1})
		expect(uncompared().truncated).toBe(1)
	})

	it("skips derived relation types and membership rows on both sides", async () => {
		const s = setup([
			[SPACE_A, FROM_1, TYPE_1],
			[SPACE_A, FROM_2, SCORE],
		])
		await run(s.shadow as never, LIST, answer([row(SPACE_A, FROM_1, TYPE_1), row(SPACE_A, FROM_3, dashed(SCORE))]))
		expect(results()).toEqual({match: 1})
	})

	it("holds PostGraphile's rows to geo-db's even when the filter says more than geo-db can apply", async () => {
		const s = setup([
			[SPACE_A, FROM_1, TYPE_1],
			[SPACE_A, FROM_3, TYPE_1],
		])
		const list =
			"backlinksList(filter: {position: {isNull: false}, spaceId: {in: $spaceIds}}) { spaceId fromEntity { id } }"
		await run(s.shadow as never, list, answer([row(SPACE_A, FROM_1)]))
		expect(results()).toEqual({match: 1})
		expect(uncompared().filter_completeness).toBe(1)
		__resetGatewayMetricsForTests()
		await run(setup([]).shadow as never, list, answer([row(SPACE_A, FROM_1)]))
		expect(results()).toEqual({mismatch: 1})
	})

	it("leaves the rows uncompared with the flag off, or when the selection names no referrer", async () => {
		await run(setup([], {flag: false}).shadow as never, LIST, answer([row(SPACE_A, FROM_1, TYPE_1)]))
		expect(results()).toEqual({match: 1})
		expect(uncompared().backlink_rows).toBe(1)
		__resetGatewayMetricsForTests()
		const s = setup([])
		await run(s.shadow as never, "backlinksList { spaceId typeId }", answer([{spaceId: SPACE_A, typeId: TYPE_1}]))
		expect(results()).toEqual({match: 1})
		expect(uncompared().backlink_rows).toBe(1)
		expect(s.readBacklinks).not.toHaveBeenCalled()
	})

	it("reads every Space when the filter names none", async () => {
		const s = setup([[SPACE_C, FROM_1, TYPE_1]])
		await run(
			s.shadow as never,
			"backlinksList { spaceId typeId fromEntity { id } }",
			answer([row(SPACE_C, FROM_1, TYPE_1)]),
		)
		expect(results()).toEqual({match: 1})
		expect(s.readBacklinks.mock.calls[0]?.[0].geoSpaceIds).toEqual([SPACE_A, SPACE_B, SPACE_C])
	})

	it("counts an unmapped Space's question as uncompared rows, not a failure", async () => {
		const log = {info: vi.fn(), warn: vi.fn()}
		const shadow = createEntityShadow({
			multiSpace: true,
			backlinks: true,
			geoDb: {
				readCard: vi.fn(async ({entityId}: {entityId: string}) =>
					rowsFromGeoDbOutput(`${NAME}\tAn entity\tsn\t${ENTITY}\t-\t-\t1\n`, entityId, {names: true}),
				),
				readBacklinks: vi.fn(async () => []),
			},
			geoSpaceOf: async (space: string) => (space === SPACE_B ? null : space),
			log,
		} as never)
		await run(shadow as never, LIST, answer([row(SPACE_A, FROM_1, TYPE_1)]))
		expect(results()).toEqual({match: 1})
		expect(uncompared().backlink_rows).toBe(1)
	})
})

describe("readBacklinkFilter", () => {
	it("reads Space and type clauses, and marks anything else inexact", () => {
		expect(readBacklinkFilter(undefined)).toEqual({spaces: null, types: null, exact: true})
		expect(
			readBacklinkFilter({spaceId: {is: dashed(SPACE_A)}, typeId: {in: [dashed(TYPE_1), dashed(TYPE_2)]}}),
		).toEqual({
			spaces: [SPACE_A],
			types: [TYPE_1, TYPE_2],
			exact: true,
		})
		expect(readBacklinkFilter({spaceId: {is: null}})).toEqual({spaces: null, types: null, exact: true})
		expect(readBacklinkFilter({spaceId: {in: []}}).exact).toBe(false)
		expect(readBacklinkFilter({spaceId: {isNot: dashed(SPACE_A)}}).exact).toBe(false)
		expect(readBacklinkFilter({position: {isNull: true}}).exact).toBe(false)
	})
})

describe("planning", () => {
	it("plans an entities selection's rows for comparison, leaving no backlink_rows", () => {
		const plan = analyzeLightEntities(
			parse(
				`query { entities(filter: {id: {in: ["${dashed(ENTITY)}"]}}) { id name spaceIds backlinks { nodes { typeId fromEntity { id description } } } } }`,
			),
			null,
			{},
			{},
		)
		expect(plan.ok && plan.plan.card.backlinkRows).toHaveLength(1)
		expect(plan.ok && plan.plan.uncompared).not.toContain("backlink_rows")
		expect(plan.ok && plan.plan.uncompared).toContain("nested_entity")
	})
})
void unsupported
