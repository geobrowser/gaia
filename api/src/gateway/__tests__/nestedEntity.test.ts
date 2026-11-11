import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow, rowsFromGeoDbOutput} from "../../geodb/card"
import {
	createEntityShadow,
	DESCRIPTION_PROPERTY,
	NAME_PROPERTY,
	NESTED_ENTITY_FIELDS,
	TYPES_PROPERTY,
} from "../entityShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"

// One entity with one relation to a nested entity (`toEntity`), the shape of the real Entity page.
const ROOT = "073ff16b-e7fe-4591-a984-c325d629d92d"
const ROOT_HEX = ROOT.replaceAll("-", "")
const NESTED = "7ed45f2b-c48b-419e-8e46-64d5ff680b0d"
const NESTED_GEO = conformingId("entity", NESTED)
const SPACE = "11111111-1111-4111-8111-111111111111"
const GEO_SPACE = "22222222222242228222222222222222"
const OTHER_SPACE = "33333333-3333-4333-8333-333333333333"
const OTHER_GEO_SPACE = "44444444444442228444444444444444"
const REL_TYPE = "6b6b6b6b-6b6b-4b6b-8b6b-6b6b6b6b6b6b"
const TYPE_A = "5ab2b8a5-a1a1-4e5d-9a6e-8fb2d4f0c9a1"
const PROP = "e1c3ad9f-5a3a-4a1c-8c0b-d1b45b4d6a10"
const REL_TARGET = "9c9c9c9c-9c9c-4c9c-8c9c-9c9c9c9c9c9c"

const nestedSelection: Record<string, string> = {
	description: "description",
	types: "types { id }",
	typeIds: "typeIds",
	spaceIds: "spaceIds",
	valuesList: "valuesList(first: 50) { propertyId text }",
	relationsList: "relationsList(first: 50) { typeId toEntityId }",
	values: "values(first: 50) { nodes { propertyId text } }",
	relations: "relations(first: 50) { nodes { typeId toEntityId } }",
	backlinks: "backlinks(first: 10) { nodes { fromEntityId } }",
	backlinksList: "backlinksList(first: 10) { fromEntityId }",
}

// What PostGraphile answers for each member, and how geo-db's card of the nested entity must differ for a mismatch.
const nestedAnswer: Record<string, unknown> = {
	description: "A person",
	types: [{id: TYPE_A}],
	typeIds: [TYPE_A],
	spaceIds: [SPACE],
	valuesList: [{propertyId: PROP, text: "Ada"}],
	relationsList: [{typeId: REL_TYPE, toEntityId: REL_TARGET}],
	values: {nodes: [{propertyId: PROP, text: "Ada"}]},
	relations: {nodes: [{typeId: REL_TYPE, toEntityId: REL_TARGET}]},
	backlinks: {nodes: []},
	backlinksList: [],
}

const hex = (id: string) => conformingId("entity", id)
const row = (prop: string, raw: string, space = GEO_SPACE): GeoCardRow => ({
	prop,
	raw,
	space,
	entity: NESTED_GEO,
})

/** geo-db's card of the nested entity, agreeing with `nestedAnswer`. */
const agreeing = (): GeoCardRow[] => [
	row(DESCRIPTION_PROPERTY, "A person"),
	row(TYPES_PROPERTY, hex(TYPE_A)),
	row(PROP.replaceAll("-", ""), "Ada"),
	row(hex(REL_TYPE), hex(REL_TARGET)),
]

const ROOT_GEO = [
	`${NAME_PROPERTY}\tRoot\ts1\t${ROOT_HEX}\t1`,
	`${hex(REL_TYPE)}\t${NESTED_GEO}\ts2\t${ROOT_HEX}\t1`,
	"",
].join("\n")

const queryFor = (members: string[]) => `query Card($id: UUID!, $spaceId: UUID) {
  entity(id: $id) {
    id
    valuesList(first: 1000, filter: {spaceId: {is: $spaceId}}) { spaceId propertyId text integer float boolean datetime date decimal }
    relationsList(first: 1000, filter: {spaceId: {is: $spaceId}}) {
      typeId toEntityId toSpaceId position
      toEntity { id ${members.map((m) => nestedSelection[m]).join(" ")} }
    }
  }
}`

