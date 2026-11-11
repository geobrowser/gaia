import {readFileSync} from "node:fs"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {CREATED_AT_PROPERTY, conformingId, rowsFromGeoDbOutput, UPDATED_AT_PROPERTY, valueKey} from "../../geodb/card"
import {GeoDbError, holdersDocument} from "../../geodb/client"
import {createEntitiesConnectionShadow, createEntitiesOrderedShadow} from "../entitiesConnectionShadow"
import {createEntityShadow, createHoldersGate, type HoldersGate, isEmptyFilter} from "../entityShadow"
import {analyzeLightEntities, analyzeLightEntity} from "../lightShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {mergeIsExact, mergePages} from "../orderPickShadow"
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
// The captured `values` list trims long texts; geo-db holds the name and the description once, in full.
for (const v of ENTITY.values.nodes) {
	const property = conformingId("entity", v.property.id)
	if (property === conformingId("entity", "a126ca53-0c8e-48d5-b888-82c734c38935")) v.text = ENTITY.name
	if (property === conformingId("entity", "9b1f76ff-9711-404c-861e-59dc3fa7d037")) v.text = ENTITY.description
}
const HEX = ENTITY.id
const SPACE_A = ENTITY.spaceIds[0] as string
const SPACE_B = "5093b6ab1b48f1111dd31275b5413ddb"
const SPACE_C = "41e851610e13a19441c4d980f2f2ce6b"
const SPACE_B_UUID = "1b5c5f5c-2d5f-4f6a-9f0d-6d1c0c4f9a11"
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
		// PostGraphile's `values` list holds the name and the description too; geo-db holds each once, so they are
		// written here (from the entity's own text) only when the list does not carry them.
		const listed = (p: string) => entity.values.nodes.some((n) => conformingId("entity", n.property.id) === p)
		if (!listed(NAME)) rows.push(`${NAME}\t${cell(entity.name)}\tsn\t${HEX}\t-\t-\t1`)
		if (!listed(DESCRIPTION)) rows.push(`${DESCRIPTION}\t${cell(entity.description)}\tsd\t${HEX}\t-\t-\t1`)
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
	return {log, readCard, readCardSpaces, deps, entity: createEntityShadow(deps), points: createPointReadShadows(deps)}
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

	it("flags a row only geo-db holds when the list was not cut", async () => {
		const extra = `${conformingId("entity", "7ed45f2b-c48b-419e-8e46-64d5ff680b0d")}\tabc\tsx\t${HEX}\t-\t-\t1\n`
		const s = await runEntity(ENTITY, {[geoSpace(SPACE_A)]: geoOutput(ENTITY) + extra})
		expect(results()).toEqual({mismatch: 1})
		expect(s.log.warn).toHaveBeenCalledWith(
			"gateway entity shadow mismatch (light)",
			expect.objectContaining({first: expect.objectContaining({geoOnlyRows: 1})}),
		)
	})

	it("flags a relation only geo-db holds in a Type the list names", async () => {
		const type = (ENTITY.lightRels.nodes[0] as Rel).type.id
		const extra = `${conformingId("entity", type)}\t${conformingId("entity", "7ed45f2b-c48b-419e-8e46-64d5ff680b0d")}\tsy\t${HEX}\t-\t-\t1\n`
		await runEntity(ENTITY, {[geoSpace(SPACE_A)]: geoOutput(ENTITY) + extra})
		expect(results()).toEqual({mismatch: 1})
	})

	it("does not count a relation of a Type the list's filter leaves out", async () => {
		const extra = `${conformingId("entity", "7ed45f2b-c48b-419e-8e46-64d5ff680b0d")}\t${conformingId("entity", "7ed45f2b-c48b-419e-8e46-64d5ff680b0e")}\tsy\t${HEX}\t-\t-\t1\n`
		await runEntity(ENTITY, {[geoSpace(SPACE_A)]: geoOutput(ENTITY) + extra})
		expect(results()).toEqual({match: 1})
	})

	it("leaves a list that reached its cut uncompared instead of flagging rows beyond it", async () => {
		const cut = clone(ENTITY)
		cut.values.nodes = cut.values.nodes.slice(0, 2)
		const extra = `${conformingId("entity", "7ed45f2b-c48b-419e-8e46-64d5ff680b0d")}\tabc\tsx\t${HEX}\t-\t-\t1\n`
		const query = FIXTURE.request.query.replace("values(first: 50)", "values(first: 2)")
		const s = setup({[geoSpace(SPACE_A)]: geoOutput(cut) + extra})
		await call(s.entity as never, query, {}, {data: {entity: cut}})
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({partial_values: 1})
	})

	describe("a nested name geo-db prints as none (the target is named in a Space outside the read)", () => {
		// geo-db's card names a row's target only from the Spaces read: blank every target name in the output.
		const unnamed = (text: string) =>
			text
				.split("\n")
				.map((line) => {
					const cells = line.split("\t")
					return cells.length === 7 && cells[3] === HEX
						? [...cells.slice(0, 5), "-", cells[6]].join("\t")
						: line
				})
				.join("\n")
		const withNames = (names: Map<string, string[]> | "refuse", flag: boolean) => {
			const s = setup({[geoSpace(SPACE_A)]: unnamed(geoOutput(ENTITY))})
			const readCards = vi.fn(async ({entityIds}: {entityIds: string[]}) => {
				if (names === "refuse") throw new Error("down")
				return entityIds.flatMap((entity) =>
					(names.get(entity) ?? []).map((raw) => ({entity, prop: NAME, raw})),
				)
			})
			const deps = {...s.deps, allGeoSpaces: async () => [geoSpace(SPACE_A)], crossSpaceNames: flag}
			return {s, readCards, shadow: createEntityShadow({...deps, geoDb: {...s.deps.geoDb, readCards}} as never)}
		}
		const everyName = () => {
			const names = new Map<string, string[]>()
			for (const r of [...ENTITY.lightRels.nodes, ...ENTITY.heavyRels.nodes] as Rel[]) {
				if (r.toEntity.name) names.set(conformingId("entity", r.toEntity.id), [r.toEntity.name])
			}
			return names
		}
		const run = (shadow: ReturnType<typeof withNames>["shadow"]) =>
			call(shadow as never, FIXTURE.request.query, {}, {data: {entity: ENTITY}})

		it("stays cross_space when the switch is off", async () => {
			const w = withNames(everyName(), false)
			await run(w.shadow)
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({cross_space: 1})
			expect(w.readCards).not.toHaveBeenCalled()
		})

		it("compares the name against its Name in every Space when the switch is on", async () => {
			const w = withNames(everyName(), true)
			await run(w.shadow)
			expect(results()).toEqual({match: 1})
			expect(uncompared().cross_space).toBeUndefined()
			expect(w.readCards).toHaveBeenCalledTimes(1)
		})

		it("keeps it inconclusive when no Space holds that name, or the read fails", async () => {
			for (const names of [new Map<string, string[]>(), "refuse" as const]) {
				__resetGatewayMetricsForTests()
				await run(withNames(names, true).shadow)
				expect(results()).toEqual({match: 1})
				expect(uncompared()).toMatchObject({cross_space: 1})
			}
		})
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

	it("compares an entity in more than three spaces in one line when multi-space reads are on", async () => {
		const wide = clone(ENTITY)
		const extra = [
			"aaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa",
			"bbbbbbbbbbbb4bbb8bbbbbbbbbbbbbbb",
			"cccccccccccc4ccc8ccccccccccccccc",
		]
		wide.spaceIds = [SPACE_A, SPACE_B, SPACE_C, ...extra]
		const moved = wide.lightRels.nodes[0] as Rel
		moved.spaceId = SPACE_B
		const outputs = {
			[geoSpace(SPACE_A)]: geoOutput(wide, {relSpace: (r) => r !== moved}),
			[geoSpace(SPACE_B)]: geoOutput(wide, {relSpace: (r) => r === moved, values: false}),
		}
		const s = setup(outputs, undefined, true)
		await call(s.entity as never, FIXTURE.request.query, {}, {data: {entity: wide}})
		expect(results()).toEqual({match: 1})
		expect(s.readCardSpaces).toHaveBeenCalledTimes(1)
		expect(s.readCardSpaces.mock.calls[0]?.[0].geoSpaceIds).toHaveLength(6)
		// Deliberate mismatch: geo-db lacks the relation PostGraphile returned from B.
		const lacking = setup({...outputs, [geoSpace(SPACE_B)]: ""}, undefined, true)
		await call(lacking.entity as never, FIXTURE.request.query, {}, {data: {entity: wide}})
		expect(results()).toEqual({match: 1, mismatch: 1})
		expect(unsupported()).toEqual({})
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

	it("reads every space when there is no spaceIds and no row space (the rows need only be held by one)", () => {
		const analysis = analyzeLightEntity(
			parse(
				`{ entity(id: "073ff16b-e7fe-4591-a984-c325d629d92d") { id values(first: 5) { nodes { property { id } text } } } }`,
			),
			null,
			{},
		)
		expect(analysis.ok).toBe(true)
		if (analysis.ok) expect(analysis.plan.everySpace).toBe(true)
	})

	it("reads every Space when the root names Spaces but the nested rows name none (the root does not scope them)", () => {
		const analysis = analyzeLightEntities(
			parse(
				`{ entities(spaceIds: {in: ["${SPACE_B_UUID}"]}, first: 5) { id values(first: 5) { nodes { property { id } text } } } }`,
			),
			null,
			{},
		)
		expect(analysis.ok).toBe(true)
		if (analysis.ok) {
			expect(analysis.plan.everySpace).toBe(true)
			expect(analysis.plan.spaceIds).toEqual([SPACE_B_UUID.replaceAll("-", "")])
		}
		// With no lists and no spaceIds selected there is still nothing to judge.
		expect(
			analyzeLightEntities(parse(`{ entities(spaceIds: {in: ["${SPACE_B_UUID}"]}, first: 5) { id } }`), null, {}),
		).toEqual({ok: false, reason: "selection"})
	})

	it("plans `entities(spaceId, orderBy: CREATED_AT_DESC) { id createdAt }` (a newest-entity probe) and still refuses a bare id", () => {
		const q = (sel: string) =>
			parse(`{ entities(spaceId: "${SPACE_A}", first: 1, orderBy: CREATED_AT_DESC) { ${sel} } }`)
		const a = analyzeLightEntities(q("id createdAt"), null, {})
		expect(a.ok).toBe(true)
		if (a.ok) {
			expect(a.plan.everySpace).toBe(true)
			expect(a.plan.uncompared).toContain("order_completeness")
		}
		expect(analyzeLightEntities(q("id"), null, {})).toEqual({ok: false, reason: "selection"})
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

	it("plans the frontend's connection lists with an empty orderBy, and still refuses a real order", () => {
		const query = (order: string) => `query ($ids: [UUID!]!, $space: UUID!) {
			entities(first: 25, filter: {id: {in: $ids}}) {
				id
				values(first: 100, filter: {spaceId: {is: $space}}, ${order}) { totalCount nodes { id propertyId spaceId text property { id name } } pageInfo { hasNextPage endCursor } }
				relations(first: 200, ${order}) { totalCount nodes { id spaceId typeId toEntityId } pageInfo { hasNextPage endCursor } }
			}
		}`
		const vars = {ids: ["073ff16b-e7fe-4591-a984-c325d629d92d"], space: "5093b6ab-1b48-4f11-91dd-31275b5413dd"}
		const options = {entitiesBatch: true, relationsTypeFilter: true, derivedFields: true}
		expect(analyzeLightEntities(parse(query("orderBy: []")), null, vars, options).ok).toBe(true)
		expect(analyzeLightEntities(parse(query("orderBy: null")), null, vars, options).ok).toBe(true)
		expect(analyzeLightEntities(parse(query("orderBy: [ID_ASC]")), null, vars, options)).toEqual({
			ok: false,
			reason: "args",
		})
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
			{data: {entity: {id: HEX, name: null, valuesList: [], relationsList: []}}},
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

	it("plans an orderBy as an uncompared order and refuses one it does not know", () => {
		const base = `spaceId: "${SPACE_A}", typeId: "${typeId}", first: 5`
		for (const order of [
			"CREATED_AT_DESC",
			"[CREATED_AT_DESC, ID_ASC]",
			"[RANKING_SCORE_DESC]",
			"UPDATED_AT_ASC",
		]) {
			const a = analyzeLightEntities(parse(withArgs(`${base}, orderBy: ${order}`)), null, {})
			expect(a.ok).toBe(true)
			if (a.ok) expect(a.plan.uncompared).toContain("order_completeness")
		}
		const viaVariable = analyzeLightEntities(
			parse(withArgs(`${base}, orderBy: $o`).replace("query", "query($o: [EntitiesOrderBy!])")),
			null,
			{o: ["CREATED_AT_ASC", "ID_ASC"]},
		)
		expect(viaVariable.ok).toBe(true)
		const plain = analyzeLightEntities(parse(withArgs(base)), null, {})
		expect(plain.ok && plain.plan.uncompared).not.toContain("order_completeness")
		for (const order of ["BACKLINKS_COUNT_DESC", "[]", "[ID_ASC, ID_ASC, ID_ASC, ID_ASC, ID_ASC]"])
			expect(analyzeLightEntities(parse(withArgs(`${base}, orderBy: ${order}`)), null, {})).toEqual({
				ok: false,
				reason: "args",
			})
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

	describe("widening a cross_space comparison (GEODB_CROSS_SPACE_HOLDERS_SHADOW)", () => {
		// The query names Space A, selects a relation row in A and neither spaceIds nor description. The Name is held in A
		// (the first comparison settles) or in B (loaded, outside the read: the first comparison is cross_space).
		const SPACE_B_HEX = "5093b6ab1b48f1111dd31275b5413ddb"
		const widened = (flag: boolean, nameIn: "A" | "B") => {
			const bare = clone(ENTITY)
			const own = new Set(nameIn === "A" ? [DESCRIPTION] : [NAME, DESCRIPTION])
			bare.values.nodes = bare.values.nodes.filter((v) => !own.has(conformingId("entity", v.property.id)))
			for (const r of [...bare.lightRels.nodes, ...bare.heavyRels.nodes]) r.spaceId = SPACE_A
			const entities = [bare]
			const full = geoOutput(bare)
			const named = full.split("\n").filter((line) => line.split("\t")[0] === NAME)
			const rest = `${full
				.split("\n")
				.filter((line) => ![NAME, DESCRIPTION].includes(line.split("\t")[0] as string))
				.join("\n")}`
			const outputs = {
				[geoSpace(SPACE_A)]: nameIn === "A" ? `${rest}\n${named.join("\n")}\n` : rest,
				[geoSpace(SPACE_B_HEX)]: nameIn === "B" ? `${named.join("\n")}\n` : "",
			}
			const s = setup(outputs, undefined, true)
			const deps = {...s.deps, allGeoSpaces: async () => Object.keys(outputs), crossSpaceHolders: flag}
			const query = FIXTURE.request.query
				.replace("spaceIds", "")
				.replace(" description", "")
				.replace('entity(id: "' + HEX + '")', `entities(spaceIds: {in: ["${SPACE_A}"]}, first: 5)`)
			return {
				s,
				run: () =>
					call(createPointReadShadows(deps as never).get("entities") as never, query, {}, {data: {entities}}),
			}
		}

		it("flag off: the comparison stays cross_space and nothing is read again", async () => {
			const w = widened(false, "B")
			await w.run()
			expect(uncompared().cross_space).toBe(1)
			expect(w.s.readCard).toHaveBeenCalledTimes(1)
			expect(w.s.readCardSpaces).not.toHaveBeenCalled()
		})

		it("flag on: the first comparison is cross_space, one more read across every Space settles it", async () => {
			const w = widened(true, "B")
			await w.run()
			expect(w.s.readCard).toHaveBeenCalledTimes(1)
			expect(w.s.readCardSpaces).toHaveBeenCalledTimes(1)
			expect(w.s.readCardSpaces.mock.calls[0]?.[0].geoSpaceIds).toEqual([
				geoSpace(SPACE_A),
				geoSpace(SPACE_B_HEX),
			])
			expect(uncompared().cross_space).toBeUndefined()
			expect(results()).toEqual({match: 1})
		})

		it("flag on: a comparison that settles on the first read is not read again", async () => {
			const w = widened(true, "A")
			await w.run()
			expect(w.s.readCard).toHaveBeenCalledTimes(1)
			expect(w.s.readCardSpaces).not.toHaveBeenCalled()
			expect(uncompared().cross_space).toBeUndefined()
			expect(results()).toEqual({match: 1})
		})
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

	it("compares the entities of an orderBy page one by one and leaves the order uncompared", async () => {
		const entities = list(2)
		const s = setup(outputsFor(entities))
		await call(
			s.points.get("entities") as never,
			withArgs(`spaceId: "${SPACE_A}", first: 5, orderBy: [CREATED_AT_DESC, ID_ASC]`),
			{},
			{data: {entities}},
		)
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({order_completeness: 1, sampled_rows: 1})
	})

	describe("the type scan page read (GEODB_TYPE_SCAN_SHADOW)", () => {
		const T = conformingId("entity", typeId)
		const scanSetup = (
			entities: Entity[],
			scan: (args: {geoSpaceIds: string[]; typeIds: string[]; first: number}) => Promise<string[]>,
			options: {flag?: boolean} = {},
		) => {
			const log = {info: vi.fn(), warn: vi.fn()}
			const typed = entities.map((e) => `${TYPES}\t${T}\tt\t${e.id}\t-\t-\t1\n`).join("")
			const outputs = {[geoSpace(SPACE_A)]: (outputsFor(entities)[geoSpace(SPACE_A)] ?? "") + typed}
			const readTypeScan = vi.fn(scan)
			const deps = {
				typeScan: options.flag !== false,
				multiSpace: true,
				geoDb: {readCard: askOf(outputs), readCardSpaces: askSpacesOf(outputs), readTypeScan},
				geoSpaceOf: async (space: string) => geoSpace(space),
				log,
			}
			return {log, readTypeScan, points: createPointReadShadows(deps)}
		}
		const q = (extra = "", first = 2, offset = 1) =>
			withArgs(`spaceId: "${SPACE_A}", typeId: "${typeId}", first: ${first}, offset: ${offset}${extra}`)
		const run = (s: ReturnType<typeof scanSetup>, window: Entity[], query: string) =>
			call(s.points.get("entities") as never, query, {}, {data: {entities: window}})
		const all = list(6)
		const ids = all.map((e) => conformingId("entity", e.id))

		it("asks first = offset + first and drops the first offset ids before comparing the sequence", async () => {
			const s = scanSetup(all.slice(0, 4), async () => ids)
			await run(s, all.slice(1, 3), q())
			expect(s.readTypeScan).toHaveBeenCalledWith({geoSpaceIds: [geoSpace(SPACE_A)], typeIds: [T], first: 3})
			expect(results()).toEqual({match: 1})
			expect(uncompared()).not.toHaveProperty("filter_completeness")
		})

		it("flags a window whose order differs from primary-key order", async () => {
			const s = scanSetup(all.slice(0, 4), async () => ids)
			await run(s, [all[2], all[1]] as Entity[], q())
			expect(results()).toEqual({mismatch: 1})
		})

		it("flags a window the offset moved: the page held the wrong rows", async () => {
			const s = scanSetup(all.slice(0, 4), async () => ids)
			await run(s, all.slice(0, 2), q())
			expect(results()).toEqual({mismatch: 1})
		})

		it("reads the scan once for a page with no offset, asking exactly first", async () => {
			const s = scanSetup(all.slice(0, 4), async () => ids)
			await run(s, all.slice(0, 2), q("", 2, 0).replace(", offset: 0", ""))
			expect(s.readTypeScan).toHaveBeenCalledWith(expect.objectContaining({first: 2}))
			expect(results()).toEqual({match: 1})
		})

		it("does not ask for a window past 4096: counted type_scan_window", async () => {
			const s = scanSetup(all.slice(0, 4), async () => ids)
			await run(s, all.slice(0, 2), q("", 100, 4000))
			expect(s.readTypeScan).not.toHaveBeenCalled()
			expect(results()).toEqual({unsupported_selection: 1})
			expect(unsupported()).toEqual({type_scan_window: 1})
		})

		it("asks the whole 4096 when the window ends on it", async () => {
			const s = scanSetup(all.slice(0, 4), async () => ids)
			await run(s, all.slice(0, 2), q("", 96, 4000))
			expect(s.readTypeScan).toHaveBeenCalledWith(expect.objectContaining({first: 4096}))
		})

		it("counts a serve that refuses the argument as unsupported, and the request is untouched", async () => {
			const s = scanSetup(all.slice(0, 4), async () => {
				throw new GeoDbError("client_failed", "refused: Request MalformedRequest at .args.types")
			})
			await expect(run(s, all.slice(1, 3), q())).resolves.toBeUndefined()
			expect(results()).toEqual({unsupported_selection: 1})
			expect(unsupported()).toEqual({type_scan_refused: 1})
			expect(s.log.warn).not.toHaveBeenCalledWith(
				expect.stringContaining("type scan read failed"),
				expect.anything(),
			)
		})

		it("counts any other failed read as failed", async () => {
			const s = scanSetup(all.slice(0, 4), async () => {
				throw new GeoDbError("timeout", "slow")
			})
			await run(s, all.slice(1, 3), q())
			expect(results()).toEqual({failed: 1})
		})

		it("reads nothing with the flag off, or with an orderBy, or with no Space argument", async () => {
			const off = scanSetup(all.slice(0, 4), async () => ids, {flag: false})
			await run(off, all.slice(1, 3), q())
			expect(off.readTypeScan).not.toHaveBeenCalled()
			const ordered = scanSetup(all.slice(0, 4), async () => ids)
			await run(ordered, all.slice(1, 3), q(", orderBy: ID_ASC"))
			const unscoped = scanSetup(all.slice(0, 4), async () => ids)
			await run(unscoped, all.slice(1, 3), withArgs(`typeId: "${typeId}", first: 2, offset: 1`))
			expect(ordered.readTypeScan).not.toHaveBeenCalled()
			expect(unscoped.readTypeScan).not.toHaveBeenCalled()
		})
	})

	describe("the ordered page read (GEODB_ORDER_PICK_SHADOW)", () => {
		const PAGE_ALL = [geoSpace(SPACE_A), geoSpace(SPACE_B_UUID)]
		/** More loaded Spaces than one line names: two lines of 1024 and 2. */
		const PAGE_MANY = [
			...PAGE_ALL,
			...Array.from({length: 1024}, (_, i) => geoSpace((i + 1).toString(16).padStart(32, "0"))),
		]
		const T = conformingId("entity", typeId)
		const instant = (seconds: number) => `instant:${seconds}.000000000`
		/** Geo rows of an entity page: one row of the order's property a (entity, seconds) pair, as geodb-client prints. */
		const pageRows = (property: string, keys: [string, number][]) =>
			keys.map(([id, seconds], at) => ({
				entity: conformingId("entity", id),
				prop: property,
				raw: instant(seconds),
				at,
			}))
		const pageSetup = (
			entities: Entity[],
			geoPage: {entity: string; prop: string; raw: string}[],
			options: {flag?: boolean; chunks?: boolean; pages?: {entity: string; prop: string; raw: string}[][]} = {},
		) => {
			const log = {info: vi.fn(), warn: vi.fn()}
			const typed = entities.map((e) => `${TYPES}\t${T}\tt\t${e.id}\t-\t-\t1\n`).join("")
			const outputs = {[geoSpace(SPACE_A)]: (outputsFor(entities)[geoSpace(SPACE_A)] ?? "") + typed}
			let line = 0
			const readEntitiesPage = vi.fn(async () => (options.pages ? (options.pages[line++] ?? []) : geoPage))
			const deps = {
				everySpaceChunks: options.chunks,
				multiSpace: true,
				orderPick: options.flag !== false,
				geoDb: {readCard: askOf(outputs), readCardSpaces: askSpacesOf(outputs), readEntitiesPage},
				geoSpaceOf: async (space: string) => geoSpace(space),
				allGeoSpaces: async () => (options.pages ? PAGE_MANY : PAGE_ALL),
				log,
			}
			return {log, readEntitiesPage, points: createPointReadShadows(deps)}
		}
		const query = (order: string, extra = `typeId: "${typeId}"`, first = 3) =>
			withArgs(`${extra}, first: ${first}, orderBy: ${order}`)
		const run = (s: ReturnType<typeof pageSetup>, entities: Entity[], q: string) =>
			call(s.points.get("entities") as never, q, {}, {data: {entities}})
		const created = (entities: Entity[], seconds: number[]) =>
			pageRows(
				CREATED_AT_PROPERTY,
				entities.map((e, i) => [e.id, seconds[i] as number]),
			)

		describe("more loaded Spaces than one line can name (GEODB_EVERY_SPACE_CHUNKS)", () => {
			// Two lines. Entity X is oldest in line 1 (created 100) but line 2 holds an older row of it (created 5).
			const [X, Y, Z] = list(3)
			const line1 = [...created([X, Y] as Entity[], [100, 50]), ...created([Z] as Entity[], [10])]
			const line2 = created([X] as Entity[], [5])

			it("leaves CREATED_AT_DESC uncompared (desc + least): a plain merge would be wrong", async () => {
				// Merged by the lines' own tops X would be placed by 5 and fall to the bottom of a desc page; the
				// reads cannot prove who is above it, so the page is left uncompared (`order_completeness`).
				const s = pageSetup([X, Y] as Entity[], [], {chunks: true, pages: [line1, line2]})
				await run(s, [X, Y] as Entity[], query("CREATED_AT_DESC", undefined, 2))
				expect(s.readEntitiesPage).not.toHaveBeenCalled()
				expect(uncompared()).toMatchObject({order_completeness: 1})
			})

			it("leaves the page uncompared with the flag off, even for an exact direction", async () => {
				const s = pageSetup([Z, X] as Entity[], [], {pages: [line1, line2]})
				await run(s, [Z, X] as Entity[], query("CREATED_AT_ASC", undefined, 2))
				expect(s.readEntitiesPage).not.toHaveBeenCalled()
				expect(uncompared()).toMatchObject({order_completeness: 1})
			})

			it("merges CREATED_AT_ASC (asc + least) by key: X is placed by its earliest row", async () => {
				const s = pageSetup([X, Z] as Entity[], [], {chunks: true, pages: [line1, line2]})
				await run(s, [X, Z] as Entity[], query("CREATED_AT_ASC", undefined, 2))
				expect(results()).toEqual({match: 1})
				expect(s.readEntitiesPage).toHaveBeenCalledTimes(2)
			})

			it("flags the merge when PostGraphile placed X by its later row", async () => {
				const s = pageSetup([Z, X] as Entity[], [], {chunks: true, pages: [line1, line2]})
				await run(s, [Z, X] as Entity[], query("CREATED_AT_ASC", undefined, 2))
				expect(results()).toEqual({mismatch: 1})
			})

			it("the desc + least counterexample: merging each line's top-2 puts A and B on the page; the truth is C and D", () => {
				// Key = least over the lines. A and B top line 1 (100, 90) but line 2 holds them at 1; C (80) is
				// third in line 1 and D (50) tops line 2. The true desc top-2 by global key is C, D.
				const nanos = (n: number) => BigInt(n)
				const line1: [string, bigint | null][] = [
					["A", nanos(100)],
					["B", nanos(90)],
				]
				const line2: [string, bigint | null][] = [
					["D", nanos(50)],
					["E", nanos(40)],
				]
				const naive = mergePages([line1, line2], "desc", "least")
					.slice(0, 2)
					.map(([id]) => id)
				expect(naive).toEqual(["A", "B"])
				const truth = [
					["A", 1],
					["B", 1],
					["C", 80],
					["D", 50],
					["E", 40],
				] as [string, number][]
				expect(
					truth
						.sort((a, b) => b[1] - a[1])
						.slice(0, 2)
						.map(([id]) => id),
				).toEqual(["C", "D"])
				expect(mergeIsExact("desc", "least")).toBe(false)
			})

			it("the asc + least merge is exact on the same data shape", () => {
				const nanos = (n: number) => BigInt(n)
				const line1: [string, bigint | null][] = [
					["A", nanos(1)],
					["B", nanos(2)],
				]
				const line2: [string, bigint | null][] = [
					["B", nanos(0)],
					["D", nanos(5)],
				]
				expect(
					mergePages([line1, line2], "asc", "least")
						.slice(0, 2)
						.map(([id]) => id),
				).toEqual(["B", "A"])
			})

			it("exposes which directions merge exactly", () => {
				expect(mergeIsExact("asc", "least")).toBe(true)
				expect(mergeIsExact("desc", "greatest")).toBe(true)
				expect(mergeIsExact("desc", "least")).toBe(false)
				expect(mergeIsExact("asc", "greatest")).toBe(false)
			})
		})

		it("plans a single CREATED_AT or UPDATED_AT key and nothing else", () => {
			const plan = (order: string, extra?: string) => {
				const a = analyzeLightEntities(parse(query(order, extra)), null, {})
				return a.ok ? a.plan.order : "refused"
			}
			expect(plan("CREATED_AT_DESC")).toEqual({property: "createdAt", direction: "desc"})
			expect(plan("[UPDATED_AT_ASC]")).toEqual({property: "updatedAt", direction: "asc"})
			expect(plan("[CREATED_AT_DESC, ID_ASC]")).toBeNull()
			expect(plan("RANKING_SCORE_DESC")).toBeNull()
			expect(plan("ID_ASC")).toBeNull()
		})

		it("reads nothing with the switch off", async () => {
			const entities = list(2)
			const s = pageSetup(entities, created(entities, [30, 20, 10]), {flag: false})
			await run(s, entities, query("CREATED_AT_DESC"))
			expect(s.readEntitiesPage).not.toHaveBeenCalled()
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({order_completeness: 1})
		})

		it("sends the pick and one row more than the page, and compares a strict order as a sequence", async () => {
			const entities = list(3)
			const keys = [
				...created(entities, [30, 20, 10]),
				...pageRows(CREATED_AT_PROPERTY, [[list(4)[3]?.id as string, 5]]),
			]
			const s = pageSetup(entities, keys)
			await run(s, entities, query("CREATED_AT_DESC"))
			expect(s.readEntitiesPage).toHaveBeenCalledWith({
				geoSpaceIds: PAGE_ALL,
				typeId: T,
				field: CREATED_AT_PROPERTY,
				direction: "desc",
				pick: "least",
				first: 4,
			})
			expect(results()).toEqual({match: 1})
			expect(uncompared()).not.toHaveProperty("order_completeness")
			expect(uncompared()).not.toHaveProperty("filter_completeness")
			expect(uncompared()).not.toHaveProperty("order_ties")
		})

		it("places an updatedAt page by its greatest value, ascending or not", async () => {
			const entities = list(2)
			const s = pageSetup(
				entities,
				pageRows(
					UPDATED_AT_PROPERTY,
					entities.map((e, i) => [e.id, 10 + i]),
				),
			)
			await run(s, entities, query("UPDATED_AT_ASC", undefined, 2))
			expect(s.readEntitiesPage).toHaveBeenCalledWith(
				expect.objectContaining({field: UPDATED_AT_PROPERTY, direction: "asc", pick: "greatest", first: 3}),
			)
			expect(results()).toEqual({match: 1})
		})

		it("flags a page whose order differs", async () => {
			const entities = list(3)
			const swapped = [entities[1], entities[0], entities[2]] as Entity[]
			const s = pageSetup(entities, created(swapped, [30, 20, 10]))
			await run(s, entities, query("CREATED_AT_DESC"))
			expect(results()).toEqual({mismatch: 1})
			expect(s.log.warn).toHaveBeenCalledWith(
				expect.stringContaining("mismatch"),
				expect.objectContaining({first: undefined}),
			)
		})

		it("flags a page with an entity geo-db does not place there", async () => {
			const entities = list(3)
			const other = list(4)[3] as Entity
			const s = pageSetup(entities, created([entities[0], entities[1], other] as Entity[], [30, 20, 10]))
			await run(s, entities, query("CREATED_AT_DESC"))
			expect(results()).toEqual({mismatch: 1})
		})

		it("compares as a set when keys tie inside the page, and says so", async () => {
			const entities = list(3)
			const geoOrder = [entities[1], entities[0], entities[2]] as Entity[]
			const s = pageSetup(entities, created(geoOrder, [30, 30, 10]).concat(created(list(4).slice(3), [5])))
			await run(s, entities, query("CREATED_AT_DESC"))
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({order_ties: 1})
			expect(uncompared()).not.toHaveProperty("order_completeness")
		})

		it("still flags a different member of a page that ties inside", async () => {
			const entities = list(3)
			const other = list(4)[3] as Entity
			const geoOrder = [entities[0], other, entities[2]] as Entity[]
			const s = pageSetup(entities, created(geoOrder, [30, 30, 10]))
			await run(s, entities, query("CREATED_AT_DESC"))
			expect(results()).toEqual({mismatch: 1})
		})

		it("compares nothing of the page when the cut splits a tie, and keeps the order uncompared", async () => {
			const entities = list(3)
			const next = list(4)[3] as Entity
			const s = pageSetup(entities, created([...entities, next], [30, 20, 10, 10]))
			await run(s, entities, query("CREATED_AT_DESC"))
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({order_completeness: 1, filter_completeness: 1, order_ties: 1})
		})

		it("does not call a short page's missing next entity a tie", async () => {
			const entities = list(2)
			const s = pageSetup(entities, created(entities, [30, 20]))
			await run(s, entities, query("CREATED_AT_DESC"))
			expect(results()).toEqual({match: 1})
			expect(uncompared()).not.toHaveProperty("order_ties")
		})

		it("leaves other shapes uncompared and reads nothing: a Space scope, a second key, an offset, no first", async () => {
			const entities = list(2)
			for (const q of [
				query("CREATED_AT_DESC", `typeId: "${typeId}", spaceId: "${SPACE_A}"`),
				query("[CREATED_AT_DESC, ID_ASC]"),
				withArgs(`typeId: "${typeId}", first: 3, offset: 3, orderBy: CREATED_AT_DESC`),
				withArgs(`typeId: "${typeId}", orderBy: CREATED_AT_DESC`),
				withArgs(`first: 3, orderBy: CREATED_AT_DESC`),
			]) {
				__resetGatewayMetricsForTests()
				const s = pageSetup(entities, created(entities, [30, 20]))
				await run(s, entities, q)
				expect(s.readEntitiesPage).not.toHaveBeenCalled()
				expect(uncompared()).toMatchObject({order_completeness: 1})
			}
		})

		it("leaves the order uncompared when the page cannot be read", async () => {
			const entities = list(2)
			const s = pageSetup(entities, [])
			s.readEntitiesPage.mockRejectedValueOnce(new Error("refused"))
			await run(s, entities, query("CREATED_AT_DESC"))
			expect(s.log.warn).toHaveBeenCalledWith(
				expect.stringContaining("ordered page read failed"),
				expect.anything(),
			)
			expect(uncompared()).toMatchObject({order_completeness: 1})
		})
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
		options: {
			flag?: boolean
			budget?: ReturnType<typeof createReadBudget>
			spaces?: string[]
			chunks?: boolean
			holders?: {gate: HoldersGate; readHolders: ReturnType<typeof vi.fn>}
		} = {},
	) {
		const log = {info: vi.fn(), warn: vi.fn()}
		const readCard = askOf(outputs)
		const readCardSpaces = askSpacesOf(outputs)
		const allGeoSpaces = vi.fn(async () => options.spaces ?? ALL)
		const deps = {
			everySpaceChunks: options.chunks,
			multiSpace: true,
			geoDb: {readCard, readCardSpaces, ...(options.holders ? {readHolders: options.holders.readHolders} : {})},
			geoSpaceOf: async (space: string) => geoSpace(space),
			log,
			...(options.holders ? {holders: options.holders.gate} : {}),
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

	describe("types { name } with no id", () => {
		const namesOnly = (name: string): Captured => ({
			request: {
				query: "query T($id: UUID!) { entity(id: $id) { types { name } } }",
				variables: {id: EVERY.entityTypes.request.variables.id},
			},
			response: {data: {entity: {types: [{name}]}}},
		})

		it("matches when geo-db's Type carries the same name", async () => {
			const s = everySetup({[geoSpace(SPACE_A)]: typeRow(HEX, NEWS_STORY, "News story")})
			await run(s, namesOnly("News story"))
			expect(results()).toEqual({match: 1})
		})

		it("leaves a different name inconclusive", async () => {
			const s = everySetup({[geoSpace(SPACE_A)]: typeRow(HEX, NEWS_STORY, "Op-ed")})
			await run(s, namesOnly("News story"))
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({cross_space: 1})
		})

		it("flags a Type geo-db holds where PostGraphile lists none", async () => {
			const s = everySetup({[geoSpace(SPACE_A)]: typeRow(HEX, NEWS_STORY, "News story")})
			await run(s, {...namesOnly("x"), response: {data: {entity: {types: []}}}})
			expect(results()).toEqual({mismatch: 1})
		})
	})

	describe("entity(id) { spaceIds } alone", () => {
		const spaceIdsOnly = (ids: string[]): Captured => ({
			request: {
				query: "query ProfileSpaceIdForEntity($entityId: UUID!) { entity(id: $entityId) { spaceIds } }",
				variables: {entityId: EVERY.entityTypes.request.variables.id},
			},
			response: {data: {entity: {spaceIds: ids}}},
		})

		it("plans as an every-Space read", () => {
			const captured = spaceIdsOnly([SPACE_A])
			const analysis = analyzeLightEntity(parse(captured.request.query), null, captured.request.variables)
			expect(analysis.ok && analysis.plan.everySpace).toBe(true)
		})

		it("matches when every Space geo-db holds the entity in is one PostGraphile names", async () => {
			const s = everySetup({[geoSpace(SPACE_A)]: typeRow(HEX, NEWS_STORY, "News story")})
			await run(s, spaceIdsOnly([SPACE_A, SPACE_B]))
			expect(results()).toEqual({match: 1})
		})

		it("flags a Space geo-db holds the entity in that PostGraphile does not name", async () => {
			const s = everySetup({[geoSpace(SPACE_A)]: typeRow(HEX, NEWS_STORY, "News story")})
			await run(s, spaceIdsOnly([SPACE_B]))
			expect(results()).toEqual({mismatch: 1})
		})
	})

	describe("an answer that names no Space and holds no row", () => {
		const bare = (response: Captured["response"]): Captured => ({
			request: {
				query: `query P($id: UUID!, $spaceId: UUID) { entity(id: $id) { id name spaceIds valuesList(first: 1000, filter: {spaceId: {is: $spaceId}}) { propertyId text integer float boolean datetime date decimal } relationsList(first: 1000, filter: {spaceId: {is: $spaceId}}) { typeId toEntityId } } }`,
				variables: {id: EVERY.entityTypes.request.variables.id, spaceId: null},
			},
			response,
		})
		const empty = {
			data: {
				entity: {
					id: EVERY.entityTypes.request.variables.id,
					name: null,
					spaceIds: [],
					valuesList: [],
					relationsList: [],
				},
			},
		}

		it("is read across every Space and matches when geo-db holds nothing either", async () => {
			const s = everySetup({})
			await run(s, bare(empty))
			expect(unsupported()).toEqual({})
			expect(results()).toEqual({match: 1})
			expect(s.readCardSpaces).toHaveBeenCalledTimes(1)
		})

		it("flags a name geo-db holds where PostGraphile has none", async () => {
			const s = everySetup({[geoSpace(SPACE_A)]: typeRow(HEX, NEWS_STORY, "News story")})
			await run(s, bare(empty))
			expect(results()).toEqual({mismatch: 1})
		})

		it("stays no_space while the every-Space flag is off", async () => {
			const s = everySetup({}, {flag: false})
			await run(s, bare(empty))
			expect(unsupported()).toEqual({no_space: 1})
		})
	})

	it("accepts spacesByTopicIdConnection in an entities selection, uncompared", () => {
		const query = `{ entities(filter: {id: {in: ["${EVERY.entityTypes.request.variables.id}"]}}) { id name spacesByTopicIdConnection(first: 20) { totalCount nodes { id members { totalCount } } } } }`
		const analysis = analyzeLightEntities(parse(query), null, {})
		expect(analysis.ok).toBe(true)
		expect(analysis.ok && analysis.plan.uncompared).toContain("topic_spaces")
	})

	describe("more loaded Spaces than one line can name (GEODB_EVERY_SPACE_CHUNKS)", () => {
		const filler = Array.from({length: 1100}, (_, i) => geoSpace(`${(i + 1).toString(16).padStart(32, "0")}`))
		const many = [...ALL, ...filler]
		const rows = () => ({[geoSpace(SPACE_C)]: geoOutput(ENTITY) + typeRow(HEX, NEWS_STORY, "News story")})

		it("refuses the read as many_spaces while the flag is off", async () => {
			const s = everySetup(rows(), {spaces: many})
			await run(s, EVERY.entityTypes)
			expect(unsupported()).toEqual({many_spaces: 1})
			expect(s.readCardSpaces).not.toHaveBeenCalled()
		})

		it("reads them in lines of at most 1024 and compares the concatenated rows", async () => {
			const s = everySetup(rows(), {spaces: many, chunks: true})
			await run(s, EVERY.entityTypes)
			expect(results()).toEqual({match: 1})
			expect(s.readCardSpaces).toHaveBeenCalledTimes(2)
			const sizes = s.readCardSpaces.mock.calls.map((c) => c[0].geoSpaceIds.length)
			expect(sizes).toEqual([1024, many.length - 1024])
		})
	})

	describe("the holders lookup (GEODB_HOLDERS_LOOKUP)", () => {
		const rows = () => ({[geoSpace(SPACE_C)]: geoOutput(ENTITY) + typeRow(HEX, NEWS_STORY, "News story")})
		const holdersOf = (answer: (args: {geoSpaceIds: string[]; entityId: string}) => Promise<string[]>) => ({
			gate: createHoldersGate(),
			readHolders: vi.fn(answer),
		})

		it("builds the holders document: the entity's id and `.holders = true`", () => {
			expect(holdersDocument(HEX)).toBe(
				`.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .id = "${HEX}", .holders = true } } } }`,
			)
		})

		it("reads the card only in the Spaces that hold the entity", async () => {
			const holders = holdersOf(async () => [geoSpace(SPACE_C)])
			const s = everySetup(rows(), {holders})
			await run(s, EVERY.entityTypes)
			expect(results()).toEqual({match: 1})
			expect(holders.readHolders).toHaveBeenCalledTimes(1)
			expect(holders.readHolders.mock.calls[0]?.[0]).toMatchObject({geoSpaceIds: ALL})
			expect(s.readCardSpaces).toHaveBeenCalledTimes(1)
			expect(s.readCardSpaces.mock.calls[0]?.[0]).toMatchObject({geoSpaceIds: [geoSpace(SPACE_C)], names: true})
		})

		it("reads no card where no Space holds the entity", async () => {
			const holders = holdersOf(async () => [])
			const s = everySetup({}, {holders})
			await run(s, EVERY.entityExists)
			expect(holders.readHolders).toHaveBeenCalledTimes(1)
			expect(s.readCardSpaces).not.toHaveBeenCalled()
			expect(s.readCard).not.toHaveBeenCalled()
			expect(s.log.warn).not.toHaveBeenCalled()
		})

		it("batches the holders in lines of at most 1024 Spaces", async () => {
			const filler = Array.from({length: 1100}, (_, i) => geoSpace(`${(i + 1).toString(16).padStart(32, "0")}`))
			const many = [...ALL, ...filler]
			const holders = holdersOf(async ({geoSpaceIds}) => geoSpaceIds.filter((g) => g === geoSpace(SPACE_C)))
			const s = everySetup(rows(), {spaces: many, chunks: true, holders})
			await run(s, EVERY.entityTypes)
			expect(results()).toEqual({match: 1})
			expect(holders.readHolders.mock.calls.map((c) => c[0].geoSpaceIds.length)).toEqual([
				1024,
				many.length - 1024,
			])
			expect(s.readCardSpaces).toHaveBeenCalledTimes(1)
			expect(s.readCardSpaces.mock.calls[0]?.[0].geoSpaceIds).toEqual([geoSpace(SPACE_C)])
		})

		it("falls back to the card across every Space when the store refuses the argument, then stays off", async () => {
			const holders = holdersOf(async () => {
				throw new GeoDbError("client_failed", "refused: DefiniteRefusal MalformedRequest span 110..118")
			})
			const s = everySetup(rows(), {holders})
			await run(s, EVERY.entityTypes)
			await run(s, EVERY.entityTypes)
			expect(results()).toEqual({match: 2})
			expect(holders.readHolders).toHaveBeenCalledTimes(1)
			expect(s.readCardSpaces).toHaveBeenCalledTimes(2)
			expect(s.readCardSpaces.mock.calls[0]?.[0]).toMatchObject({geoSpaceIds: ALL})
		})

		it("counts any other failed holders read as failed, with no fallback and no cooldown", async () => {
			for (const failure of [
				new GeoDbError("client_failed", "TransportClosed"),
				new GeoDbError("client_failed", "refused: DefiniteRefusal NotFoundOrRestricted VersionNotFound"),
				new GeoDbError("client_failed", "geodb-client pipe exited (code 1)"),
				new GeoDbError("overloaded", "geo-db pool full"),
				new Error("boom"),
			]) {
				const holders = holdersOf(async () => {
					throw failure
				})
				const s = everySetup(rows(), {holders})
				await run(s, EVERY.entityTypes)
				expect(s.readCardSpaces).not.toHaveBeenCalled()
				expect(holders.gate.allowed()).toBe(true)
			}
			expect(results()).toEqual({failed: 5})
		})

		it("tries the lookup again once the cooldown has passed", () => {
			let now = 0
			const gate = createHoldersGate({cooldownMs: 1000, now: () => now})
			expect(gate.allowed()).toBe(true)
			gate.refused()
			expect(gate.allowed()).toBe(false)
			now = 1000
			expect(gate.allowed()).toBe(true)
		})

		it("falls back when the store answers a Space the line did not name", async () => {
			const holders = holdersOf(async () => [geoSpace(SPACE_D)])
			const s = everySetup(rows(), {holders})
			await run(s, EVERY.entityTypes)
			expect(results()).toEqual({match: 1})
			expect(s.readCardSpaces.mock.calls[0]?.[0]).toMatchObject({geoSpaceIds: ALL})
		})

		it("counts a timed-out holders read as failed, not a refusal to read around", async () => {
			const holders = holdersOf(async () => {
				throw new GeoDbError("timeout", "geo-db read exceeded 500 ms")
			})
			const s = everySetup(rows(), {holders})
			await run(s, EVERY.entityTypes)
			expect(results()).toEqual({failed: 1})
			expect(s.readCardSpaces).not.toHaveBeenCalled()
			expect(holders.gate.allowed()).toBe(true)
		})

		it("reads the card across every Space with the flag off (no gate)", async () => {
			const s = everySetup(rows())
			await run(s, EVERY.entityTypes)
			expect(s.readCardSpaces.mock.calls[0]?.[0]).toMatchObject({geoSpaceIds: ALL})
		})
	})

	describe("the lines of one every-Space read go out together", () => {
		const filler = Array.from({length: 1100}, (_, i) => geoSpace(`${(i + 1).toString(16).padStart(32, "0")}`))
		const many = [...ALL, ...filler]
		const rows = () => ({[geoSpace(SPACE_C)]: geoOutput(ENTITY) + typeRow(HEX, NEWS_STORY, "News story")})

		it("issues both lines before either answers, and matches when the first answers last", async () => {
			const s = everySetup(rows(), {spaces: many, chunks: true})
			const answer = s.readCardSpaces.getMockImplementation() as (a: never) => Promise<unknown>
			let inFlight = 0
			let widest = 0
			s.readCardSpaces.mockImplementation((async (a: {geoSpaceIds: string[]}) => {
				inFlight += 1
				widest = Math.max(widest, inFlight)
				await new Promise((resolve) => setTimeout(resolve, a.geoSpaceIds.length === 1024 ? 20 : 1))
				inFlight -= 1
				return answer(a as never)
			}) as never)
			await run(s, EVERY.entityTypes)
			expect(widest).toBe(2)
			expect(results()).toEqual({match: 1})
		})

		it("is throttled whole when the budget cannot give both lines", async () => {
			const budget = createReadBudget({perSecond: 0, burst: 1})
			const s = everySetup(rows(), {spaces: many, chunks: true, budget})
			await run(s, EVERY.entityTypes)
			expect(unsupported()).toEqual({throttled: 1})
			expect(s.readCardSpaces).not.toHaveBeenCalled()
		})
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
		options: {
			flag?: boolean
			budget?: ReturnType<typeof createReadBudget>
			typeFilter?: boolean
			derivedFields?: boolean
		} = {},
	) {
		const log = {info: vi.fn(), warn: vi.fn()}
		const readCard = askOf(outputs)
		const readCardSpaces = askSpacesOf(outputs)
		const deps = {
			relationsTypeFilter: options.typeFilter === true,
			derivedFields: options.derivedFields,
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

	// A type-wide listing: `entities(filter: {typeIds}) { id relations(first, filter: {typeId: {in}}) { nodes { toEntity { id name } } } }`.
	describe("a typeId-filtered relations list that selects no type", () => {
		const typeId = "e550fe517e904b2c8fffdf13408f5634"
		const edgeType = "0a1b2c3d4e5f4a6b8c7d8e9f0a1b2c3d"
		const otherType = "1a1b2c3d4e5f4a6b8c7d8e9f0a1b2c3e"
		const target = "99999999999949998999999999999999"
		const query = `query { entities(first: 10, filter: {typeIds: {anyEqualTo: "${typeId}"}}) { id relations(first: 50, filter: {typeId: {in: ["${edgeType}", "${otherType}"]}}) { nodes { toEntity { id name } } } } }`
		const body = (name: string) => ({
			data: {entities: [{id: FIRST.id, relations: {nodes: [{toEntity: {id: target, name}}]}}]},
		})
		const typed = `${TYPES}\t${conformingId("entity", typeId)}\ts1\t${FIRST.id}\t-\t-\t1\n`
		const edge = (type: string) =>
			`${conformingId("entity", type)}\t${conformingId("entity", target)}\ts2\t${FIRST.id}\t-\tTarget\t1\n`
		const callQuery = (s: ReturnType<typeof namesSetup>, text: string, payload: unknown) =>
			call(s.points.get("entities") as never, text, {}, payload)

		it("stays unsupported while the switch is off", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: typed + edge(edgeType)})
			await callQuery(s, query, body("Target"))
			expect(results()).toEqual({unsupported_selection: 1})
			expect(unsupported()).toEqual({selection: 1})
		})

		it("matches a target held under one of the filter's Types", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: typed + edge(otherType)}, {typeFilter: true})
			await callQuery(s, query, body("Target"))
			expect(results()).toEqual({match: 1})
		})

		// The row names its Space (PostGraphile's id) but the list's selection leaves the entity's Spaces unknown, so the
		// card is read across every Space and keyed by geo-db's Space ids: the row's Space is looked up by its geo-db id.
		describe("a row that names its Space, read across every Space", () => {
			const spaced = `query { entities(first: 10, filter: {typeIds: {anyEqualTo: "${typeId}"}}) { id relations(first: 50, filter: {typeId: {in: ["${edgeType}", "${otherType}"]}}) { nodes { spaceId toEntity { id name } } } } }`
			const spacedBody = (space: string) => ({
				data: {
					entities: [
						{id: FIRST.id, relations: {nodes: [{spaceId: space, toEntity: {id: target, name: "Target"}}]}},
					],
				},
			})

			it("matches a target held in the row's own Space", async () => {
				const s = namesSetup({[geoSpace(SPACE_B)]: typed + edge(otherType)}, {typeFilter: true})
				await callQuery(s, spaced, spacedBody(SPACE_B))
				expect(results()).toEqual({match: 1})
			})

			it("flags a target geo-db holds only in another Space than the row's", async () => {
				const s = namesSetup({[geoSpace(SPACE_C)]: typed + edge(otherType)}, {typeFilter: true})
				await callQuery(s, spaced, spacedBody(SPACE_B))
				expect(results()).toEqual({mismatch: 1})
			})
		})

		it("flags a target geo-db holds under none of the filter's Types", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: typed + edge(typeId)}, {typeFilter: true})
			await callQuery(s, query, body("Target"))
			expect(results()).toEqual({mismatch: 1})
		})

		it("flags a target geo-db does not hold at all", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: typed}, {typeFilter: true})
			await callQuery(s, query, body("Target"))
			expect(results()).toEqual({mismatch: 1})
		})

		it("still refuses a type filter it cannot read", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: typed + edge(edgeType)}, {typeFilter: true})
			await callQuery(s, query.replace(`in: ["${edgeType}", "${otherType}"]`, "in: []"), {data: {entities: []}})
			await callQuery(
				s,
				query.replace(`typeId: {in: ["${edgeType}", "${otherType}"]}`, `spaceId: {is: "${SPACE_B}"}`),
				body("Target"),
			)
			expect(results()).toEqual({unsupported_selection: 2})
		})
	})

	// The nested card read: `entities { spaceIds relationsList { typeId toEntity { valuesList } } }`, the target not selected by id.
	describe("a relations list whose target is selected without its id", () => {
		const typeId = "e550fe517e904b2c8fffdf13408f5634"
		const edgeType = "0a1b2c3d4e5f4a6b8c7d8e9f0a1b2c3d"
		const target = "99999999999949998999999999999999"
		const query = `query { entities(first: 10, filter: {typeIds: {anyEqualTo: "${typeId}"}}) { id spaceIds relationsList(filter: {spaceId: {in: ["${SPACE_B}"]}}) { typeId toEntity { valuesList { propertyId text } } } } }`
		const body = (type: string) => ({
			data: {
				entities: [
					{
						id: FIRST.id,
						spaceIds: [SPACE_B],
						relationsList: [{typeId: type, toEntity: {valuesList: [{propertyId: edgeType, text: "x"}]}}],
					},
				],
			},
		})
		const typed = `${TYPES}\t${conformingId("entity", typeId)}\ts1\t${FIRST.id}\t-\t-\t1\n`
		const edge = `${conformingId("entity", edgeType)}\t${conformingId("entity", target)}\ts2\t${FIRST.id}\t-\tTarget\t1\n`
		const callQuery = (s: ReturnType<typeof namesSetup>, payload: unknown) =>
			call(s.points.get("entities") as never, query, {}, payload)

		it("matches when geo-db holds a relation of the row's type", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: typed + edge})
			await callQuery(s, body(edgeType))
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({nested_entity: 1})
		})

		it("flags a relation type geo-db does not hold", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: typed + edge})
			await callQuery(s, body(typeId))
			expect(results()).toEqual({mismatch: 1})
		})
	})

	// The target's own values: `relationsList { typeId toEntity { id valuesList } }`, read as one batch of the targets' cards.
	describe("a relation target's own values", () => {
		const typeId = "e550fe517e904b2c8fffdf13408f5634"
		const edgeType = "0a1b2c3d4e5f4a6b8c7d8e9f0a1b2c3d"
		const valueProp = "1a1b2c3d4e5f4a6b8c7d8e9f0a1b2c3e"
		const target = "99999999999949998999999999999999"
		const query = `query { entities(first: 10, filter: {typeIds: {anyEqualTo: "${typeId}"}}) { id spaceIds relationsList(filter: {spaceId: {in: ["${SPACE_B}"]}}) { typeId toEntity { id valuesList { propertyId text } } } } }`
		const body = (text: string) => ({
			data: {
				entities: [
					{
						id: FIRST.id,
						spaceIds: [SPACE_B],
						relationsList: [
							{
								typeId: edgeType,
								toEntity: {id: target, valuesList: [{propertyId: valueProp, text}]},
							},
						],
					},
				],
			},
		})
		const typed = `${TYPES}\t${conformingId("entity", typeId)}\ts1\t${FIRST.id}\t-\t-\t1\n`
		const edge = `${conformingId("entity", edgeType)}\t${conformingId("entity", target)}\ts2\t${FIRST.id}\t-\tTarget\t1\n`
		const setup = (held: string | null, on = true) => {
			const log = {info: vi.fn(), warn: vi.fn()}
			const outputs = {[geoSpace(SPACE_B)]: typed + edge}
			const readCards = vi.fn(async (args: {entityIds: string[]}) =>
				held === null ? [] : [{prop: valueProp, raw: held, entity: args.entityIds[0] as string}],
			)
			const deps = {
				nestedTargetValues: on,
				multiSpace: true,
				geoDb: {readCard: askOf(outputs), readCardSpaces: askSpacesOf(outputs), readCards},
				geoSpaceOf: async (space: string) => geoSpace(space),
				log,
			}
			return {readCards, points: createPointReadShadows(deps)}
		}
		const callQuery = (s: ReturnType<typeof setup>, payload: unknown) =>
			call(s.points.get("entities") as never, query, {}, payload)

		it("matches a target whose values geo-db holds, in one batch read", async () => {
			const s = setup("x")
			await callQuery(s, body("x"))
			expect(results()).toEqual({match: 1})
			expect(s.readCards).toHaveBeenCalledTimes(1)
			expect(s.readCards.mock.calls[0]?.[0].entityIds).toEqual([conformingId("entity", target)])
		})

		it("flags a target value geo-db does not hold", async () => {
			await callQuery(setup("y"), body("x"))
			await callQuery(setup(null), body("x"))
			expect(results()).toEqual({mismatch: 2})
		})

		it("reads no target card with the switch off", async () => {
			const s = setup("y", false)
			await callQuery(s, body("x"))
			expect(results()).toEqual({match: 1})
			expect(s.readCards).not.toHaveBeenCalled()
		})
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

	// The typeahead: `entities(typeId, first, filter: {values: {some: {propertyId, text: {includesInsensitive}}}})`.
	describe("the vote scores PostGraphile computes (GEODB_DERIVED_FIELDS_SHADOW)", () => {
		const scores = "id name spaceIds upvotes downvotes agrees disagrees commenters commentScore"
		const query = `query Q($ids: [UUID!]) { entities(filter: {id: {in: $ids}}) { ${scores} } }`
		const row = {
			id: FIRST.id,
			name: FIRST.name,
			spaceIds: [SPACE_A],
			upvotes: 1,
			downvotes: 0,
			agrees: 0,
			disagrees: 0,
			commenters: 0,
			commentScore: 0,
		}
		const ask = (derivedFields: boolean) => {
			const s = namesSetup({[geoSpace(SPACE_A)]: nameRow(FIRST.id, FIRST.name)}, {derivedFields})
			return call(s.points.get("entities") as never, query, {ids: [FIRST.id]}, {data: {entities: [row]}})
		}

		it("refuses the selection while the flag is off", async () => {
			await ask(false)
			expect(results()).toEqual({unsupported_selection: 1})
		})

		it("compares the rest of the selection and counts the scores uncompared when the flag is on", async () => {
			await ask(true)
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({derived_fields: 1})
		})

		it("still flags a wrong name beside the scores", async () => {
			const s = namesSetup({[geoSpace(SPACE_A)]: nameRow(FIRST.id, "Other")}, {derivedFields: true})
			await call(
				s.points.get("entities") as never,
				query,
				{ids: [FIRST.id]},
				{data: {entities: [{...row, name: "Ada"}]}},
			)
			expect(results()).toEqual({mismatch: 1})
		})
	})

	describe("the typeahead and the selections it shares with id lists", () => {
		const typeId = "e550fe517e904b2c8fffdf13408f5634"
		const typeahead = (selection: string) =>
			`query { entities(typeId: "${typeId}", first: 10, filter: {values: {some: {propertyId: {is: "a126ca53-0c8e-48d5-b888-82c734c38935"}, text: {includesInsensitive: "ada"}}}}) { ${selection} } }`
		const typed = `${TYPES}\t${conformingId("entity", typeId)}\ts1\t${FIRST.id}\t-\t-\t1\n`
		const callQuery = (s: ReturnType<typeof namesSetup>, query: string, body: unknown) =>
			call(s.points.get("entities") as never, query, {}, body)

		it("compares the filter's type and the name, leaving which entities matched the text uncompared", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: typed + nameRow(FIRST.id, "Ada")})
			await callQuery(s, typeahead("id name"), {data: {entities: [{id: FIRST.id, name: "Ada"}]}})
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({filter_completeness: 1})
		})

		it("flags a returned entity that geo-db does not hold with the promised type", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: nameRow(FIRST.id, "Ada")})
			await callQuery(s, typeahead("id name"), {data: {entities: [{id: FIRST.id, name: "Ada"}]}})
			expect(results()).toEqual({mismatch: 1})
		})

		it("leaves a differing name inconclusive: the name may come from a Space geo-db did not load", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: typed + nameRow(FIRST.id, "Bob")})
			await callQuery(s, typeahead("id name"), {data: {entities: [{id: FIRST.id, name: "Ada"}]}})
			expect(results()).toEqual({match: 1})
		})

		it("compares typeIds as the entity's Types statements", async () => {
			const s = namesSetup({[geoSpace(SPACE_B)]: typed + nameRow(FIRST.id, "Ada")})
			const query = typeahead("id name description typeIds spaceIds updatedAt")
			const row = {
				id: FIRST.id,
				name: "Ada",
				description: null,
				typeIds: [typeId],
				spaceIds: [SPACE_B],
				updatedAt: "2026-01-01T00:00:00Z",
			}
			await callQuery(s, query, {data: {entities: [row]}})
			expect(results()).toEqual({match: 1})
			// A type PostGraphile lists that geo-db never held for the entity is a difference.
			const lacking = namesSetup({[geoSpace(SPACE_B)]: nameRow(FIRST.id, "Ada")})
			await callQuery(lacking, query, {data: {entities: [row]}})
			expect(results()).toMatchObject({mismatch: 1})
		})

		it("compares typeIds on an id list", async () => {
			const query = `query { entities(filter: {id: {in: ["${FIRST.id}"]}}, first: 1) { id typeIds } }`
			const s = namesSetup({[geoSpace(SPACE_C)]: typed})
			await callQuery(s, query, {data: {entities: [{id: FIRST.id, typeIds: [typeId]}]}})
			expect(results()).toEqual({match: 1})
		})

		it("refuses typeIds that are not ids, and a typeIds field with a selection", () => {
			const plain = analyzeLightEntities(parse(typeahead("id name typeIds")), null, {})
			expect(plain.ok).toBe(true)
			expect(analyzeLightEntities(parse(typeahead("id name typeIds { id }")), null, {})).toEqual({
				ok: false,
				reason: "selection",
			})
		})

		it("reads every space for lists that name no space, instead of counting no_space", async () => {
			const query = `query { entities(filter: {id: {in: ["${FIRST.id}"]}}, first: 1) { id relations(first: 10) { nodes { typeId toEntityId } } } }`
			const target = "7ed45f2bc48b419e8e4664d5ff680b0d"
			const rel = `${conformingId("entity", typeId)}\t${conformingId("entity", target)}\ts5\t${FIRST.id}\t-\t-\t1\n`
			const body = {data: {entities: [{id: FIRST.id, relations: {nodes: [{typeId, toEntityId: target}]}}]}}
			const held = namesSetup({[geoSpace(SPACE_B)]: rel})
			await callQuery(held, query, body)
			expect(results()).toEqual({match: 1})
			// A relation geo-db holds in no Space is still a mismatch.
			const missing = namesSetup({[geoSpace(SPACE_B)]: nameRow(FIRST.id, FIRST.name)})
			await callQuery(missing, query, body)
			expect(results()).toEqual({match: 1, mismatch: 1})
		})

		it("compares a values list filtered by propertyId that selects no property, as held under one of them", async () => {
			const NAME_ID = "a126ca53-0c8e-48d5-b888-82c734c38935"
			const query = (props: string) =>
				`query { entities(filter: {id: {in: ["${FIRST.id}"]}}, first: 1) { id values(filter: {propertyId: {in: [${props}]}}) { nodes { text } } } }`
			const body = (text: string) => ({data: {entities: [{id: FIRST.id, values: {nodes: [{text}]}}]}})
			const held = namesSetup({[geoSpace(SPACE_B)]: nameRow(FIRST.id, "Ada")}, {typeFilter: true})
			await callQuery(held, query(`"${NAME_ID}"`), body("Ada"))
			expect(results()).toEqual({match: 1})
			expect(uncompared()).toMatchObject({filter_completeness: 1})
			// A text geo-db does not hold under any filtered Property is a mismatch.
			const wrong = namesSetup({[geoSpace(SPACE_B)]: nameRow(FIRST.id, "Ada")}, {typeFilter: true})
			await callQuery(wrong, query(`"${NAME_ID}"`), body("Grace"))
			expect(results()).toEqual({match: 1, mismatch: 1})
			// Switched off, the shape stays refused.
			const off = namesSetup({[geoSpace(SPACE_B)]: nameRow(FIRST.id, "Ada")})
			await callQuery(off, query(`"${NAME_ID}"`), body("Ada"))
			expect(results()).toEqual({match: 1, mismatch: 1, unsupported_selection: 1})
		})

		it("reads every space for space-less lists under a root spaceId, and still flags an entity the root Space does not hold", async () => {
			const query = `query { entities(spaceId: "${SPACE_B}", first: 1) { id relations(first: 10) { nodes { typeId toEntityId } } } }`
			const target = "7ed45f2bc48b419e8e4664d5ff680b0d"
			const rel = `${conformingId("entity", typeId)}\t${conformingId("entity", target)}\ts5\t${FIRST.id}\t-\t-\t1\n`
			const body = {data: {entities: [{id: FIRST.id, relations: {nodes: [{typeId, toEntityId: target}]}}]}}
			// Held in the root Space (and so in every Space read): a match, not no_space.
			await callQuery(namesSetup({[geoSpace(SPACE_B)]: rel}), query, body)
			expect(results()).toEqual({match: 1})
			// Held only in another Space: the root Space does not hold it, which is a mismatch the root claims.
			await callQuery(namesSetup({[geoSpace(SPACE_A)]: rel}), query, body)
			expect(results()).toEqual({match: 1, mismatch: 1})
		})

		it("keeps an entities answer unsupported when every-space reads are off (never a mismatch)", async () => {
			const query = `query { entities(filter: {id: {in: ["${FIRST.id}"]}}, first: 1) { id relations(first: 10) { nodes { typeId toEntityId } } } }`
			const s = namesSetup({}, {flag: false})
			await callQuery(s, query, {
				data: {entities: [{id: FIRST.id, relations: {nodes: [{typeId, toEntityId: typeId}]}}]},
			})
			expect(results()).toEqual({unsupported_selection: 1})
			expect(unsupported()).toEqual({no_space: 1})
		})
	})
})

