import {type FieldNode, type OperationDefinitionNode, parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow} from "../../geodb/card"
import type {EntityShadowDeps} from "../entityShadow"
import {
	checkRelationsMembership,
	collectPropertyInfo,
	judgePropertyInfo,
	type PropertyInfoPg,
	type PropertyInfos,
	planPropertyInfo,
} from "../propertyInfo"

const PROP = "e1c3ad9f-5a3a-4a1c-8c0b-d1b45b4d6a10"
const PROP_GEO = conformingId("entity", PROP)
const DATA_TYPE_RELATION = conformingId("entity", "6d29d578-49bb-4959-baf7-2cc696b1671a")
const RENDERABLE_RELATION = conformingId("entity", "2316bbe1-c76f-4635-83f2-3e03b4f1fe46")
const FORMAT = conformingId("entity", "396f8c72-dfd0-4b57-91ea-09c1b9321b2f")
const IS_TYPE = conformingId("entity", "d2c1a101-14e3-464a-8272-f4e75b0f1407")
const TEXT_TYPE = "9edb6fcc-e454-4aa5-8611-39d7f024c010"
const URL_TYPE = "283127c9-6142-4a6e-9a1a-4f9b5b9a7d11"

const row = (prop: string, raw: string, targetName?: string): GeoCardRow => ({
	prop,
	raw,
	space: "s1",
	entity: PROP_GEO,
	...(targetName === undefined ? {} : {targetName}),
})

const agreeing = (): GeoCardRow[] => [
	row(DATA_TYPE_RELATION, conformingId("entity", TEXT_TYPE), "Text"),
	row(RENDERABLE_RELATION, conformingId("entity", URL_TYPE), "Url"),
	row(FORMAT, "dd/mm"),
	row(IS_TYPE, "true"),
]

const pg = (over: Partial<PropertyInfoPg> = {}): PropertyInfoPg => ({
	id: PROP,
	dataTypeId: TEXT_TYPE,
	dataTypeName: "Text",
	renderableTypeId: URL_TYPE,
	renderableTypeName: "Url",
	format: "dd/mm",
	isType: true,
	...over,
})

const judge = (items: PropertyInfoPg[], geo: GeoCardRow[], complete = true) =>
	judgePropertyInfo(new Map([[PROP, items]]), new Map(geo.length ? [[PROP, geo]] : []), complete)

describe("judgePropertyInfo", () => {
	it("matches and settles when geo-db agrees on every field", () => {
		const v = judge([pg()], agreeing())
		expect(v).toMatchObject({read: true, mismatches: {}, settled: true})
	})

	it("does not settle when the plan was incomplete", () => {
		expect(judge([pg()], agreeing(), false)).toMatchObject({settled: false})
	})

	it.each([
		["dataTypeId", {dataTypeId: URL_TYPE}],
		["dataTypeName", {dataTypeName: "Number"}],
		["renderableTypeId", {renderableTypeId: TEXT_TYPE}],
		["renderableTypeName", {renderableTypeName: "Image"}],
		["format", {format: "mm/dd"}],
		["isType", {isType: false}],
	])("flags a differing %s", (field, over) => {
		const v = judge([pg(over as Partial<PropertyInfoPg>)], agreeing())
		expect(v).toMatchObject({read: true, mismatches: {[field]: 1}})
	})

	it("flags a null PostgreSQL field when geo-db holds one", () => {
		const v = judge([pg({renderableTypeId: null, renderableTypeName: null})], agreeing())
		expect(v).toMatchObject({mismatches: {renderableTypeId: 1, renderableTypeName: 1}})
	})

	it("matches a null field when geo-db holds none", () => {
		const geo = agreeing().filter((r) => r.prop !== RENDERABLE_RELATION)
		const v = judge([pg({renderableTypeId: null, renderableTypeName: null})], geo)
		expect(v).toMatchObject({mismatches: {}})
	})

	it("leaves a name unjudged when geo-db prints none", () => {
		const geo = [row(DATA_TYPE_RELATION, conformingId("entity", TEXT_TYPE))]
		const v = judge(
			[pg({renderableTypeId: undefined, renderableTypeName: undefined, format: undefined, isType: undefined})],
			geo,
		)
		expect(v).toMatchObject({mismatches: {}, uncompared: ["cross_space"]})
	})

	it("reports a Property geo-db holds nothing of as uncompared, not mismatched", () => {
		expect(judge([pg()], [])).toMatchObject({mismatches: {}, uncompared: ["no_statements"]})
	})
})

const plansOf = (query: string) => {
	const op = parse(query).definitions[0] as OperationDefinitionNode
	return op.selectionSet.selections[0] as FieldNode
}

