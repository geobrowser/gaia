import {readFileSync} from "node:fs"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, DERIVED_PROPERTIES, rowsFromGeoDbOutput} from "../../geodb/card"
import {analyzeLightEntities} from "../lightShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {createPointReadShadows} from "../pointReadShadows"

// geogenesis's EntitiesBatch: `entities(filter: {id: {in}}, spaceId)` with the cross-Space projections
// (`allValuesList { spaceId propertyId }`, `allRelationsList { spaceId }`), `relations { totalCount }`, the Space's
// own values and relations (with their fragments), and `types`. PostGraphile's answer is built here from the
// same rows the geo-db side is built from.
const FIXTURE: {request: {query: string; variables: Record<string, unknown>}} = JSON.parse(
	readFileSync(new URL("./fixtures/entities-batch.json", import.meta.url), "utf8"),
)
const QUERY = FIXTURE.request.query
const VARIABLES = FIXTURE.request.variables
const SPACE_A = "a19c345ab9866679b001d7d2138d88a1"
const SPACE_B = "5093b6ab1b48f1111dd31275b5413ddb"
const ENTITY = "0123456789ab4bcd8ef0123456789abc"
const TARGET = "16d5e35f207544de85ac5fd287b04f44"
const RELATED = "6e3503fab974460ea3dbab8af9a41427"
const TYPE = "e550fe517e904b2c8fffdf13408f5634"
const RANK_TYPE = "890deffb-3843-49fa-8269-74000e3dcef6"
const geoSpace = (s: string) => `${s.slice(0, 12)}4${s.slice(13, 16)}8${s.slice(17)}`
const NAME = conformingId("entity", "a126ca53-0c8e-48d5-b888-82c734c38935")
const TYPES = conformingId("entity", "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1")
const SUMMARY = conformingId("entity", "c5e0f9a4-6b1d-4a39-8d2e-0f4a7b3c9d12")

const property = (id: string) => ({
	id,
	name: null,
	dataTypeId: null,
	dataTypeName: null,
	renderableTypeId: null,
	renderableTypeName: null,
	format: null,
	isType: false,
})
const valueNode = (spaceId: string, propertyId: string, text: string) => ({
	spaceId,
	property: property(propertyId),
	text,
	integer: null,
	float: null,
	point: null,
	boolean: null,
	time: null,
	language: null,
	unit: null,
	datetime: null,
	date: null,
	decimal: null,
	schedule: null,
})
const relationNode = (spaceId: string, typeId: string, toId: string, at: number) => ({
	id: `0000000000004000800000000000000${at}`,
	spaceId,
	position: null,
	verified: null,
	entityId: `1111111111114000800000000000000${at}`,
	fromEntity: {id: ENTITY, name: "Story"},
	toEntity: {id: toId, name: "Target", types: [], valuesList: []},
	toSpaceId: null,
	type: {id: typeId, name: null},
})

type Row = {space: string; kind: "value" | "relation"; prop: string; raw: string}
const rowsA: Row[] = [
	{space: SPACE_A, kind: "value", prop: "a126ca53-0c8e-48d5-b888-82c734c38935", raw: "Story"},
	{space: SPACE_A, kind: "value", prop: "c5e0f9a4-6b1d-4a39-8d2e-0f4a7b3c9d12", raw: "A summary"},
	{space: SPACE_A, kind: "relation", prop: "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1", raw: TYPE},
	{space: SPACE_A, kind: "relation", prop: RELATED, raw: TARGET},
]