describe("entities selections with nested entities and backlinks (live refusals, 2026-11-06)", () => {
	const SPACE = "11111111-1111-4111-8111-111111111111"
	const TYPE = "22222222-2222-4222-8222-222222222222"
	const vars = {spaceIds: [SPACE], typeIds: [TYPE], filter: {}, first: 10, offset: 0}
	const analyze = (query: string, variables: Record<string, unknown> = vars) =>
		analyzeLightEntities(parse(query), null, variables, {
			entitiesBatch: true,
			relationsTypeFilter: true,
			derivedFields: true,
		})
	const VALUES =
		"valuesList(filter: {spaceId: {in: $spaceIds}}) { propertyId text boolean float datetime point schedule }"
	const wrap = (body: string) =>
		`query entities($spaceIds: [UUID!]!, $typeIds: [UUID!]!, $first: Int, $filter: EntityFilter!, $offset: Int) {
			entities(spaceIds: {in: $spaceIds} typeIds: {in: $typeIds} filter: $filter first: $first offset: $offset) {
				id name spaceIds ${VALUES} ${body}
			}
		}`
	const relations = `relations(filter: {spaceId: {in: $spaceIds}, typeId: {is: "${TYPE}"}}) {
		nodes { id entity { ${VALUES} } toEntity { id name ${VALUES} backlinks(filter: {typeId: {is: "${TYPE}"}}) { totalCount } } typeId }
	}`
	const backlinks = `backlinks(filter: {spaceId: {in: $spaceIds}, typeId: {is: "${TYPE}"}}) {
		totalCount
		nodes { id entity { ${VALUES} } toEntity: fromEntity { id name ${VALUES} ${relations} } typeId }
	}`

	it("plans a relation row's own entity and a nested entity's lists and counts as uncompared", () => {
		const a = analyze(wrap(relations))
		expect(a.ok).toBe(true)
		if (a.ok) {
			expect(a.plan.uncompared).toEqual(expect.arrayContaining(["nested_entity", "sampled_rows"]))
			expect(a.plan.lists.map((l) => l.kind)).toEqual(["values", "relations"])
		}
	})

	it("plans backlink rows that name their referrer for comparison, the rest as uncompared", () => {
		const a = analyze(wrap(`${relations} ${backlinks}`))
		expect(a.ok).toBe(true)
		if (a.ok) {
			// `toEntity: fromEntity { id }` names the referrer: compared (backlinkRows.test.ts), not counted.
			expect(a.plan.card.backlinkRows).toHaveLength(1)
			expect(a.plan.uncompared).not.toContain("backlink_rows")
			// The rows are not lists of the card: only its own values and relations are compared.
			expect(a.plan.lists).toHaveLength(2)
		}
		const list = analyze(
			wrap(
				`backlinksList(first: 50, filter: {typeId: {is: "${TYPE}"}}) { fromEntity { id comments: backlinks { totalCount } } }`,
			),
		)
		expect(list.ok && list.plan.card.backlinkRows).toHaveLength(1)
		// Rows that do not name a referrer cannot be compared.
		const bare = analyze(wrap("backlinksList(first: 50) { typeId spaceId }"))
		expect(bare.ok && bare.plan.uncompared).toContain("backlink_rows")
		expect(bare.ok && bare.plan.card.backlinkRows).toHaveLength(0)
	})

	it("still refuses what a backlink selection cannot say", () => {
		const bad = (inner: string) => analyze(wrap(`backlinks { nodes { ${inner} } }`))
		expect(bad("mystery")).toEqual({ok: false, reason: "selection"})
		expect(bad("fromEntity { mystery }")).toEqual({ok: false, reason: "selection"})
		expect(analyze(wrap('backlinks(after: "x") { totalCount nodes { id } }'))).toEqual({ok: false, reason: "args"})
		expect(analyze(wrap("backlinks { edges { cursor } }"))).toEqual({ok: false, reason: "selection"})
	})

	it("plans the generic by-id read's value and relation row ids and versions", () => {
		const a = analyze(
			`query ($ids: [UUID!]!) { entities(first: 25, filter: {id: {in: $ids}}) { id
				values(first: 100) { totalCount nodes { id propertyId spaceId language text date datetime datetimeUtc integer float decimal boolean property { id name } } pageInfo { hasNextPage endCursor } }
				relations(first: 200) { totalCount nodes { id entityId spaceId fromEntityId toEntityId typeId toSpaceId toVersionId fromSpaceId fromVersionId position } pageInfo { hasNextPage endCursor } } } }`,
			{ids: ["33333333-3333-4333-8333-333333333333"]},
		)
		expect(a.ok).toBe(true)
	})
})

