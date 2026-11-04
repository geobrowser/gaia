import {readFileSync} from "node:fs"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, rowsFromGeoDbOutput, valueKey} from "../../geodb/card"
import {createEntityShadow} from "../entityShadow"
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