/** PostGraphile's entity for `EntitiesBatch`, with the rows of the requested Space in its lists. */
function answer(options: {extraRelations?: ReturnType<typeof relationNode>[]; name?: string} = {}) {
	const own = rowsA.filter((r) => r.kind === "value")
	const relations = [
		relationNode(SPACE_A, "8f151ba4de204e3c9cb499ddf96f48f1", TYPE, 1),
		relationNode(SPACE_A, RELATED, TARGET, 2),
		...(options.extraRelations ?? []),
	]
	return {
		data: {
			entities: [
				{
					id: ENTITY,
					name: options.name ?? "Story",
					description: null,
					spaceIds: [SPACE_A, SPACE_B],
					createdAt: "2026-10-01T00:00:00Z",
					updatedAt: "2026-10-02T00:00:00Z",
					allValuesList: [
						...own.map((r) => ({spaceId: r.space, propertyId: r.prop.replaceAll("-", "")})),
						{spaceId: SPACE_B, propertyId: "7ed45f2bc48b419e8e4664d5ff680b0d"},
					],
					allRelationsList: [...relations.map((r) => ({spaceId: r.spaceId})), {spaceId: SPACE_B}],
					relations: {totalCount: relations.length + 1},
					types: [{id: TYPE, name: "News story"}],
					valuesList: own.map((r) => valueNode(r.space, r.prop.replaceAll("-", ""), r.raw)),
					relationsList: relations,
				},
			],
		},
	}
}

const cell = (v: string) => v.replaceAll("\\", "\\\\").replaceAll("\t", "\\t").replaceAll("\n", "\\n")
const geoLines = (rows: Row[], hex = ENTITY) =>
	`${rows
		.map((r, n) =>
			r.kind === "value"
				? `${conformingId("entity", r.prop)}\t${cell(r.raw)}\ts${n}\t${hex}\t-\t-\t1`
				: `${conformingId("entity", r.prop)}\t${conformingId("entity", r.raw)}\ts${n}\t${hex}\t-\t-\t1`,
		)
		.join("\n")}\n`

const askOf = (outputs: Record<string, string>) =>
	vi.fn(async ({geoSpaceId, entityId, names}: {geoSpaceId: string; entityId: string; names?: boolean}) =>
		rowsFromGeoDbOutput(outputs[geoSpaceId] ?? "", entityId, {names}),
	)

function setup(outputs: Record<string, string>, entitiesBatch = true) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const readCard = askOf(outputs)
	const deps = {
		multiSpace: false,
		geoDb: {readCard, readCardSpaces: vi.fn()},
		geoSpaceOf: async (space: string) => geoSpace(space),
		log,
		entitiesBatch,
	}
	return {log, readCard, points: createPointReadShadows(deps)}
}
const run = (s: ReturnType<typeof setup>, body: unknown, variables: Record<string, unknown> = VARIABLES) =>
	(s.points.get("entities") as never as (c: never) => Promise<void>)({
		classification: {operationName: "EntitiesBatch", operationType: "query", rootFields: []},
		params: {query: QUERY, operationName: "EntitiesBatch", variables},
		document: parse(QUERY),
		response: Response.json(body),
	} as never)