describe("selections refused as `selection` on the live mix (2026-11-06)", () => {
	const id = "33333333-3333-4333-8333-333333333333"
	const property = "44444444-4444-4444-8444-444444444444"
	const space = "55555555-5555-4555-8555-555555555555"

	describe("entity(id) { values(filter: {spaceId, propertyId}) { nodes { text } } }", () => {
		const query = `query ($i: UUID!, $s: UUID!) { entity(id: $i) { values(filter: {spaceId: {is: $s}, propertyId: {is: "${property}"}}) { nodes { text } } } }`
		const vars = {i: id, s: space}

		it("is refused while the typed-list widening is off", () => {
			expect(analyzeLightEntity(parse(query), null, vars)).toEqual({ok: false, reason: "selection"})
		})

		it("is planned every-Space with the property filter when the widening is on", () => {
			const a = analyzeLightEntity(parse(query), null, vars, {relationsTypeFilter: true})
			expect(a.ok).toBe(true)
			if (a.ok) {
				expect(a.plan.everySpace).toBe(true)
				expect(a.plan.lists[0]?.propertyFilter).toEqual(["44444444444444448444444444444444"])
			}
		})
	})

	describe("the property-filtered values read, end to end", () => {
		const query = `query ($i: UUID!, $s: UUID!) { entity(id: $i) { values(filter: {spaceId: {is: $s}, propertyId: {is: "${property}"}}) { nodes { text } } } }`
		const propHex = conformingId("entity", property)
		const held = (text: string) => `${propHex}\t${text}\ts1\t${HEX}\t-\t-\t1\n`
		const shadow = (relationsTypeFilter: boolean, output: string) => {
			const log = {info: vi.fn(), warn: vi.fn()}
			const outputs = {[geoSpace(SPACE_A)]: output}
			return createEntityShadow({
				multiSpace: true,
				relationsTypeFilter,
				geoDb: {readCard: askOf(outputs), readCardSpaces: askSpacesOf(outputs)},
				geoSpaceOf: async (s: string) => geoSpace(s),
				allGeoSpaces: async () => [geoSpace(SPACE_A)],
				log,
			})
		}
		const ask = (relationsTypeFilter: boolean, output: string, text: string) =>
			call(
				shadow(relationsTypeFilter, output) as never,
				query,
				{i: HEX, s: space},
				{data: {entity: {values: {nodes: [{text}]}}}},
			)

		it("stays unsupported while the switch is off", async () => {
			await ask(false, held("Hello"), "Hello")
			expect(results()).toEqual({unsupported_selection: 1})
			expect(unsupported()).toEqual({selection: 1})
		})

		it("matches a value geo-db holds under the filter's property", async () => {
			await ask(true, held("Hello"), "Hello")
			expect(results()).toEqual({match: 1})
		})

		it("flags a value geo-db does not hold", async () => {
			await ask(true, held("Other"), "Hello")
			expect(results()).toEqual({mismatch: 1})
		})
	})

	describe("the EntitiesBatch selection with ranking scores, typeIds beside types, and toEntity { typeIds }", () => {
		const query = `query ($ids: [UUID!]!) { entities(first: 50, filter: {id: {in: $ids}}) {
			id name description spaceIds createdAt updatedAt typeIds
			agrees disagrees upvotes downvotes commenters commentScore qualityScore rankingScore trendingScore
			participationScore intrinsicScore lastListedOn listedOnAgendaId trendingTopicId trendingSource matchedTopicIds
			types { id name }
			values(first: 100) { pageInfo { hasNextPage } nodes { id propertyId property { name dataTypeName } spaceId language text boolean integer float decimal date datetime } }
			relations(first: 150) { pageInfo { hasNextPage } nodes { id entityId typeId type { name } spaceId position toEntityId toEntity { name typeIds } } }
		} }`
		const options = {entitiesBatch: true, relationsTypeFilter: true, derivedFields: true}

		it("is planned, with the scores and the repeated types counted uncompared", () => {
			const a = analyzeLightEntities(parse(query), null, {ids: [id]}, options)
			expect(a.ok).toBe(true)
			if (a.ok) {
				expect(a.plan.uncompared).toEqual(
					expect.arrayContaining(["derived_fields", "types_alias", "nested_entity"]),
				)
				expect(a.plan.card.types?.plain).toBeFalsy()
				expect(a.plan.card.types?.nameKey).toBe("name")
			}
		})

		it("keeps the ranking scores refused while the derived-fields flag is off", () => {
			expect(analyzeLightEntities(parse(query), null, {ids: [id]}, {...options, derivedFields: false}).ok).toBe(
				false,
			)
		})

		it("plans typeIds before types the same way", () => {
			const q = `{ entities(filter: {id: {in: ["${id}"]}}) { id typeIds types { id name } } }`
			const a = analyzeLightEntities(parse(q), null, {}, options)
			expect(a.ok).toBe(true)
			if (a.ok) {
				expect(a.plan.card.types?.nameKey).toBe("name")
				expect(a.plan.uncompared).toContain("types_alias")
			}
		})

		it("plans a relations list of names only as uncompared, and refuses it without the batch flag", () => {
			const q = `{ entities(filter: {id: {in: ["${id}"]}}) { id spaceIds types { name } relationsList(first: 80) { type { name } toEntity { name } } } }`
			const a = analyzeLightEntities(parse(q), null, {}, options)
			expect(a.ok).toBe(true)
			if (a.ok) expect(a.plan.uncompared).toContain("relation_names")
			expect(analyzeLightEntities(parse(q), null, {}, {...options, entitiesBatch: false}).ok).toBe(false)
		})

		it("plans `types { name }` with no id", () => {
			const q = `{ entities(filter: {id: {in: ["${id}"]}}) { id spaceIds types { name } } }`
			const a = analyzeLightEntities(parse(q), null, {}, options)
			expect(a.ok).toBe(true)
			if (a.ok) {
				expect(a.plan.card.types?.idKey).toBeNull()
				expect(a.plan.card.types?.nameKey).toBe("name")
			}
		})

		it("still refuses an unknown scalar", () => {
			const q = `{ entities(filter: {id: {in: ["${id}"]}}) { id notAColumn } }`
			expect(analyzeLightEntities(parse(q), null, {}, options)).toEqual({ok: false, reason: "selection"})
		})
	})
})