describe("planPropertyInfo / collectPropertyInfo", () => {
	it("plans the compared fields and collects one item per row", () => {
		const field = plansOf("{ property { id dataTypeId t: dataTypeName format isType } }")
		const plan = planPropertyInfo(field, new Map(), undefined)
		expect(plan).toEqual({
			key: "property",
			fields: {dataTypeId: "dataTypeId", dataTypeName: "t", format: "format", isType: "isType"},
			residual: false,
		})
		const into: PropertyInfos = {items: [], incomplete: false}
		collectPropertyInfo(
			{property: {dataTypeId: TEXT_TYPE, t: "Text", format: null, isType: false}},
			PROP,
			plan!,
			into,
		)
		expect(into).toEqual({
			items: [{id: PROP, dataTypeId: TEXT_TYPE, dataTypeName: "Text", format: null, isType: false}],
			incomplete: false,
		})
	})

	it("marks a field with arguments residual", () => {
		const field = plansOf("{ property { dataTypeId(x: 1) } }")
		expect(planPropertyInfo(field, new Map(), undefined)?.residual).toBe(true)
	})

	it("returns nothing when no compared field is selected", () => {
		expect(planPropertyInfo(plansOf("{ property { id } }"), new Map(), undefined)).toBeUndefined()
	})

	it("marks the card incomplete on a malformed value or an unresolved object", () => {
		const plan = planPropertyInfo(plansOf("{ property { isType } }"), new Map(), undefined)!
		const a: PropertyInfos = {items: [], incomplete: false}
		collectPropertyInfo({property: {isType: "yes"}}, PROP, plan, a)
		expect(a.incomplete).toBe(true)
		const b: PropertyInfos = {items: [], incomplete: false}
		collectPropertyInfo({property: undefined}, PROP, plan, b)
		expect(b.incomplete).toBe(true)
		const c: PropertyInfos = {items: [], incomplete: false}
		collectPropertyInfo({property: null}, PROP, plan, c)
		expect(c).toEqual({items: [], incomplete: false})
	})
})

const S1 = "11111111111141118111111111111111"
const S2 = "22222222222242228222222222222222"
const ENTITY = "073ff16b-e7fe-4591-a984-c325d629d92d"
const rel = (space: string): GeoCardRow => ({prop: "aa", raw: "bb", space, entity: conformingId("entity", ENTITY)})

const membershipDeps = (rows: GeoCardRow[] | Error) => {
	const readCardSpaces = vi.fn(async () => {
		if (rows instanceof Error) throw rows
		return rows
	})
	const deps = {
		geoDb: {readCardSpaces},
		log: {warn: vi.fn()},
		budget: undefined,
	} as unknown as EntityShadowDeps
	return {deps, readCardSpaces}
}
const identity = async (s: string) => s

describe("checkRelationsMembership", () => {
	it("matches when every Space lists at least geo-db's relations", async () => {
		const {deps, readCardSpaces} = membershipDeps([rel(S1), rel(S1), rel(S2)])
		const v = await checkRelationsMembership(
			deps,
			ENTITY,
			{spaces: [S1, S1, S1, S2], cut: false},
			[S1, S2],
			false,
			identity,
			"t",
		)
		expect(v).toMatchObject({read: true, mismatch: false})
		expect(readCardSpaces).toHaveBeenCalledWith(expect.objectContaining({relations: true, geoSpaceIds: [S1, S2]}))
	})

	it("flags a Space where geo-db holds more relations than PostgreSQL lists", async () => {
		const {deps} = membershipDeps([rel(S1), rel(S1)])
		const v = await checkRelationsMembership(deps, ENTITY, {spaces: [S1], cut: false}, [S1], false, identity, "t")
		expect(v).toMatchObject({read: true, mismatch: true, detail: {excessSpaces: [S1]}})
	})

	it("does not read a cut list", async () => {
		const {deps, readCardSpaces} = membershipDeps([])
		const v = await checkRelationsMembership(deps, ENTITY, {spaces: [S1], cut: true}, [S1], false, identity, "t")
		expect(v).toEqual({read: false})
		expect(readCardSpaces).not.toHaveBeenCalled()
	})

	it("matches without a read when geo-db holds the entity nowhere", async () => {
		const {deps, readCardSpaces} = membershipDeps([])
		const v = await checkRelationsMembership(deps, ENTITY, {spaces: [], cut: false}, [], false, identity, "t")
		expect(v).toMatchObject({read: true, mismatch: false})
		expect(readCardSpaces).not.toHaveBeenCalled()
	})

	it("is unread when the read fails", async () => {
		const {deps} = membershipDeps(new Error("boom"))
		const v = await checkRelationsMembership(deps, ENTITY, {spaces: [S1], cut: false}, [S1], false, identity, "t")
		expect(v).toEqual({read: false})
	})
})