const answerFor = (members: string[], over: Record<string, unknown> = {}) => ({
	data: {
		entity: {
			id: ROOT,
			valuesList: [{spaceId: SPACE, propertyId: NAME_PROPERTY, text: "Root"}],
			relationsList: [
				{
					typeId: REL_TYPE,
					toEntityId: NESTED,
					toSpaceId: SPACE,
					position: "a",
					toEntity: {
						id: NESTED,
						...Object.fromEntries(members.map((m) => [m, nestedAnswer[m]])),
						...over,
					},
				},
			],
		},
	},
})

const spaceMap = (space: string) =>
	space === SPACE.replaceAll("-", "") ? GEO_SPACE : space === OTHER_SPACE.replaceAll("-", "") ? OTHER_GEO_SPACE : null

function setup(
	options: {nestedEntity?: boolean; cards?: GeoCardRow[] | Error; spaces?: string[]; budget?: boolean} = {},
) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const readCard = vi.fn(async ({entityId}: {geoSpaceId: string; entityId: string}) =>
		rowsFromGeoDbOutput(ROOT_GEO, entityId),
	)
	const readCards = vi.fn(async (_args: {geoSpaceIds: string[]; entityIds: string[]}) => {
		if (options.cards instanceof Error) throw options.cards
		return options.cards ?? agreeing()
	})
	const tryAcquire = vi.fn(() => options.budget !== false)
	const shadow = createEntityShadow({
		geoDb: {readCard, readCards},
		geoSpaceOf: async (space) => spaceMap(space),
		log,
		nestedEntity: options.nestedEntity ?? true,
		allGeoSpaces: async () => options.spaces ?? [GEO_SPACE],
		everySpaceBudget: {tryAcquire} as never,
	})
	return {log, readCard, readCards, shadow, tryAcquire}
}

const run = (shadow: ReturnType<typeof setup>["shadow"], members: string[], body: unknown) => {
	const query = queryFor(members)
	return shadow({
		classification: {operationName: "Card", operationType: "query", rootFields: ["entity"]},
		params: {query, operationName: "Card", variables: {id: ROOT, spaceId: SPACE}},
		document: parse(query),
		response: Response.json(body),
	})
}