describe("entities filters read through `and`, `id: {is}` and `overlaps` (2026-11-06)", () => {
	const A = "11111111-1111-4111-8111-111111111111"
	const B = "22222222-2222-4222-8222-222222222222"
	const C = "33333333-3333-4333-8333-333333333333"
	const plan = (args: string) => {
		const a = analyzeLightEntities(parse(`{ entities(${args}) { id name } }`), null, {})
		if (!a.ok) throw new Error(`refused: ${a.reason}`)
		return a.plan
	}
	const id = (u: string) => u.replaceAll("-", "")

	it("reads a type claim and an edge claim out of an `and` list, and stays exact", () => {
		const p = plan(
			`first: 20, filter: {and: [{relations: {some: {typeId: {is: "${A}"}, toEntityId: {is: "${B}"}}}}], typeIds: {anyEqualTo: "${C}"}}`,
		)
		expect(p.claims).toEqual([
			{kind: "edge", typeIds: [id(A)], toEntityIds: [id(B)]},
			{kind: "type", typeIds: [id(C)]},
		])
		expect(p.exactFilter).toBe(true)
	})

	it("reads several edge claims, and `typeIds: {overlaps}` as a type claim", () => {
		const p = plan(
			`filter: {and: [{relations: {some: {typeId: {is: "${A}"}, toEntityId: {is: "${B}"}}}}, {typeIds: {overlaps: ["${B}", "${C}"]}}]}`,
		)
		expect(p.claims).toEqual([
			{kind: "edge", typeIds: [id(A)], toEntityIds: [id(B)]},
			{kind: "type", typeIds: [id(B), id(C)]},
		])
		expect(p.exactFilter).toBe(true)
	})

	it("leaves a clause it cannot read uncompared, without dropping the ones it can", () => {
		const p = plan(
			`filter: {and: [{typeIds: {anyEqualTo: "${C}"}}, {name: {includesInsensitive: "x"}}, {relations: {some: {typeId: {is: "${A}"}}}}]}`,
		)
		expect(p.claims).toEqual([{kind: "type", typeIds: [id(C)]}])
		expect(p.exactFilter).toBe(false)
		expect(p.uncompared).toContain("filter_completeness")
	})

	it("does not read claims out of `or` or `not`", () => {
		const p = plan(`filter: {or: [{typeIds: {anyEqualTo: "${C}"}}], not: {typeIds: {anyEqualTo: "${B}"}}}`)
		expect(p.claims).toEqual([])
		expect(p.exactFilter).toBe(false)
	})

	it("reads `id: {is}` as an id list, intersected with `id: {in}` across `and`", () => {
		expect(plan(`filter: {id: {is: "${A}"}}`).ids).toEqual([id(A)])
		expect(plan(`filter: {and: [{id: {in: ["${A}", "${B}"]}}, {id: {is: "${B}"}}]}`).ids).toEqual([id(B)])
	})

	it("combines an `id: {is}` filter with `in` root arguments", () => {
		const a = analyzeLightEntities(
			parse(
				`{ entities(first: 5, filter: {id: {is: "${A}"}}, spaceIds: {in: ["${B}"]}, typeIds: {in: ["${C}"]}) { id name spaceIds } }`,
			),
			null,
			{},
		)
		expect(a.ok).toBe(true)
		if (a.ok) {
			expect(a.plan.ids).toEqual([id(A)])
			expect(a.plan.spaceIds).toEqual([id(B)])
			expect(a.plan.claims).toEqual([{kind: "type", typeIds: [id(C)]}])
		}
	})
})

