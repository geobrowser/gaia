import {readFileSync} from "node:fs"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, rowsFromGeoDbOutput, valueKey} from "../../geodb/card"
import {createEntityShadow, isEmptyFilter} from "../entityShadow"
import {analyzeLightEntities, analyzeLightEntity} from "../lightShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {createPointReadShadows} from "../pointReadShadows"
import {createReadBudget} from "../readBudget"

// The news app's entity query (the biggest share of gateway traffic), and PostGraphile's answer captured
// read-only from the testnet gaia API (lists trimmed). The geo-db side is built below from the same rows, in
// geodb-client's card output format, because geo-db holds no copy of this testnet entity.
type Fixture = {request: {query: string; variables: Record<string, unknown>}; response: {data: {entity: Entity}}}
type Entity = {
	id: string
	name: string
	description: string
	spaceIds: string[]
	values: {nodes: {property: {id: string; name: string}; text: string | null; datetime: string | null}[]}
	lightRels: {nodes: Rel[]}
	heavyRels: {nodes: Rel[]}
}
type Rel = {spaceId: string; type: {id: string; name: string}; toEntity: {id: string; name: string | null}}

const FIXTURE: Fixture = JSON.parse(readFileSync(new URL("./fixtures/entity-light-news.json", import.meta.url), "utf8"))
const ENTITY = FIXTURE.response.data.entity
const HEX = ENTITY.id
const SPACE_A = ENTITY.spaceIds[0] as string
const SPACE_B = "5093b6ab1b48f1111dd31275b5413ddb"
const SPACE_C = "41e851610e13a19441c4d980f2f2ce6b"
const SPACE_D = "0123456789ab4bcd8ef0123456789abc"
const geoSpace = (s: string) => `${s.slice(0, 12)}4${s.slice(13, 16)}8${s.slice(17)}`
const NAME = conformingId("entity", "a126ca53-0c8e-48d5-b888-82c734c38935")
const DESCRIPTION = conformingId("entity", "9b1f76ff-9711-404c-861e-59dc3fa7d037")
const TYPES = conformingId("entity", "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1")

const cell = (v: string) => v.replaceAll("\\", "\\\\").replaceAll("\t", "\\t").replaceAll("\n", "\\n")
const nameOf = (n: string | null | undefined) => (typeof n === "string" ? cell(n) : "-")

/** The rows of `entity` (in the given relation spaces) as geodb-client prints a card with names. */
function geoOutput(entity: Entity, options: {relSpace?: (rel: Rel) => boolean; values?: boolean} = {}): string {
	const rows: string[] = []
	let n = 0
	if (options.values !== false) {
		rows.push(`${NAME}\t${cell(entity.name)}\tsn\t${HEX}\t-\t-\t1`)
		rows.push(`${DESCRIPTION}\t${cell(entity.description)}\tsd\t${HEX}\t-\t-\t1`)
	}
	if (options.values !== false)
		for (const v of entity.values.nodes) {
			const key = valueKey({
				property_id: v.property.id,
				text: v.text,
				integer: null,
				float: null,
				decimal: null,
				boolean: null,
				datetime: v.datetime,
				date: null,
			})
			if (!key) throw new Error("not mappable")
			rows.push(
				`${conformingId("entity", v.property.id)}\t${cell(key.norm)}\ts${n++}\t${HEX}\t${nameOf(v.property.name)}\t-\t1`,
			)
		}
	for (const r of [...entity.lightRels.nodes, ...entity.heavyRels.nodes]) {
		if (options.relSpace && !options.relSpace(r)) continue
		rows.push(
			`${conformingId("entity", r.type.id)}\t${conformingId("entity", r.toEntity.id)}\ts${n++}\t${HEX}\t${nameOf(r.type.name)}\t${nameOf(r.toEntity.name)}\t1`,
		)
	}
	return `${rows.join("\n")}\n`
}

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v))
const askOf = (outputs: Record<string, string>) =>
	vi.fn(async ({geoSpaceId, entityId, names}: {geoSpaceId: string; entityId: string; names?: boolean}) =>
		rowsFromGeoDbOutput(outputs[geoSpaceId] ?? "", entityId, {names}),
	)

/** One multi-space pipe line's answer: each Space's rows with the statement cell prefixed `<space>:`. */
const askSpacesOf = (outputs: Record<string, string>) =>
	vi.fn(async ({geoSpaceIds, entityId, names}: {geoSpaceIds: string[]; entityId: string; names?: boolean}) => {
		const text = geoSpaceIds
			.flatMap((geo) =>
				(outputs[geo] ?? "")
					.split("\n")
					.filter(Boolean)
					.map((line) =>
						line
							.split("\t")
							.map((c, i) => (i === 2 ? `${geo}:${c}` : c))
							.join("\t"),
					),
			)
			.join("\n")
		return rowsFromGeoDbOutput(`${text}\n`, entityId, {names, spaces: true})
	})