const series = (prefix: string) =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${prefix}{`) && !l.endsWith(" 0"))
			.map((l) => [l.match(/(?:result|reason)="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
	)
const results = () => series("gaia_api_gateway_shadow_total")
const uncompared = () => series("gaia_api_gateway_shadow_uncompared_total")

beforeEach(() => __resetGatewayMetricsForTests())

const COMPARED = Object.keys(NESTED_ENTITY_FIELDS).filter((m) => !m.startsWith("backlinks") && m !== "__typename")

describe("nested entity fields", () => {
	it("covers every member of NESTED_ENTITY_FIELDS", () => {
		expect(Object.keys(NESTED_ENTITY_FIELDS).sort()).toEqual(Object.keys(nestedSelection).sort())
	})

	describe.each(COMPARED)("%s", (member) => {
		it("flag off: uncompared as before, no nested read", async () => {
			const w = setup({nestedEntity: false})
			await run(w.shadow, [member], answerFor([member]))
			expect(w.readCards).not.toHaveBeenCalled()
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({nested_entity: 1})
		})

		it("flag on, geo-db agrees: match, nested_entity no longer uncompared, one batched read", async () => {
			const w = setup()
			await run(w.shadow, [member], answerFor([member]))
			expect(results()).toEqual({match: 1})
			expect(uncompared().nested_entity).toBeUndefined()
			expect(w.readCards).toHaveBeenCalledTimes(1)
			expect(w.readCards.mock.calls[0]?.[0]).toMatchObject({geoSpaceIds: [GEO_SPACE], entityIds: [NESTED_GEO]})
			expect(w.log.warn).not.toHaveBeenCalled()
		})

		it("flag on, geo-db disagrees: mismatch", async () => {
			const disagree: Record<string, GeoCardRow[]> = {
				description: [row(DESCRIPTION_PROPERTY, "Someone else")],
				types: [row(TYPES_PROPERTY, hex(REL_TARGET))],
				typeIds: [row(TYPES_PROPERTY, hex(REL_TARGET))],
				// geo-db holds a statement in a Space PostGraphile does not list
				spaceIds: [...agreeing(), row(PROP.replaceAll("-", ""), "x", OTHER_GEO_SPACE)],
				valuesList: [row(PROP.replaceAll("-", ""), "Grace")],
				relationsList: [row(hex(REL_TYPE), hex(TYPE_A))],
				values: [row(PROP.replaceAll("-", ""), "Grace")],
				relations: [row(hex(REL_TYPE), hex(TYPE_A))],
			}
			const w = setup({cards: disagree[member], spaces: [GEO_SPACE, OTHER_GEO_SPACE]})
			// a nested entity geo-db holds nothing of is not a mismatch; these cards hold something
			await run(w.shadow, [member], answerFor([member]))
			expect(results()).toEqual({mismatch: 1})
			expect(w.log.warn).toHaveBeenCalledWith(
				"gateway entity shadow mismatch",
				expect.objectContaining({nestedMismatches: expect.any(Object)}),
			)
		})
	})

	describe.each(["backlinks", "backlinksList"])("%s", (member) => {
		it("stays uncompared as nested_entity even with the flag on, and the other fields are still judged", async () => {
			const w = setup()
			await run(w.shadow, ["description", member], answerFor(["description", member]))
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({nested_entity: 1})
			__resetGatewayMetricsForTests()
			const bad = setup({cards: [row(DESCRIPTION_PROPERTY, "Someone else")]})
			await run(bad.shadow, ["description", member], answerFor(["description", member]))
			expect(results()).toEqual({mismatch: 1})
		})
	})
})

describe("nested entity reads", () => {
	it("a nested entity geo-db holds nothing of is no_statements, not a mismatch", async () => {
		const w = setup({cards: []})
		await run(w.shadow, ["description"], answerFor(["description"]))
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({no_statements: 1})
	})

	it("a failed read keeps nested_entity uncompared and does not fail the shadow", async () => {
		const w = setup({cards: new Error("boom")})
		await run(w.shadow, ["description"], answerFor(["description"]))
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({nested_entity: 1})
		expect(w.log.warn).toHaveBeenCalledWith("gateway entity shadow nested entity read failed", expect.anything())
	})

	it("an empty every-Space budget reads nothing", async () => {
		const w = setup({budget: false})
		await run(w.shadow, ["description"], answerFor(["description"]))
		expect(w.readCards).not.toHaveBeenCalled()
		expect(uncompared()).toMatchObject({nested_entity: 1})
	})

	it("more than 32 distinct nested entities are not read", async () => {
		const many = Array.from({length: 33}, (_, i) => {
			const id = `${(i + 16).toString(16).padStart(8, "0")}-aaaa-4aaa-8aaa-aaaaaaaaaaaa`
			return {
				typeId: REL_TYPE,
				toEntityId: id,
				toSpaceId: SPACE,
				position: "a",
				toEntity: {id, description: "d"},
			}
		})
		const body = answerFor(["description"]) as {data: {entity: {relationsList: unknown[]}}}
		body.data.entity.relationsList = many
		const w = setup()
		await run(w.shadow, ["description"], body)
		expect(w.readCards).not.toHaveBeenCalled()
		expect(uncompared()).toMatchObject({nested_entity: 1})
	})

	it("a space-filtered nested list is compared within that Space only", async () => {
		const query = (space: string) => `query Card($id: UUID!, $spaceId: UUID) {
		  entity(id: $id) {
		    id
		    valuesList(first: 1000, filter: {spaceId: {is: $spaceId}}) { spaceId propertyId text integer float boolean datetime date decimal }
		    relationsList(first: 1000, filter: {spaceId: {is: $spaceId}}) {
		      typeId toEntityId toSpaceId position
		      toEntity { id valuesList(first: 5, filter: {spaceId: {is: "${space}"}}) { propertyId text } }
		    }
		  }
		}`
		// geo-db holds the value in the first Space only; PostGraphile listed it under the second
		const cards = [row(PROP.replaceAll("-", ""), "Ada", GEO_SPACE)]
		const w = setup({cards, spaces: [GEO_SPACE, OTHER_GEO_SPACE]})
		const q = query(OTHER_SPACE)
		await w.shadow({
			classification: {operationName: "Card", operationType: "query", rootFields: ["entity"]},
			params: {query: q, operationName: "Card", variables: {id: ROOT, spaceId: SPACE}},
			document: parse(q),
			response: Response.json(answerFor(["valuesList"])),
		})
		expect(results()).toEqual({mismatch: 1})
	})
})