describe("entitiesConnection with the light selection", () => {
	const connection = (args: string, extra = "", wrap = (s: string) => `nodes ${s}`) => {
		const q = FIXTURE.request.query
		const open = q.indexOf('entity(id: "' + HEX + '")')
		const bodyStart = q.indexOf("{", open)
		const tail = q.slice(bodyStart)
		const lastClose = tail.lastIndexOf("}")
		const inner = tail.slice(0, lastClose) // the entity selection set, without the operation's closing brace
		return `${q.slice(0, open)}entitiesConnection(${args}) { ${extra} ${wrap(inner)} }${tail.slice(lastClose)}`
	}
	const list = (n: number): Entity[] =>
		Array.from({length: n}, (_, at) => {
			const e = clone(ENTITY)
			e.id = `${at.toString(16).padStart(2, "0")}${HEX.slice(2)}`
			return e
		})
	const outputsFor = (entities: Entity[]) => ({
		[geoSpace(SPACE_A)]: entities.map((e) => geoOutput(e).replaceAll(HEX, e.id)).join(""),
	})
	const args = `spaceId: "${SPACE_A}", first: 5`

	it("plans nodes, reads totalCount as uncompared and skips pageInfo", () => {
		const a = analyzeLightEntities(
			parse(connection(args, "totalCount pageInfo { hasNextPage }")),
			null,
			{},
			{connection: true},
		)
		expect(a.ok).toBe(true)
		if (a.ok) {
			expect(a.plan.nodesKey).toBe("nodes")
			expect(a.plan.uncompared).toContain("total_count")
		}
	})

	it("tolerates a `metas` sentinel beside the connection and counts it uncompared", () => {
		const q = connection(args).replace("entitiesConnection(", "metas { nodes { id } } entitiesConnection(")
		const a = analyzeLightEntities(parse(q), null, {}, {connection: true})
		expect(a.ok).toBe(true)
		if (a.ok) expect(a.plan.uncompared).toContain("metas_cursor")
		// The plain `entities` root still refuses a second root.
		expect(analyzeLightEntities(parse(q.replace("entitiesConnection", "entities")), null, {}).ok).toBe(false)
	})

	it("is not the plain entities root, and entities is not the connection root", () => {
		const q = connection(args)
		expect(analyzeLightEntities(parse(q), null, {})).toEqual({ok: false, reason: "not_single_root"})
		const plain = FIXTURE.request.query.replace('entity(id: "' + HEX + '")', `entities(${args})`)
		expect(analyzeLightEntities(parse(plain), null, {}, {connection: true}).ok).toBe(false)
	})

	it("plans edges { node cursor } as the nodes of the page", () => {
		const a = analyzeLightEntities(
			parse(connection(args, "", (s) => `edges { cursor node ${s} }`)),
			null,
			{},
			{connection: true},
		)
		expect(a.ok).toBe(true)
		if (a.ok) expect(a.plan.edges).toEqual({edgesKey: "edges", nodeKey: "node"})
	})

	it("counts a count-only selection, single or several roots, with its own reason", () => {
		const one = analyzeLightEntities(parse(connection(args, "totalCount", () => "")), null, {}, {connection: true})
		expect(one).toEqual({ok: false, reason: "count_only"})
		const q = connection(args, "totalCount", () => "")
		const two = q.replace(/entitiesConnection\([^)]*\) \{[^}]*\}/, (m) => `a: ${m} b: ${m}`)
		expect(analyzeLightEntities(parse(two), null, {}, {connection: true})).toEqual({
			ok: false,
			reason: "count_only",
		})
	})

	it("compares the nodes of an edges page against geo-db", async () => {
		const entities = list(2)
		const s = setup(outputsFor(entities))
		const shadow = createEntitiesConnectionShadow({...s.deps, derivedFields: false})
		await call(
			shadow as never,
			connection(args, "", (x) => `edges { node ${x} cursor }`),
			{},
			{data: {entitiesConnection: {edges: entities.map((node) => ({node, cursor: "c"}))}}},
		)
		expect(results()).toEqual({match: 1})
	})

	it("leaves a page after a cursor out of the order", () => {
		const q = connection(`${args}, orderBy: CREATED_AT_DESC`)
		const first = analyzeLightEntities(parse(q), null, {}, {connection: true})
		const after = analyzeLightEntities(
			parse(q.replace("first: 5", 'first: 5, after: "abc"')),
			null,
			{},
			{connection: true},
		)
		expect(first.ok && after.ok).toBe(true)
		if (first.ok && after.ok) {
			expect(first.plan.order).not.toBeNull()
			expect(after.plan.order).toBeNull()
		}
	})

	it("compares the sampled nodes of a page against geo-db", async () => {
		const entities = list(2)
		const s = setup(outputsFor(entities))
		const shadow = createEntitiesConnectionShadow({...s.deps, derivedFields: false})
		await call(
			shadow as never,
			connection(args, "totalCount"),
			{},
			{data: {entitiesConnection: {totalCount: 2, nodes: entities}}},
		)
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({total_count: 1})
	})

	it("flags a node that differs from geo-db", async () => {
		const entities = list(1)
		const s = setup(outputsFor(entities))
		const shadow = createEntitiesConnectionShadow(s.deps)
		const wrong = clone(entities)
		;(wrong[0] as Entity).name = "something else"
		await call(shadow as never, connection(args), {}, {data: {entitiesConnection: {nodes: wrong}}})
		expect(results()).toEqual({mismatch: 1})
	})

	it("counts a count-only selection as unsupported", async () => {
		const s = setup({})
		const shadow = createEntitiesConnectionShadow(s.deps)
		await call(
			shadow as never,
			connection(args, "totalCount", () => ""),
			{},
			{data: {}},
		)
		expect(results()).toEqual({unsupported_selection: 1})
	})
})

