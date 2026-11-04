import {readFileSync} from "node:fs"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, rowsFromGeoDbOutput, type ValueRow, valueKey} from "../../geodb/card"
import {readGatewayConfig} from "../config"
import {
	analyzeEntityOperation,
	compareDerived,
	createEntityShadow,
	type EntityPlan,
	extractPostgresCard,
	type PostgresCard,
} from "../entityShadow"
import {createGateway} from "../gateway"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"

// Real frontend queries (`Entity`, `EntityPage`) and PostGraphile's answers, captured read-only from the testnet
// gaia API for one entity that lives in two spaces: A holds its Name, 8 values and its relations, B one value
// (a Name-less X link). Relation nodes are trimmed to five. The geo-db side is NOT captured: it is built below
// from the same rows in the card's output format, because geo-db holds no copy of this testnet entity.
type Fixture = {
	request: {query: string; operationName: string; variables: Record<string, string>}
	response: {data: {entity: Record<string, any>}}
}
const load = (name: string): Fixture =>
	JSON.parse(readFileSync(new URL(`./fixtures/${name}.json`, import.meta.url), "utf8"))
const ENTITY_A = load("entity-space-a")
const ENTITY_B = load("entity-space-b")
const PAGE_A = load("entity-page-space-a")

const ENTITY_HEX = "073ff16be7fe4591a984c325d629d92d"
const SPACE_A = "41e851610e13a19441c4d980f2f2ce6b"
const SPACE_B = "5093b6ab1b48f1111dd31275b5413ddb"
const GEO_SPACE_A = "aaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa"
const GEO_SPACE_B = "bbbbbbbbbbbb4bbb8bbbbbbbbbbbbbbb"
const NAME = "a126ca530c8e48d5b88882c734c38935"
const DESCRIPTION = "9b1f76ff9711404c861e59dc3fa7d037"
const TYPES = "8f151ba4de204e3c9cb499ddf96f48f1"
const PERSON = "7ed45f2bc48b419e8e4664d5ff680b0d"

const cell = (v: string) => v.replaceAll("\\", "\\\\").replaceAll("\t", "\\t").replaceAll("\n", "\\n")

/** A names cell as geo-db prints it: the name with `\t`, `\n`, `\` escaped, or `none`. */
const nameOf = (name: string | null | undefined) => (typeof name === "string" ? cell(name) : "none")

/**
 * geodb-client's output for the rows of one space of a fixture, as the card query would print them: the four
 * columns, then (`claims`) the two claim columns, then (`names`, the default) the property's and the target's
 * names, then the weight. Names are the fixture's own, so geo-db and PostGraphile agree on every one.
 */
function geoOutput(fixture: Fixture, options: {names?: boolean; claims?: boolean} = {}): string {
	const {names = true, claims = false} = options
	const tail = (propName: string | null | undefined, targetName: string | null | undefined) =>
		`${claims ? "\tnone\tnone" : ""}${names ? `\t${nameOf(propName)}\t${nameOf(targetName)}` : ""}\t1`
	const entity = fixture.response.data.entity
	const rows: string[] = []
	let n = 0
	for (const v of entity.valuesList) {
		const row: ValueRow = {
			property_id: v.property.id,
			text: v.text,
			integer: v.integer,
			float: v.float,
			decimal: v.decimal,
			boolean: v.boolean,
			datetime: v.datetime,
			date: v.date,
		}
		const key = valueKey(row)
		if (!key) throw new Error(`fixture value not mappable: ${v.property.id}`)
		rows.push(
			`${conformingId("entity", v.property.id)}\t${cell(key.norm)}\ts${n++}\t${ENTITY_HEX}${tail(v.property.name, null)}`,
		)
	}
	for (const r of entity.relations?.nodes ?? entity.relationsList) {
		const type = conformingId("entity", r.type.id)
		rows.push(
			`${type}\t${conformingId("entity", r.toEntity.id)}\ts${n++}\t${ENTITY_HEX}${tail(r.type.name, r.toEntity.name)}`,
		)
	}
	return `${rows.join("\n")}\n`
}