const series = (prefix: string) =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${prefix}{`) && !l.endsWith(" 0"))
			.map((l) => [l.match(/(?:result|reason)="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
	)
const results = () => series("gaia_api_gateway_shadow_total")
const unsupported = () => series("gaia_api_gateway_shadow_unsupported_total")
const uncompared = () => series("gaia_api_gateway_shadow_uncompared_total")

beforeEach(() => __resetGatewayMetricsForTests())

const BOTH = {[geoSpace(SPACE_A)]: geoLines(rowsA), [geoSpace(SPACE_B)]: ""}

describe("the EntitiesBatch selection", () => {
	it("is refused as a selection with the switch off, and planned with it on", () => {
		const off = analyzeLightEntities(parse(QUERY), "EntitiesBatch", VARIABLES)
		expect(off).toEqual({ok: false, reason: "selection"})
		const on = analyzeLightEntities(parse(QUERY), "EntitiesBatch", VARIABLES, {entitiesBatch: true})
		expect(on.ok).toBe(true)
		if (on.ok) {
			expect(on.plan.spaceIds).toEqual([SPACE_A])
			expect(on.plan.ids).toHaveLength(1)
			expect(on.plan.lists.map((l) => [l.kind, l.projection === true])).toEqual([
				["values", false],
				["relations", true],
				["values", false],
				["relations", false],
			])
			expect(on.plan.uncompared).toEqual(
				expect.arrayContaining([
					"membership_only",
					"partial_values",
					"relation_count",
					"nested_entity",
					"property_info",
					"filter_completeness",
					"sampled_rows",
				]),
			)
		}
	})

	it("plans the query with no Space too (every Space of the entity, from spaceIds)", () => {
		const {spaceId: _omitted, ...rest} = VARIABLES
		const plan = analyzeLightEntities(parse(QUERY), "EntitiesBatch", rest, {entitiesBatch: true})
		expect(plan.ok).toBe(true)
		if (plan.ok) expect(plan.plan.spaceIds).toBeNull()
	})

	it("counts unsupported_selection and reads nothing with the switch off", async () => {
		const s = setup(BOTH, false)
		await run(s, answer())
		expect(results()).toEqual({unsupported_selection: 1})
		expect(unsupported()).toEqual({selection: 1})
		expect(s.readCard).not.toHaveBeenCalled()
	})

	it("matches when geo-db holds every row of the answer, in the entity's Spaces", async () => {
		const s = setup(BOTH)
		await run(s, answer())
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({membership_only: 1, partial_values: 1, sampled_rows: 1})
		expect(s.readCard).toHaveBeenCalledTimes(2)
		expect(s.log.warn).not.toHaveBeenCalled()
	})

	it("collapses a duplicate relation: geo-db holds one edge for the two PostGraphile lists", async () => {
		const s = setup(BOTH)
		await run(s, answer({extraRelations: [relationNode(SPACE_A, RELATED, TARGET, 3)]}))
		expect(results()).toEqual({match: 1})
	})

	it("does not count a rank relation derived in Postgres as a replica gap", async () => {
		const rank = relationNode(SPACE_A, RANK_TYPE.replaceAll("-", ""), TARGET, 4)
		expect(DERIVED_PROPERTIES.has(RANK_TYPE.replaceAll("-", ""))).toBe(true)
		const s = setup(BOTH)
		await run(s, answer({extraRelations: [rank]}))
		expect(results()).toEqual({match: 1})
	})

	it("flags a relation PostGraphile lists that geo-db lacks", async () => {
		const s = setup(BOTH)
		await run(s, answer({extraRelations: [relationNode(SPACE_A, RELATED, "0f0f0f0f0f0f4f0f8f0f0f0f0f0f0f0f", 5)]}))
		expect(results()).toEqual({mismatch: 1})
		expect(s.log.warn).toHaveBeenCalledWith(
			"gateway entities shadow mismatch (light)",
			expect.objectContaining({first: expect.objectContaining({missingInGeoDb: 1})}),
		)
	})

	it("flags a name geo-db holds in none of the entity's Spaces", async () => {
		const s = setup(BOTH)
		await run(s, answer({name: "Other"}))
		expect(results()).toEqual({mismatch: 1})
	})

	it("flags an id the filter did not ask for", async () => {
		const stray = answer()
		const first = stray.data.entities[0]
		if (!first) throw new Error("no entity")
		stray.data.entities.push({...first, id: "fedcba9876544bcd8ef0fedcba987654"})
		const s = setup({...BOTH})
		await run(s, stray)
		expect(results()).toEqual({mismatch: 1})
	})

	it("counts a failed geo-db read as failed (an every-Space refusal included)", async () => {
		const s = setup(BOTH)
		s.readCard.mockRejectedValue(new Error("refused: ResourceExhausted"))
		await run(s, answer())
		expect(results()).toEqual({failed: 1})
	})

	it("skips an empty answer", async () => {
		const s = setup(BOTH)
		await run(s, {data: {entities: []}})
		expect(results()).toEqual({})
	})

	it("still refuses a relations list that selects no edge and is filtered or a connection", () => {
		const filtered = QUERY.replace(
			"allRelationsList: relationsList(first: 1000) {",
			"allRelationsList: relationsList(first: 1000, filter: {spaceId: {is: $spaceId}}) {",
		)
		expect(analyzeLightEntities(parse(filtered), "EntitiesBatch", VARIABLES, {entitiesBatch: true})).toEqual({
			ok: false,
			reason: "selection",
		})
	})
})