describe("entitiesOrderedByProperty with the light selection", () => {
	const PROPERTY = "00000000-9962-4962-9962-0000000000bb"
	const ordered = (args: string) => {
		const q = FIXTURE.request.query
		return q.replace('entity(id: "' + HEX + '")', `entitiesOrderedByProperty(${args})`)
	}
	const list = (n: number): Entity[] =>
		Array.from({length: n}, (_, at) => {
			const e = clone(ENTITY)
			e.id = `${at.toString(16).padStart(2, "0")}${HEX.slice(2)}`
			return e
		})
	const outputsFor = (entities: Entity[]) => ({
		[geoSpace(SPACE_A)]: entities.map((e) => geoOutput(e).replaceAll(HEX, e.id)).join(""),
	})
	const base = `propertyId: "${PROPERTY}", sortDirection: DESC, dataType: "integer", first: 5`

	it("plans plain Space and Type lists, and leaves order and vote narrowing uncompared", () => {
		const q = ordered(
			`${base}, spaceIds: ["${SPACE_A}"], typeIds: ["e550fe517e904b2c8fffdf13408f5634"], votedBy: "${PROPERTY}", votedByKinds: [0]`,
		)
		const a = analyzeLightEntities(parse(q), null, {}, {ordered: true})
		expect(a.ok).toBe(true)
		if (a.ok) {
			expect(a.plan.uncompared).toEqual(expect.arrayContaining(["order_completeness", "filter_completeness"]))
			expect(a.plan.claims.map((c) => c.kind)).toEqual(["type"])
			expect(a.plan.spaceIds).toEqual([SPACE_A])
		}
	})

	it("reads a relation filter naming a Type list and a target as an edge claim", () => {
		const T1 = "6e3503fab974460ea3dbab8af9a41427"
		const T2 = "e550fe517e904b2c8fffdf13408f5634"
		const target = "16d5e35f207544de85ac5fd287b04f44"
		const f = `filter: {and: [{relations: {some: {toEntityId: {is: "${target}"}, typeId: {in: ["${T1}", "${T2}"]}}}}]}`
		const a = analyzeLightEntities(parse(ordered(`${base}, ${f}`)), null, {}, {ordered: true})
		expect(a.ok).toBe(true)
		if (a.ok) {
			expect(a.plan.claims).toEqual([{kind: "edge", typeIds: [T1, T2], toEntityIds: [target]}])
			expect(a.plan.uncompared).toEqual(expect.arrayContaining(["order_completeness", "filter_completeness"]))
			expect(a.plan.exactFilter).toBe(false)
		}
	})

	it("still refuses a filter it cannot read as a claim only by leaving it uncompared", () => {
		const f = `filter: {name: {includes: "x"}}`
		const a = analyzeLightEntities(parse(ordered(`${base}, ${f}`)), null, {}, {ordered: true})
		expect(a.ok).toBe(true)
		if (a.ok) expect(a.plan.claims).toEqual([])
	})

	it("refuses an unknown argument and a missing property", () => {
		expect(analyzeLightEntities(parse(ordered(`${base}, bogus: 1`)), null, {}, {ordered: true})).toEqual({
			ok: false,
			reason: "args",
		})
		expect(analyzeLightEntities(parse(ordered("first: 5")), null, {}, {ordered: true})).toEqual({
			ok: false,
			reason: "args",
		})
	})

	it("is only its own root", () => {
		expect(analyzeLightEntities(parse(ordered(base)), null, {}).ok).toBe(false)
	})

	it("plans the order for a Space-scoped page, accepting a single uuid string for spaceIds", () => {
		const a = analyzeLightEntities(parse(ordered(`${base}, spaceIds: "${SPACE_A}"`)), null, {}, {ordered: true})
		expect(a.ok).toBe(true)
		if (a.ok) {
			expect(a.plan.propertyOrder).toMatchObject({dataType: "integer", direction: "desc", spaceIds: [SPACE_A]})
		}
		const none = analyzeLightEntities(
			parse(ordered(`${base}, spaceIds: ["${SPACE_A}"], includeWithoutValue: true`)),
			null,
			{},
			{ordered: true},
		)
		expect(none.ok && none.plan.propertyOrder).toBeFalsy()
		expect(none.ok && none.plan.uncompared).toContain("order_completeness")
	})

	it("holds the page order to geo-db's values and counts a wrong order a mismatch", async () => {
		const entities = list(2)
		const s = setup(outputsFor(entities))
		const valueRow = (e: Entity, raw: string) => ({
			prop: conformingId("entity", PROPERTY),
			raw,
			entity: conformingId("entity", e.id),
		})
		const readCards = vi.fn(async () => [
			valueRow(entities[0] as Entity, "1"),
			valueRow(entities[1] as Entity, "5"),
		])
		const shadow = createEntitiesOrderedShadow({...s.deps, geoDb: {...s.deps.geoDb, readCards}} as never)
		const query = ordered(`${base}, spaceIds: ["${SPACE_A}"]`)
		// DESC: the 5 must come first, so [1, 5] is out of order.
		await call(shadow as never, query, {}, {data: {entitiesOrderedByProperty: entities}})
		expect(results()).toEqual({mismatch: 1})
		await call(shadow as never, query, {}, {data: {entitiesOrderedByProperty: [...entities].reverse()}})
		expect(results()).toEqual({match: 1, mismatch: 1})
		// Only the mismatching run reports the plan's uncompared parts; the matching run judged the order.
		expect(uncompared().order_completeness).toBe(1)
	})

	it("compares the returned entities and flags a differing one", async () => {
		const entities = list(2)
		const s = setup(outputsFor(entities))
		const shadow = createEntitiesOrderedShadow(s.deps)
		await call(shadow as never, ordered(base), {}, {data: {entitiesOrderedByProperty: entities}})
		expect(results()).toEqual({match: 1})
		const wrong = clone(entities)
		;(wrong[0] as Entity).name = "something else"
		await call(shadow as never, ordered(base), {}, {data: {entitiesOrderedByProperty: wrong}})
		expect(results()).toEqual({match: 1, mismatch: 1})
	})
})