const plan = (fixture: Fixture): EntityPlan => {
	const result = analyzeEntityOperation(
		parse(fixture.request.query),
		fixture.request.operationName,
		fixture.request.variables,
	)
	if (!result.ok) throw new Error(`plan: ${result.reason}`)
	return result.plan
}
const card = (fixture: Fixture): NonNullable<PostgresCard> => {
	const result = extractPostgresCard(fixture.response, plan(fixture))
	if (!result) throw new Error("card")
	return result
}

function setup(output: string) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const readCard = vi.fn(async ({entityId, names}: {geoSpaceId: string; entityId: string; names?: boolean}) =>
		rowsFromGeoDbOutput(output, entityId, {names}),
	)
	const shadow = createEntityShadow({
		geoDb: {readCard},
		geoSpaceOf: async (space) => (space === SPACE_A ? GEO_SPACE_A : space === SPACE_B ? GEO_SPACE_B : null),
		log,
	})
	return {log, readCard, shadow}
}

const run = (
	shadow: ReturnType<typeof setup>["shadow"],
	fixture: Fixture,
	rootFields = ["entity"],
	body = fixture.response,
) =>
	shadow({
		classification: {operationName: fixture.request.operationName, operationType: "query", rootFields},
		params: {
			query: fixture.request.query,
			operationName: fixture.request.operationName,
			variables: fixture.request.variables,
		},
		document: parse(fixture.request.query),
		response: Response.json(body),
	})