function setup(outputs: Record<string, string>, budget?: ReturnType<typeof createReadBudget>, multiSpace = false) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const readCard = askOf(outputs)
	const readCardSpaces = askSpacesOf(outputs)
	const deps = {
		multiSpace,
		geoDb: {readCard, readCardSpaces},
		geoSpaceOf: async (space: string) => (space === SPACE_D ? null : geoSpace(space)),
		log,
		budget,
	}
	return {log, readCard, readCardSpaces, entity: createEntityShadow(deps), points: createPointReadShadows(deps)}
}

const call = (shadow: (c: never) => Promise<void>, query: string, variables: Record<string, unknown>, body: unknown) =>
	shadow({
		classification: {operationName: null, operationType: "query", rootFields: []},
		params: {query, operationName: null, variables},
		document: parse(query),
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

const runEntity = (entity: Entity, outputs: Record<string, string>, budget?: ReturnType<typeof createReadBudget>) => {
	const s = setup(outputs, budget)
	const body = {data: {entity}}
	return call(s.entity as never, FIXTURE.request.query, {}, body).then(() => s)
}

describe("the light entity selection (news app)", () => {
	it("plans the real query", () => {
		const analysis = analyzeLightEntity(parse(FIXTURE.request.query), null, {})
		expect(analysis.ok).toBe(true)
		if (analysis.ok) {
			expect(analysis.plan.lists.map((l) => l.kind)).toEqual(["values", "relations", "relations"])
			expect(analysis.plan.uncompared).toContain("partial_values")
			expect(analysis.plan.uncompared).toContain("nested_entity")
		}
	})

	it("matches when geo-db holds every row PostGraphile returned", async () => {
		const s = await runEntity(ENTITY, {[geoSpace(SPACE_A)]: geoOutput(ENTITY)})
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({partial_values: 1, nested_entity: 1})
		expect(s.readCard).toHaveBeenCalledTimes(1)
		expect(s.log.warn).not.toHaveBeenCalled()
	})

	it("does not count rows only geo-db holds (the lists are filtered and cut)", async () => {
		const extra = `${conformingId("entity", "7ed45f2b-c48b-419e-8e46-64d5ff680b0d")}\tabc\tsx\t${HEX}\t-\t-\t1\n`
		await runEntity(ENTITY, {[geoSpace(SPACE_A)]: geoOutput(ENTITY) + extra})
		expect(results()).toEqual({match: 1})
	})

	it("flags a relation geo-db lacks", async () => {
		const missing = ENTITY.lightRels.nodes[0] as Rel
		const s = await runEntity(ENTITY, {
			[geoSpace(SPACE_A)]: geoOutput(ENTITY, {relSpace: (r) => r.toEntity.id !== missing.toEntity.id}),
		})
		expect(results()).toEqual({mismatch: 1})
		expect(s.log.warn).toHaveBeenCalledWith(
			"gateway entity shadow mismatch (light)",
			expect.objectContaining({first: expect.objectContaining({missingInGeoDb: 1})}),
		)
	})

	it("flags a value geo-db lacks", async () => {
		await runEntity(ENTITY, {
			[geoSpace(SPACE_A)]: geoOutput({...ENTITY, values: {nodes: ENTITY.values.nodes.slice(1)}}),
		})
		expect(results()).toEqual({mismatch: 1})
	})

	it("flags a wrong name in an entity that lives in one space", async () => {
		const wrong = clone(ENTITY)
		wrong.name = "Something else"
		await runEntity(wrong, {[geoSpace(SPACE_A)]: geoOutput(ENTITY)})
		expect(results()).toEqual({mismatch: 1})
	})

	it("checks a row in the space it names, not in any space", async () => {
		const two = clone(ENTITY)
		two.spaceIds = [SPACE_A, SPACE_B]
		const moved = two.lightRels.nodes[0] as Rel
		moved.spaceId = SPACE_B
		// geo-db holds that relation in A only: B's card lacks it.
		await runEntity(two, {
			[geoSpace(SPACE_A)]: geoOutput(two),
			[geoSpace(SPACE_B)]: "",
		})
		expect(results()).toEqual({mismatch: 1})
	})

	it("reads every space of the entity and matches when each row is in its space", async () => {
		const two = clone(ENTITY)
		two.spaceIds = [SPACE_A, SPACE_B]
		const moved = two.lightRels.nodes[0] as Rel
		moved.spaceId = SPACE_B
		const s = await runEntity(two, {
			[geoSpace(SPACE_A)]: geoOutput(two, {relSpace: (r) => r !== moved}),
			[geoSpace(SPACE_B)]: geoOutput(two, {relSpace: (r) => r === moved, values: false}),
		})
		expect(results()).toEqual({match: 1})
		expect(s.readCard).toHaveBeenCalledTimes(2)
	})

	it("reads a wide card in one line when multi-space reads are on: one read, one budget unit, same verdict", async () => {
		const two = clone(ENTITY)
		two.spaceIds = [SPACE_A, SPACE_B]
		const moved = two.lightRels.nodes[0] as Rel
		moved.spaceId = SPACE_B
		const outputs = {
			[geoSpace(SPACE_A)]: geoOutput(two, {relSpace: (r) => r !== moved}),
			[geoSpace(SPACE_B)]: geoOutput(two, {relSpace: (r) => r === moved, values: false}),
		}
		let now = 0
		const budget = createReadBudget({perSecond: 1, burst: 1, now: () => now})
		const s = setup(outputs, budget, true)
		await call(s.entity as never, FIXTURE.request.query, {}, {data: {entity: two}})
		expect(results()).toEqual({match: 1})
		expect(s.readCard).not.toHaveBeenCalled()
		expect(s.readCardSpaces).toHaveBeenCalledTimes(1)
		expect(s.readCardSpaces.mock.calls[0]?.[0].geoSpaceIds).toEqual([geoSpace(SPACE_A), geoSpace(SPACE_B)])
		// The whole burst was one unit, so a second wide read is throttled, not a third.
		await call(s.entity as never, FIXTURE.request.query, {}, {data: {entity: two}})
		expect(unsupported()).toEqual({throttled: 1})
		// A wrong row is still caught: B's card lacks the relation.
		now += 1000
		const lacking = setup({...outputs, [geoSpace(SPACE_B)]: ""}, budget, true)
		await call(lacking.entity as never, FIXTURE.request.query, {}, {data: {entity: two}})
		expect(results()).toEqual({match: 1, mismatch: 1, unsupported_selection: 1})
	})

	it("sends nothing when one of the spaces is not loaded, even in one-line mode", async () => {
		const unloaded = clone(ENTITY)
		unloaded.spaceIds = [SPACE_A, SPACE_D]
		const s = setup({[geoSpace(SPACE_A)]: geoOutput(ENTITY)}, undefined, true)
		await call(s.entity as never, FIXTURE.request.query, {}, {data: {entity: unloaded}})
		expect(unsupported()).toEqual({unmapped_space: 1})
		expect(s.readCardSpaces).not.toHaveBeenCalled()
		expect(s.readCard).not.toHaveBeenCalled()
	})

	it("counts an entity in too many spaces, an unloaded space and an empty budget as unsupported", async () => {
		const many = clone(ENTITY)
		many.spaceIds = [SPACE_A, SPACE_B, SPACE_C, "ffffffffffff4fff8fffffffffffffff"]
		await runEntity(many, {})
		const unloaded = clone(ENTITY)
		unloaded.spaceIds = [SPACE_A, SPACE_D]
		await runEntity(unloaded, {[geoSpace(SPACE_A)]: geoOutput(ENTITY)})
		let now = 0
		const budget = createReadBudget({perSecond: 1, burst: 1, now: () => now})
		await runEntity(ENTITY, {[geoSpace(SPACE_A)]: geoOutput(ENTITY)}, budget)
		await runEntity(ENTITY, {[geoSpace(SPACE_A)]: geoOutput(ENTITY)}, budget)
		expect(unsupported()).toEqual({many_spaces: 1, unmapped_space: 1, throttled: 1})
		expect(results()).toEqual({match: 1, unsupported_selection: 3})
		now += 1000
		await runEntity(ENTITY, {[geoSpace(SPACE_A)]: geoOutput(ENTITY)}, budget)
		expect(results()).toEqual({match: 2, unsupported_selection: 3})
	})

	it("counts a failed read as failed", async () => {
		const s = setup({})
		s.readCard.mockRejectedValueOnce(new Error("pipe closed"))
		await call(s.entity as never, FIXTURE.request.query, {}, FIXTURE.response)
		expect(results()).toEqual({failed: 1})
	})

	it("needs the entity's spaces: without spaceIds or a row space it is no_space", () => {
		const analysis = analyzeLightEntity(
			parse(
				`{ entity(id: "073ff16b-e7fe-4591-a984-c325d629d92d") { id values(first: 5) { nodes { property { id } text } } } }`,
			),
			null,
			{},
		)
		expect(analysis).toEqual({ok: false, reason: "no_space"})
	})

	it("refuses an unknown column or a list argument it cannot interpret", () => {
		const q = (sel: string, args = "first: 5") =>
			analyzeLightEntity(
				parse(
					`{ entity(id: "073ff16b-e7fe-4591-a984-c325d629d92d") { spaceIds relations(${args}) { nodes { ${sel} } } } }`,
				),
				null,
				{},
			)
		expect(q("type { id } toEntity { id }").ok).toBe(true)
		expect(q("type { id } toEntity { id } bogus")).toEqual({ok: false, reason: "selection"})
		expect(q("type { id } toEntity { id }", "first: 5, orderBy: ID_ASC")).toEqual({ok: false, reason: "args"})
		expect(q("type { id }")).toEqual({ok: false, reason: "selection"})
	})
})

describe("the card selection with no space filter", () => {
	const query = `query($id: String!, $spaceId: String) {
		entity(id: $id) {
			id name spaceIds createdAt createdAtBlock relations { totalCount }
			valuesList(first: 100, filter: {spaceId: {is: $spaceId}}) { propertyId text }
			relationsList(first: 100, filter: {spaceId: {is: $spaceId}}) { typeId toEntityId }
		}
	}`

	it("is planned (every space) when the space variable is null, and reads the entity's spaces", async () => {
		const s = setup({
			[geoSpace(SPACE_A)]:
				`${NAME}\tN\ts1\t${HEX}\t-\t-\t1\n${TYPES}\t${conformingId("entity", "7ed45f2bc48b419e8e4664d5ff680b0d")}\ts2\t${HEX}\t-\t-\t1\n`,
			[geoSpace(SPACE_B)]: `${DESCRIPTION}\tD\ts3\t${HEX}\t-\t-\t1\n`,
		})
		const body = {
			data: {
				entity: {
					id: HEX,
					name: "N",
					spaceIds: [SPACE_A, SPACE_B],
					createdAt: "2026-01-01T00:00:00Z",
					createdAtBlock: "1",
					relations: {totalCount: 1},
					valuesList: [
						{propertyId: "a126ca53-0c8e-48d5-b888-82c734c38935", text: "N"},
						{propertyId: "9b1f76ff-9711-404c-861e-59dc3fa7d037", text: "D"},
					],
					relationsList: [
						{
							typeId: "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1",
							toEntityId: "7ed45f2b-c48b-419e-8e46-64d5ff680b0d",
						},
					],
				},
			},
		}
		await call(s.entity as never, query, {id: HEX, spaceId: null}, body)
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({created_at: 1, relation_count: 1})
		expect(s.readCard).toHaveBeenCalledTimes(2)
	})

	it("still counts no_space when the answer does not say which spaces the entity lives in", async () => {
		const q = query.replace("spaceIds", "")
		const s = setup({})
		await call(
			s.entity as never,
			q.replace("createdAt createdAtBlock relations { totalCount }", ""),
			{id: "a94cb341-0aea-4737-8dda-3b6489cbacef", spaceId: null},
			{data: {entity: {id: HEX, name: null}}},
		)
		expect(unsupported()).toEqual({no_space: 1})
	})
})

describe("entities with the light selection", () => {
	const filtered = (filter: string) =>
		FIXTURE.request.query.replace('entity(id: "' + HEX + '")', `entities(filter: ${filter})`)
	const list = (n: number): Entity[] =>
		Array.from({length: n}, (_, at) => {
			const e = clone(ENTITY)
			e.id = `${at.toString(16).padStart(2, "0")}${HEX.slice(2)}`
			return e
		})
	const outputsFor = (entities: Entity[]) => {
		// One card per entity: the same rows under each id.
		const out = entities.map((e) => geoOutput(e).replaceAll(HEX, e.id)).join("")
		return {[geoSpace(SPACE_A)]: out}
	}
	const typeId = "e550fe517e904b2c8fffdf13408f5634"

	it("plans the real filter shapes", () => {
		const a = analyzeLightEntities(
			parse(
				filtered(
					`{typeIds: {anyEqualTo: "${typeId}"}, relations: {some: {toEntityId: {is: "16d5e35f207544de85ac5fd287b04f44"}, typeId: {is: "6e3503fab974460ea3dbab8af9a41427"}}}}`,
				),
			),
			null,
			{},
		)
		expect(a.ok).toBe(true)
		if (a.ok) {
			expect(a.plan.claims.map((c) => c.kind)).toEqual(["type", "edge"])
			expect(a.plan.uncompared).toEqual(expect.arrayContaining(["filter_completeness", "sampled_rows"]))
		}
	})

	const withArgs = (args: string) => FIXTURE.request.query.replace('entity(id: "' + HEX + '")', `entities(${args})`)
	const SPACE_B = "1b5c5f5c-2d5f-4f6a-9f0d-6d1c0c4f9a11"

	it("plans the Space and Type root arguments", () => {
		const a = analyzeLightEntities(
			parse(
				withArgs(
					`spaceIds: {in: ["${SPACE_A}", "${SPACE_B}"]}, typeIds: {in: ["${typeId}", "${HEX}"]}, first: 5`,
				),
			),
			null,
			{},
		)
		expect(a.ok).toBe(true)
		if (a.ok) {
			expect(a.plan.spaceIds).toHaveLength(2)
			expect(a.plan.claims).toEqual([{kind: "type", typeIds: [expect.any(String), expect.any(String)]}])
		}
		const single = analyzeLightEntities(parse(withArgs(`typeId: "${typeId}", spaceId: "${SPACE_A}"`)), null, {})
		expect(single.ok && single.plan.claims.map((c) => c.kind)).toEqual(["type"])
		expect(single.ok && single.plan.spaceIds?.length).toBe(1)
	})

	it("plans a selection with no lists when the derived fields and spaceIds are asked for", () => {
		const q = (selection: string) => `query { entities(typeId: "${typeId}", first: 5) { ${selection} } }`
		expect(analyzeLightEntities(parse(q("id name description spaceIds")), null, {}).ok).toBe(true)
		// Nothing to judge, or no Space to read.
		expect(analyzeLightEntities(parse(q("id spaceIds")), null, {})).toEqual({ok: false, reason: "selection"})
		// No Space to read: every Space is (see "entities answers with no Space" below).
		const names = analyzeLightEntities(parse(q("id name")), null, {})
		expect(names.ok && names.plan.everySpace).toBe(true)
		// A Space argument narrows to that Space, which says nothing about a name from another.
		const scoped = `query { entities(spaceId: "${SPACE_A}", first: 5) { id name } }`
		expect(analyzeLightEntities(parse(scoped), null, {})).toEqual({ok: false, reason: "selection"})
	})

	it("refuses a malformed Space or Type argument", () => {
		for (const args of [
			`spaceIds: ["${SPACE_A}"]`,
			`typeIds: {in: []}`,
			`typeIds: {overlaps: ["${typeId}"]}`,
			`typeId: "nope"`,
		])
			expect(analyzeLightEntities(parse(withArgs(args)), null, {})).toEqual({ok: false, reason: "args"})
	})

	it("flags an entity outside every Space the spaceIds argument names", async () => {
		const entities = list(1)
		const s = setup(outputsFor(entities))
		await call(
			s.points.get("entities") as never,
			withArgs(`spaceIds: {in: ["${SPACE_B}"]}`),
			{},
			{data: {entities}},
		)
		expect(results()).toEqual({mismatch: 1})
	})

	it("accepts an entity in one of the Spaces the spaceIds argument names", async () => {
		const entities = list(1)
		const s = setup(outputsFor(entities))
		await call(
			s.points.get("entities") as never,
			withArgs(`spaceIds: {in: ["${SPACE_B}", "${SPACE_A}"]}`),
			{},
			{data: {entities}},
		)
		expect(results()).toEqual({match: 1})
	})

	it("compares a sample of the returned entities and checks the filter's promise on each", async () => {
		const entities = list(7)
		const claimed = `{typeIds: {anyEqualTo: "${typeId}"}}`
		const withType = (e: Entity) =>
			// geo-db holds the Types statement; PostGraphile's `types` is not selected in this query.
			geoOutput(e).replaceAll(HEX, e.id) + `${TYPES}\t${conformingId("entity", typeId)}\tt\t${e.id}\t-\t-\t1\n`
		const s = setup({[geoSpace(SPACE_A)]: entities.map(withType).join("")})
		await call(s.points.get("entities") as never, filtered(claimed), {}, {data: {entities}})
		expect(results()).toEqual({match: 1})
		expect(s.readCard).toHaveBeenCalledTimes(3)
		expect(uncompared()).toMatchObject({sampled_rows: 1, filter_completeness: 1})
	})

	it("flags an entity the filter promised a type it does not hold", async () => {
		const entities = list(2)
		const s = setup(outputsFor(entities))
		await call(
			s.points.get("entities") as never,
			filtered(`{typeIds: {anyEqualTo: "${typeId}"}}`),
			{},
			{data: {entities}},
		)
		expect(results()).toEqual({mismatch: 1})
		expect(s.log.warn).toHaveBeenCalledWith(
			"gateway entities shadow mismatch (light)",
			expect.objectContaining({first: expect.objectContaining({unmetFilter: [expect.anything()]})}),
		)
	})

	it("flags an entity returned for an id list that did not ask for it", async () => {
		const entities = list(2)
		const asked = entities[0]?.id
		const s = setup(outputsFor(entities))
		await call(s.points.get("entities") as never, filtered(`{id: {in: ["${asked}"]}}`), {}, {data: {entities}})
		expect(results()).toEqual({mismatch: 1})
	})

	it("an empty answer compares nothing", async () => {
		const s = setup({})
		await call(
			s.points.get("entities") as never,
			filtered(`{typeIds: {anyEqualTo: "${typeId}"}}`),
			{},
			{data: {entities: []}},
		)
		expect(results()).toEqual({})
	})
})

// geogenesis's EntityTypes (every entity page) and EntityExists (the post-publish poll) say nothing about where
// the entity lives, so the shadow reads the card across every Space in the space map, in one pipe line.
describe("entity answers with no Space (every-Space reads)", () => {
	type Captured = {request: {query: string; variables: Record<string, unknown>}; response: {data: {entity: unknown}}}
	const EVERY: Record<"entityTypes" | "entityExists" | "entityAbsent", Captured> = JSON.parse(
		readFileSync(new URL("./fixtures/entity-every-space.json", import.meta.url), "utf8"),
	)
	const NEWS_STORY = "e550fe517e904b2c8fffdf13408f5634"
	const ABSENT = conformingId("entity", "0f3c2a51-7d4e-4b1a-9c2e-5a6b7c8d9e0f")
	const ALL = [geoSpace(SPACE_A), geoSpace(SPACE_B), geoSpace(SPACE_C)]
	/** A Types relation row as geodb-client prints it with names. */
	const typeRow = (entity: string, type: string, name: string, n = 90) =>
		`${TYPES}\t${conformingId("entity", type)}\ts${n}\t${entity}\tTypes\t${name}\t1\n`

	function everySetup(
		outputs: Record<string, string>,
		options: {flag?: boolean; budget?: ReturnType<typeof createReadBudget>} = {},
	) {
		const log = {info: vi.fn(), warn: vi.fn()}
		const readCard = askOf(outputs)
		const readCardSpaces = askSpacesOf(outputs)
		const allGeoSpaces = vi.fn(async () => ALL)
		const deps = {
			multiSpace: true,
			geoDb: {readCard, readCardSpaces},
			geoSpaceOf: async (space: string) => geoSpace(space),
			log,
			...(options.flag === false ? {} : {allGeoSpaces, everySpaceBudget: options.budget}),
		}
		return {log, readCard, readCardSpaces, allGeoSpaces, entity: createEntityShadow(deps)}
	}
	const run = (s: ReturnType<typeof everySetup>, captured: Captured, body: unknown = captured.response) =>
		call(s.entity as never, captured.request.query, captured.request.variables, body)

	it("treats a null filter operand as no filter (PostGraphile drops null and empty input)", () => {
		expect(isEmptyFilter(undefined)).toBe(true)
		expect(isEmptyFilter(null)).toBe(true)
		expect(isEmptyFilter({spaceIds: {anyEqualTo: null}})).toBe(true)
		expect(isEmptyFilter({spaceIds: {}})).toBe(true)
		expect(isEmptyFilter({spaceIds: {anyEqualTo: SPACE_A}})).toBe(false)
		expect(isEmptyFilter({id: {in: []}})).toBe(false)
	})

	it("plans EntityTypes with a null space and EntityExists as every-Space reads", () => {
		for (const captured of [EVERY.entityTypes, EVERY.entityExists]) {
			const analysis = analyzeLightEntity(parse(captured.request.query), null, captured.request.variables)
			expect(analysis.ok && analysis.plan.everySpace).toBe(true)
		}
		// A real space filter on the types narrows them to types held in that Space, which a card cannot say.
		const scoped = analyzeLightEntity(parse(EVERY.entityTypes.request.query), null, {
			...EVERY.entityTypes.request.variables,
			spaceId: SPACE_A,
		})
		expect(scoped).toEqual({ok: false, reason: "selection"})
	})

	it("matches EntityTypes against the Types statements of every Space, in one read", async () => {
		const s = everySetup({[geoSpace(SPACE_A)]: geoOutput(ENTITY) + typeRow(HEX, NEWS_STORY, "News story")})
		await run(s, EVERY.entityTypes)
		expect(results()).toEqual({match: 1})
		expect(s.readCard).not.toHaveBeenCalled()
		expect(s.readCardSpaces).toHaveBeenCalledTimes(1)
		expect(s.readCardSpaces.mock.calls[0]?.[0]).toMatchObject({geoSpaceIds: ALL, names: true})
	})

	it("flags a type geo-db holds that PostGraphile does not list; a differing type name is inconclusive", async () => {
		const other = "0123456789ab4bcd8ef0123456789abd"
		const extra = everySetup({
			[geoSpace(SPACE_A)]: geoOutput(ENTITY) + typeRow(HEX, NEWS_STORY, "News story"),
			[geoSpace(SPACE_C)]: typeRow(HEX, other, "Person", 91),
		})
		await run(extra, EVERY.entityTypes)
		expect(results()).toEqual({mismatch: 1})
		const renamed = everySetup({[geoSpace(SPACE_A)]: typeRow(HEX, NEWS_STORY, "Op-ed")})
		await run(renamed, EVERY.entityTypes)
		// geo-db names the type in the Space it read; PostGraphile may have taken the name from another.
		expect(results()).toEqual({match: 1, mismatch: 1})
		expect(uncompared()).toMatchObject({cross_space: 1})
	})

	it("leaves a type PostGraphile has from a Space geo-db did not answer for inconclusive, not wrong", async () => {
		const s = everySetup({[geoSpace(SPACE_A)]: geoOutput(ENTITY)})
		await run(s, EVERY.entityTypes)
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({cross_space: 1})
	})

	it("matches EntityExists when geo-db holds the entity somewhere", async () => {
		const s = everySetup({[geoSpace(SPACE_B)]: geoOutput(ENTITY)})
		await run(s, EVERY.entityExists)
		expect(results()).toEqual({match: 1})
		expect(uncompared().no_statements).toBeUndefined()
	})

	it("counts an entity PostGraphile has but geo-db holds no statement about as uncompared, not a mismatch", async () => {
		const s = everySetup({})
		await run(s, EVERY.entityExists)
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({no_statements: 1})
	})

	it("checks an absent entity: geo-db must hold nothing about it in any Space", async () => {
		const none = everySetup({[geoSpace(SPACE_A)]: geoOutput(ENTITY)})
		await run(none, EVERY.entityAbsent)
		expect(results()).toEqual({match: 1})
		expect(none.readCardSpaces.mock.calls[0]?.[0].entityId).toBe(ABSENT)
		const held = everySetup({[geoSpace(SPACE_B)]: `${NAME}\tGhost\tsn\t${ABSENT}\t-\t-\t1\n`})
		await run(held, EVERY.entityAbsent)
		expect(results()).toEqual({match: 1, mismatch: 1})
		expect(held.log.warn).toHaveBeenCalledWith(
			expect.stringContaining("mismatch"),
			expect.objectContaining({absentInPostgres: true, holders: 1}),
		)
	})

	it("stays no_space and reads nothing with the switch off", async () => {
		const s = everySetup({[geoSpace(SPACE_A)]: geoOutput(ENTITY)}, {flag: false})
		await run(s, EVERY.entityExists)
		await run(s, EVERY.entityAbsent)
		await run(s, EVERY.entityTypes)
		expect(unsupported()).toEqual({no_space: 3})
		expect(s.readCard).not.toHaveBeenCalled()
		expect(s.readCardSpaces).not.toHaveBeenCalled()
	})

	it("spends one unit of its own budget a read, and counts an empty budget as throttled", async () => {
		const budget = createReadBudget({perSecond: 1, burst: 1, now: () => 0})
		const s = everySetup({[geoSpace(SPACE_A)]: geoOutput(ENTITY)}, {budget})
		await run(s, EVERY.entityExists)
		await run(s, EVERY.entityExists)
		expect(results()).toEqual({match: 1, unsupported_selection: 1})
		expect(unsupported()).toEqual({throttled: 1})
		expect(s.readCardSpaces).toHaveBeenCalledTimes(1)
	})

	it("counts a failed every-Space read as failed", async () => {
		const s = everySetup({})
		s.readCardSpaces.mockRejectedValueOnce(new Error("pipe closed"))
		await run(s, EVERY.entityExists)
		expect(results()).toEqual({failed: 1})
	})
})

// geogenesis's EntityNames (topic menus, relation pickers) asks `entities(filter: {id: {in}}, first) { id name }`: no
// Space anywhere, so one sampled entity is read across every Space in the space map.
describe("entities answers with no Space (every-Space reads)", () => {
	type Captured = {
		request: {query: string; variables: Record<string, unknown>}
		response: {data: {entities: {id: string; name: string}[]}}
	}
	const NAMES: Captured = JSON.parse(readFileSync(new URL("./fixtures/entity-names.json", import.meta.url), "utf8"))
	const [FIRST] = NAMES.response.data.entities
	if (!FIRST) throw new Error("fixture has no entities")
	const ALL = [geoSpace(SPACE_A), geoSpace(SPACE_B), geoSpace(SPACE_C)]
	const nameRow = (entity: string, name: string) => `${NAME}\t${cell(name)}\tsn\t${entity}\t-\t-\t1\n`

	function namesSetup(
		outputs: Record<string, string>,
		options: {flag?: boolean; budget?: ReturnType<typeof createReadBudget>} = {},
	) {
		const log = {info: vi.fn(), warn: vi.fn()}
		const readCard = askOf(outputs)
		const readCardSpaces = askSpacesOf(outputs)
		const deps = {
			multiSpace: true,
			geoDb: {readCard, readCardSpaces},
			geoSpaceOf: async (space: string) => geoSpace(space),
			log,
			...(options.flag === false ? {} : {allGeoSpaces: async () => ALL, everySpaceBudget: options.budget}),
		}
		return {log, readCard, readCardSpaces, points: createPointReadShadows(deps)}
	}
	const run = (s: ReturnType<typeof namesSetup>, body: unknown = NAMES.response) =>
		call(s.points.get("entities") as never, NAMES.request.query, NAMES.request.variables, body)

	it("plans EntityNames as an every-Space read of the ids it asks for", () => {
		const analysis = analyzeLightEntities(parse(NAMES.request.query), "EntityNames", NAMES.request.variables)
		expect(analysis.ok).toBe(true)
		if (analysis.ok) {
			expect(analysis.plan.everySpace).toBe(true)
			expect(analysis.plan.ids).toHaveLength(2)
			expect(analysis.plan.uncompared).toEqual(expect.arrayContaining(["partial_values", "sampled_rows"]))
		}
	})

	it("matches the name geo-db holds in any Space, reading one sampled entity in one line", async () => {
		const s = namesSetup({[geoSpace(SPACE_C)]: nameRow(FIRST.id, FIRST.name)})
		await run(s)
		expect(results()).toEqual({match: 1})
		expect(s.readCard).not.toHaveBeenCalled()
		expect(s.readCardSpaces).toHaveBeenCalledTimes(1)
		expect(s.readCardSpaces.mock.calls[0]?.[0]).toMatchObject({
			geoSpaceIds: ALL,
			entityId: conformingId("entity", FIRST.id),
			names: true,
		})
	})

	it("leaves a differing name inconclusive: PostGraphile may have taken it from a Space geo-db did not load", async () => {
		const s = namesSetup({[geoSpace(SPACE_A)]: nameRow(FIRST.id, "Op-ed")})
		await run(s)
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({cross_space: 1})
	})

	it("flags an entity PostGraphile has no name for that geo-db names", async () => {
		const s = namesSetup({[geoSpace(SPACE_B)]: nameRow(FIRST.id, FIRST.name)})
		const unnamed = clone(NAMES.response)
		const [first] = unnamed.data.entities
		if (first) (first as {name: string | null}).name = null
		await run(s, unnamed)
		expect(results()).toEqual({mismatch: 1})
	})

	it("counts an entity geo-db holds nothing about as uncompared, not a mismatch", async () => {
		const s = namesSetup({})
		await run(s)
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({no_statements: 1})
	})

	it("flags an id PostGraphile returned that the filter did not ask for", async () => {
		const s = namesSetup({[geoSpace(SPACE_A)]: nameRow(FIRST.id, FIRST.name)})
		const stray = clone(NAMES.response)
		stray.data.entities.push({id: "0123456789ab4bcd8ef0123456789abc", name: "Stray"})
		await run(s, stray)
		expect(results()).toEqual({mismatch: 1})
	})

	it("stays no_space and reads nothing with the switch off", async () => {
		const s = namesSetup({[geoSpace(SPACE_A)]: nameRow(FIRST.id, FIRST.name)}, {flag: false})
		await run(s)
		expect(unsupported()).toEqual({no_space: 1})
		expect(s.readCard).not.toHaveBeenCalled()
		expect(s.readCardSpaces).not.toHaveBeenCalled()
	})

	it("spends the every-Space budget, and counts an empty one as throttled", async () => {
		const budget = createReadBudget({perSecond: 1, burst: 1, now: () => 0})
		const s = namesSetup({[geoSpace(SPACE_A)]: nameRow(FIRST.id, FIRST.name)}, {budget})
		await run(s)
		await run(s)
		expect(results()).toEqual({match: 1, unsupported_selection: 1})
		expect(unsupported()).toEqual({throttled: 1})
		expect(s.readCardSpaces).toHaveBeenCalledTimes(1)
	})

	it("checks the claims of an { id } selection's filter in every Space", async () => {
		const typeId = "e550fe517e904b2c8fffdf13408f5634"
		const query = `query { entities(filter: {typeIds: {anyEqualTo: "${typeId}"}}) { id } }`
		const typed = `${TYPES}\t${conformingId("entity", typeId)}\ts1\t${FIRST.id}\t-\t-\t1\n`
		const body = {data: {entities: [{id: FIRST.id}]}}
		const held = namesSetup({[geoSpace(SPACE_B)]: typed})
		await call(held.points.get("entities") as never, query, {}, body)
		expect(results()).toEqual({match: 1})
		const other = namesSetup({[geoSpace(SPACE_B)]: nameRow(FIRST.id, FIRST.name)})
		await call(other.points.get("entities") as never, query, {}, body)
		expect(results()).toEqual({match: 1, mismatch: 1})
		// Nothing held anywhere: not judged.
		const none = namesSetup({})
		await call(none.points.get("entities") as never, query, {}, body)
		expect(results()).toEqual({match: 2, mismatch: 1})
		expect(uncompared()).toMatchObject({no_statements: 1})
	})

	it("skips an empty answer", async () => {
		const s = namesSetup({})
		await run(s, {data: {entities: []}})
		expect(results()).toEqual({})
		expect(s.readCardSpaces).not.toHaveBeenCalled()
	})
})