const series = (prefix: string) =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${prefix}{`) && !l.endsWith(" 0"))
			.map((l) => [l.match(/(?:result|reason)="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
	)
const results = () => series("gaia_api_gateway_shadow_total")
const uncompared = () => series("gaia_api_gateway_shadow_uncompared_total")

const geoRows = (fixture: Fixture) => rowsFromGeoDbOutput(geoOutput(fixture), ENTITY_HEX, {names: true})
const withCard = (fixture: Fixture, patch: Partial<NonNullable<PostgresCard>>) => ({...card(fixture), ...patch})

beforeEach(() => __resetGatewayMetricsForTests())

describe("the real Entity and EntityPage queries", () => {
	it("are supported selections, with what a card cannot settle named", () => {
		const entity = plan(ENTITY_A)
		expect(entity).toMatchObject({
			entityId: ENTITY_HEX,
			spaceId: SPACE_A,
			name: "name",
			description: "description",
			spaceIds: "spaceIds",
		})
		expect(entity.relations.nodesKey).toBe("nodes")
		expect(entity.relations.hasNext).toEqual({pageInfo: "pageInfo", field: "hasNextPage"})
		expect(new Set(entity.uncompared)).toEqual(
			new Set(["updated_at", "nested_entity", "property_info", "membership_only"]),
		)
		const page = plan(PAGE_A)
		expect(page.relations.nodesKey).toBeNull()
		expect(new Set(page.uncompared)).toEqual(
			new Set(["nested_entity", "property_info", "membership_only", "root_relations"]),
		)
	})

	it("Entity, space A: match; updatedAt and property info are counted uncompared, the nested names are compared", async () => {
		const {shadow, readCard, log} = setup(geoOutput(ENTITY_A))
		await run(shadow, ENTITY_A)
		expect(results()).toEqual({match: 1})
		expect(log.warn).not.toHaveBeenCalled()
		expect(readCard).toHaveBeenCalledWith({geoSpaceId: GEO_SPACE_A, entityId: ENTITY_HEX, names: true})
		expect(uncompared()).toEqual({
			updated_at: 1,
			nested_entity: 1,
			property_info: 1,
			membership_only: 1,
			cross_space: 1,
		})
	})

	it("EntityPage, space A: match, and the co-root relations field is uncompared", async () => {
		const {shadow} = setup(geoOutput(PAGE_A))
		await run(shadow, PAGE_A, ["entity", "relations"])
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({root_relations: 1, cross_space: 1})
	})

	it("Entity, space B: a Name PostGraphile took from space A is not a mismatch (cross_space)", async () => {
		const {shadow, log} = setup(geoOutput(ENTITY_B))
		await run(shadow, ENTITY_B)
		expect(results()).toEqual({match: 1})
		expect(log.warn).not.toHaveBeenCalled()
		const checks = compareDerived(plan(ENTITY_B), card(ENTITY_B), geoRows(ENTITY_B))
		expect(checks.checks).toMatchObject({
			name: "inconclusive",
			description: "inconclusive",
			spaceIds: "ok",
			types: "inconclusive",
		})
		expect(checks.uncompared).toContain("cross_space")
	})

	it("counts a mismatch, with the derived field named, when geo-db lacks a statement", async () => {
		const missingName = geoOutput(ENTITY_A)
			.split("\n")
			.filter((line) => !line.startsWith(`${NAME}\t`))
			.join("\n")
		const {shadow, log} = setup(missingName)
		await run(shadow, ENTITY_A)
		expect(results()).toEqual({mismatch: 1})
		expect(log.warn).toHaveBeenCalledWith(
			"gateway entity shadow mismatch",
			expect.objectContaining({onlyPostgres: 1, derivedMismatches: ["allValuesList"]}),
		)
	})

	it("counts a full relations connection (hasNextPage) as unsupported: truncated", async () => {
		const body = structuredClone(ENTITY_A.response)
		body.data.entity.relations.pageInfo.hasNextPage = true
		const {shadow, readCard} = setup(geoOutput(ENTITY_A))
		await run(shadow, ENTITY_A, ["entity"], body)
		expect(results()).toEqual({unsupported_selection: 1})
		expect(renderGatewayMetrics()).toContain(
			'gaia_api_gateway_shadow_unsupported_total{field="entity",reason="truncated"} 1',
		)
		expect(readCard).not.toHaveBeenCalled()
	})
})

describe("derived field checks", () => {
	const base = () => ({plan: plan(ENTITY_A), pg: card(ENTITY_A), geo: geoRows(ENTITY_A)})
	/** A card for an entity that lives only in space A. */
	const single = (extra: Partial<NonNullable<PostgresCard>> = {}) => ({...base().pg, spaceIds: [SPACE_A], ...extra})

	it("name and description: ok when the card holds the value", () => {
		const {plan, pg, geo} = base()
		expect(compareDerived(plan, pg, geo).checks).toMatchObject({
			name: "ok",
			description: "ok",
			types: "ok",
			spaceIds: "ok",
			allValuesList: "ok",
		})
	})

	it("name: a value the card lacks is a mismatch for a single-space entity, inconclusive otherwise", () => {
		const {plan, geo} = base()
		expect(compareDerived(plan, single({name: "Someone Else"}), geo).checks.name).toBe("mismatch")
		const multi = compareDerived(plan, withCard(ENTITY_A, {name: "Someone Else"}), geo)
		expect(multi.checks.name).toBe("inconclusive")
		expect(multi.uncompared).toContain("cross_space")
	})

	it("name: null against a stored name is a mismatch; null against none is ok", () => {
		const {plan, geo} = base()
		expect(compareDerived(plan, withCard(ENTITY_A, {name: null}), geo).checks.name).toBe("mismatch")
		const noName = geo.filter((row) => row.prop !== NAME)
		expect(compareDerived(plan, withCard(ENTITY_A, {name: null}), noName).checks.name).toBe("ok")
	})

	it("description follows the same rule", () => {
		const {plan, geo} = base()
		expect(geo.some((row) => row.prop === DESCRIPTION)).toBe(true)
		expect(compareDerived(plan, single({description: "x"}), geo).checks.description).toBe("mismatch")
		expect(compareDerived(plan, withCard(ENTITY_A, {description: "x"}), geo).checks.description).toBe(
			"inconclusive",
		)
	})

	it("types: a Types target the card has but PostGraphile lacks is a mismatch", () => {
		const {plan, geo} = base()
		expect(geo.some((row) => row.prop === TYPES && row.raw === PERSON)).toBe(true)
		expect(compareDerived(plan, withCard(ENTITY_A, {types: []}), geo).checks.types).toBe("mismatch")
	})

	it("types: an extra PostGraphile type is another space's (inconclusive), or a mismatch for one space", () => {
		const {plan, geo} = base()
		const extra = [PERSON, "5e24fb52856c4189a9716af4387b1b89"]
		const multi = compareDerived(plan, withCard(ENTITY_A, {types: extra}), geo)
		expect(multi.checks.types).toBe("inconclusive")
		expect(multi.uncompared).toContain("cross_space")
		expect(compareDerived(plan, single({types: extra}), geo).checks.types).toBe("mismatch")
		// a duplicate (the SQL has no DISTINCT) is not a difference
		expect(compareDerived(plan, withCard(ENTITY_A, {types: [PERSON, PERSON]}), geo).checks.types).toBe("ok")
	})

	it("spaceIds: the card's space must be listed when it has statements; other spaces are uncompared", () => {
		const {plan, geo} = base()
		expect(compareDerived(plan, withCard(ENTITY_A, {spaceIds: [SPACE_B]}), geo).checks.spaceIds).toBe("mismatch")
		const ok = compareDerived(plan, withCard(ENTITY_A, {spaceIds: [SPACE_A, SPACE_B]}), geo)
		expect(ok.checks.spaceIds).toBe("ok")
		expect(ok.uncompared).toContain("cross_space")
		// a space with no statements may or may not be listed (derived rows live only in Postgres)
		expect(compareDerived(plan, withCard(ENTITY_A, {spaceIds: [SPACE_B]}), []).checks.spaceIds).toBe("ok")
	})

	it("spaceIds are read in either id form", () => {
		const body = structuredClone(ENTITY_A.response)
		body.data.entity.spaceIds = ["41e85161-0e13-a194-41c4-d980f2f2ce6b"]
		const dashed = extractPostgresCard(body, plan(ENTITY_A))
		if (!dashed) throw new Error("card")
		expect(compareDerived(plan(ENTITY_A), dashed, geoRows(ENTITY_A)).checks.spaceIds).toBe("ok")
	})

	it("allValuesList: a property in this space the card lacks is a mismatch; other spaces' rows are skipped", () => {
		const {plan, pg, geo} = base()
		const rows = [...(pg.allValues?.rows ?? []), {spaceId: SPACE_A, propertyId: "ffffffffffff4fffbfffffffffffffff"}]
		const bad = compareDerived(plan, {...pg, allValues: {rows, truncated: false}}, geo)
		expect(bad.checks.allValuesList).toBe("mismatch")
		const good = compareDerived(plan, pg, geo)
		expect(good.checks.allValuesList).toBe("ok")
		expect(good.uncompared).toContain("cross_space")
	})

	it("allValuesList: a derived property (vote Score) in PostGraphile only is not a mismatch", () => {
		const {plan, pg, geo} = base()
		const score = "85a4668a42fa4f488969c0a9de0c294b"
		const rows = [...(pg.allValues?.rows ?? []), {spaceId: SPACE_A, propertyId: score}]
		expect(compareDerived(plan, {...pg, allValues: {rows, truncated: false}}, geo).checks.allValuesList).toBe("ok")
	})

	it("a full unfiltered list is uncompared (truncated), not compared", () => {
		const {plan, pg, geo} = base()
		const result = compareDerived(plan, {...pg, allValues: {rows: [], truncated: true}}, geo)
		expect(result.checks.allValuesList).toBeUndefined()
		expect(result.uncompared).toContain("truncated")
	})

	it("an entity PostGraphile does not have has no name, types or spaces", () => {
		const empty = extractPostgresCard({data: {entity: null}}, plan(ENTITY_A))
		expect(empty).toMatchObject({name: null, description: null, spaceIds: [], types: [], truncated: false})
		if (!empty) throw new Error("card")
		expect(compareDerived(plan(ENTITY_A), empty, []).checks).toMatchObject({
			name: "ok",
			types: "ok",
			spaceIds: "ok",
		})
	})
})

describe("nested names", () => {
	const geoNamed = (fixture: Fixture) => geoRows(fixture)
	const nestedOf = (fixture: Fixture) => {
		const nested = card(fixture).nested
		if (!nested) throw new Error("no nested names")
		return nested
	}

	it("plans the five compared names and leaves only the relation's own entity name uncompared", () => {
		const entity = plan(ENTITY_A)
		expect(entity.values.names).toEqual({property: "property\tname"})
		expect(Object.keys(entity.relations.names).sort()).toEqual(["fromEntity", "toEntity", "type"])
		expect(entity.types?.nameKey).toBe("name")
		expect(entity.uncompared).not.toContain("nested_names")
		const own = parse(
			"query Q($id: UUID!, $s: UUID!) { entity(id: $id) { id valuesList(first: 5, filter: {spaceId: {is: $s}}) { propertyId text integer float boolean datetime date decimal } relationsList(first: 5, filter: {spaceId: {is: $s}}) { typeId toEntityId entity { name } } } }",
		)
		const result = analyzeEntityOperation(own, "Q", {id: ENTITY_A.request.variables.id, s: SPACE_A})
		if (!result.ok) throw new Error(result.reason)
		expect(result.plan.uncompared).toContain("nested_names")
	})

	it("reads each nested name out of PostGraphile's answer", () => {
		const nested = nestedOf(ENTITY_A)
		expect(nested.property?.length).toBe(8)
		expect(nested.property?.map((p) => p.name)).toContain("Citation count")
		expect(nested.toEntity?.map((t) => t.name)).toEqual([
			"Person",
			"Attention Is All You Need",
			"### Key Contribution",
			null,
			expect.any(String),
		])
		expect(nested.fromEntity).toEqual(Array(5).fill("Noam Shazeer"))
		expect(card(ENTITY_A).typeNames).toEqual(["Person"])
	})

	it("agrees, on every compared name, with the geo-db names built from the same fixture", () => {
		const {checks, uncompared} = compareDerived(plan(ENTITY_A), card(ENTITY_A), geoNamed(ENTITY_A))
		expect(checks).toMatchObject({
			propertyNames: "ok",
			typeNames: "ok",
			toEntityNames: "ok",
			typesNames: "ok",
			fromEntityNames: "ok",
		})
		expect(uncompared).not.toContain("nested_names")
	})

	it("fromEntity names are exact when the entity lives in this space alone", () => {
		const alone = {...card(ENTITY_A), spaceIds: [SPACE_A]}
		expect(compareDerived(plan(ENTITY_A), alone, geoNamed(ENTITY_A)).checks.fromEntityNames).toBe("ok")
		const wrong = {
			...alone,
			nested: {...alone.nested, fromEntity: ["Somebody Else", ...Array(4).fill("Noam Shazeer")]},
		}
		expect(compareDerived(plan(ENTITY_A), wrong, geoNamed(ENTITY_A)).checks.fromEntityNames).toBe("mismatch")
	})

	it("a name PostGraphile lacks that geo-db holds is a mismatch", () => {
		const base = card(ENTITY_A)
		const nested = nestedOf(ENTITY_A)
		const patched = withCard(ENTITY_A, {
			nested: {...nested, toEntity: (nested.toEntity ?? []).map((t, i) => (i === 1 ? {...t, name: null} : t))},
		})
		expect(base.nested).toBeDefined()
		expect(compareDerived(plan(ENTITY_A), patched, geoNamed(ENTITY_A)).checks.toEntityNames).toBe("mismatch")
		const types = withCard(ENTITY_A, {typeNames: [null]})
		expect(compareDerived(plan(ENTITY_A), types, geoNamed(ENTITY_A)).checks.typesNames).toBe("mismatch")
	})

	it("a different PostGraphile name, or a `none` from geo-db, is inconclusive (another space may have named it)", () => {
		const nested = nestedOf(ENTITY_A)
		const other = withCard(ENTITY_A, {
			nested: {
				...nested,
				property: (nested.property ?? []).map((p, i) => (i === 0 ? {...p, name: "Elsewhere"} : p)),
			},
		})
		const differing = compareDerived(plan(ENTITY_A), other, geoNamed(ENTITY_A))
		expect(differing.checks.propertyNames).toBe("inconclusive")
		expect(differing.uncompared).toContain("cross_space")
		// geo-db prints `none` for a target outside the Space: blank every target name on its side.
		const blank = geoNamed(ENTITY_A).map((row) => ({...row, targetName: null}))
		expect(compareDerived(plan(ENTITY_A), card(ENTITY_A), blank).checks.toEntityNames).toBe("inconclusive")
	})

	it("without names on the geo-db side the names are uncompared, not mismatched", () => {
		const bare = rowsFromGeoDbOutput(geoOutput(ENTITY_A, {names: false}), ENTITY_HEX)
		const {checks, uncompared} = compareDerived(plan(ENTITY_A), card(ENTITY_A), bare)
		expect(uncompared).toContain("nested_names")
		for (const key of ["propertyNames", "typeNames", "toEntityNames", "typesNames", "fromEntityNames"] as const)
			expect(checks[key]).toBeUndefined()
	})

	it("the shadow counts a name mismatch with its field named, and still matches when the names agree", async () => {
		const body = structuredClone(ENTITY_A.response)
		body.data.entity.relations.nodes[1].toEntity.name = null
		const {shadow, log} = setup(geoOutput(ENTITY_A))
		await run(shadow, ENTITY_A, ["entity"], body)
		expect(results()).toEqual({mismatch: 1})
		expect(log.warn).toHaveBeenCalledWith(
			"gateway entity shadow mismatch",
			expect.objectContaining({derivedMismatches: ["toEntityNames"]}),
		)
	})

	it("parses the same names whether or not the read also asked for claims", () => {
		const without = rowsFromGeoDbOutput(geoOutput(ENTITY_A), ENTITY_HEX, {names: true})
		const withClaims = rowsFromGeoDbOutput(geoOutput(ENTITY_A, {claims: true}), ENTITY_HEX, {
			names: true,
			claims: true,
		})
		expect(withClaims).toEqual(without)
		expect(without.some((row) => typeof row.propName === "string")).toBe(true)
		// A reader told the wrong layout reads the claim cells as names, which is why the option exists.
		expect(
			rowsFromGeoDbOutput(geoOutput(ENTITY_A, {claims: true}), ENTITY_HEX, {names: true})[0]?.propName,
		).toBeNull()
	})
})

describe("analysis of the wider selections", () => {
	const SPACE = "41e85161-0e13-a194-41c4-d980f2f2ce6b"
	const ID = "073ff16b-e7fe-4591-a984-c325d629d92d"
	const lists = `valuesList(first: 9, filter: {spaceId: {is: "${SPACE}"}}) { propertyId text integer float boolean datetime date decimal }
    relationsList(first: 9, filter: {spaceId: {is: "${SPACE}"}}) { typeId toEntityId }`
	const analyze = (query: string) => analyzeEntityOperation(parse(query), null, {})

	it("refuses a co-root field other than relations, or a repeated entity", () => {
		expect(analyze(`{ entity(id: "${ID}") { id ${lists} } spaces { id } }`)).toEqual({
			ok: false,
			reason: "not_single_root",
		})
		expect(analyze(`{ entity(id: "${ID}") { id ${lists} } b: entity(id: "${ID}") { id } }`)).toEqual({
			ok: false,
			reason: "not_single_root",
		})
	})

	it("refuses fields it does not know inside the nested objects", () => {
		const query = (inner: string) =>
			`{ entity(id: "${ID}") { ${lists.replace("{ typeId toEntityId }", `{ typeId toEntityId toEntity { id ${inner} } }`)} } }`
		expect(analyze(query("name")).ok).toBe(true)
		expect(analyze(query("secret"))).toEqual({ok: false, reason: "selection"})
	})

	it("refuses a second pageable relations list", () => {
		expect(
			analyze(
				`{ entity(id: "${ID}") { ${lists} relations(first: 1, filter: {spaceId: {is: "${SPACE}"}}) { nodes { typeId toEntityId } } } }`,
			).ok,
		).toBe(false)
	})
})

describe("gateway: co-root fields", () => {
	const config = readGatewayConfig({
		GRAPHQL_GATEWAY: "on",
		GRAPHQL_GATEWAY_FIELDS: "entity=shadow",
		GRAPHQL_GATEWAY_SHADOW_SAMPLE: "1",
	})
	const settle = () => new Promise((resolve) => setTimeout(resolve, 20))
	const call = async (query: string) => {
		const {shadow} = setup(geoOutput(PAGE_A))
		const spy = Object.assign(vi.fn(shadow), {tolerates: shadow.tolerates})
		const gateway = createGateway({
			config,
			delegate: () => Response.json(PAGE_A.response),
			shadows: new Map([["entity", spy]]),
		})
		await gateway(
			new Request("http://api/graphql", {
				method: "POST",
				body: JSON.stringify({query, variables: PAGE_A.request.variables}),
			}),
			{},
		)
		await settle()
		return spy
	}

	it("shadows the real EntityPage operation (entity plus relations)", async () => {
		const spy = await call(PAGE_A.request.query)
		expect(spy).toHaveBeenCalledTimes(1)
		expect(results()).toEqual({match: 1})
	})

	it("does not shadow entity beside a root field the shadow does not tolerate", async () => {
		const spy = await call(`query { entity(id: "x") { id } spaces { id } }`)
		expect(spy).not.toHaveBeenCalled()
	})
})
