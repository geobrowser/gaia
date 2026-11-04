import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {rowsFromGeoDbOutput} from "../../geodb/card"
import {GeoDbError} from "../../geodb/client"
import {readGatewayConfig} from "../config"
import {createGateway} from "../gateway"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {analyzePointRead, createPointReadShadows, MAX_BATCH_IDS} from "../pointReadShadows"

const SPACE = "11111111-1111-4111-8111-111111111111"
const GEO_SPACE = "22222222222242228222222222222222"
const A = "073ff16b-e7fe-4591-a984-c325d629d92d"
const B = "5ab2b8a5-a1a1-4e5d-9a6e-8fb2d4f0c9a1"
const C = "0a0a0a0a-0a0a-4a0a-8a0a-0a0a0a0a0a0a"
const hex = (id: string) => id.replaceAll("-", "")
const NAME = "a126ca530c8e48d5b88882c734c38935"
const TYPES = "8f151ba4de204e3c9cb499ddf96f48f1"
const PERSON = "7ed45f2bc48b419e8e4664d5ff680b0d"
const REL_TYPE = "e1c3ad9f5a3a4a1c8c0bd1b45b4d6a10"
const RANK_TYPE = "890deffb384349fa8269 74000e3dcef6".replace(" ", "")

const CARD = `id valuesList(first: 1000, filter: {spaceId: {is: $spaceId}}) {
  spaceId propertyId text integer float boolean datetime date decimal
}
relationsList(first: 1000, filter: {spaceId: {is: $spaceId}}) { typeId toEntityId toSpaceId position }`

const ENTITIES = `query Many($ids: [UUID!], $spaceId: UUID) { entities(filter: {id: {in: $ids}}, first: 10) { ${CARD} } }`
const RELATION = `query One($id: UUID!, $spaceId: UUID) {
  relation(id: $id) { id spaceId fromEntityId typeId toEntityId entityId entity { ${CARD} } }
}`
const RELATIONS = `query Some($ids: [UUID!], $spaceId: UUID) {
  relations(filter: {id: {in: $ids}}) { id spaceId fromEntityId typeId toEntityId entityId entity { ${CARD} } }
}`

const nulls = {integer: null, float: null, boolean: null, datetime: null, date: null, decimal: null}
const cardOf = (id: string, name: string) => ({
	id,
	valuesList: [{spaceId: SPACE, propertyId: NAME, text: name, ...nulls}],
	relationsList: [{typeId: TYPES, toEntityId: PERSON, toSpaceId: SPACE, position: "a"}],
})
const geoRows = (id: string, name: string) => [
	`${NAME}\t${name}\ts1\t${hex(id)}\t1`,
	`${TYPES}\t${PERSON}\ts2\t${hex(id)}\t1`,
]

/** geo-db's pipe output by (conformed) entity id; entities not listed have no statements. */
function setup(outputs: Record<string, string[]> = {[hex(A)]: geoRows(A, "Ada"), [hex(B)]: geoRows(B, "Bob")}) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const readCard = vi.fn(async ({entityId}: {geoSpaceId: string; entityId: string}) => {
		const failure = outputs.fail
		if (failure) throw new GeoDbError("timeout", "slow")
		return rowsFromGeoDbOutput((outputs[entityId] ?? []).join("\n"), entityId)
	})
	const shadows = createPointReadShadows({
		geoDb: {readCard},
		geoSpaceOf: async (space) => (space === hex(SPACE) ? GEO_SPACE : null),
		log,
	})
	return {log, readCard, shadows}
}

const run = (
	shadows: ReturnType<typeof setup>["shadows"],
	field: "entities" | "relation" | "relations",
	query: string,
	body: unknown,
	vars: Record<string, unknown>,
) =>
	shadows.get(field)?.({
		classification: {operationName: null, operationType: "query", rootFields: [field]},
		params: {query, operationName: null, variables: vars},
		document: parse(query),
		response: Response.json(body),
	})

const totals = (field: string, metric = "gaia_api_gateway_shadow_total") =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${metric}{field="${field}"`) && !l.endsWith(" 0"))
			.map((l) => [l.match(/(?:result|reason)="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
	)

beforeEach(() => __resetGatewayMetricsForTests())

const analyze = (field: "entities" | "relation" | "relations", query: string, vars: Record<string, unknown> = {}) =>
	analyzePointRead(field, parse(query), null, {spaceId: SPACE, ...vars})

describe("analyzePointRead: entities", () => {
	it("accepts an id list with the card selection, literal or variable", () => {
		expect(analyze("entities", ENTITIES, {ids: [A, B]}).ok).toBe(true)
		const literal = ENTITIES.replace("{in: $ids}", `{in: ["${A}"]}`)
		expect(analyze("entities", literal).ok).toBe(true)
	})

	it.each([
		["a filter other than id in", ENTITIES.replace("{id: {in: $ids}}", "{typeId: {is: $ids}}"), "filter"],
		[
			"an id filter with another key",
			ENTITIES.replace("{id: {in: $ids}}", '{id: {in: $ids}, name: {is: "x"}}'),
			"filter",
		],
		["no filter", ENTITIES.replace("filter: {id: {in: $ids}}, ", ""), "filter"],
		["another argument", ENTITIES.replace("first: 10", "first: 10, offset: 5"), "args"],
		["a first below the id count", ENTITIES.replace("first: 10", "first: 1"), "args"],
		["a selection without the id", ENTITIES.replace("{ id valuesList", "{ valuesList"), "selection"],
		[
			"a selection that is not a card",
			"query Q($ids: [UUID!]) { entities(filter: {id: {in: $ids}}) { id name } }",
			"no_space",
		],
	])("refuses %s", (_, query, reason) => {
		expect(analyze("entities", query, {ids: [A, B]})).toEqual({ok: false, reason})
	})

	it("refuses a bad uuid, an empty list and a batch over the limit", () => {
		expect(analyze("entities", ENTITIES, {ids: ["nope"]})).toEqual({ok: false, reason: "args"})
		expect(analyze("entities", ENTITIES, {ids: []})).toEqual({ok: false, reason: "filter"})
		const many = Array.from(
			{length: MAX_BATCH_IDS + 1},
			(_, i) => `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`,
		)
		expect(analyze("entities", ENTITIES, {ids: many})).toEqual({ok: false, reason: "batch_size"})
	})

	it("refuses extra root fields and the wrong root", () => {
		expect(
			analyze(
				"entities",
				`query Q($ids: [UUID!]) { entities(filter: {id: {in: $ids}}) { id } entity(id: "${A}") { id } }`,
				{ids: [A]},
			),
		).toEqual({
			ok: false,
			reason: "not_single_root",
		})
		expect(analyze("relation", ENTITIES, {ids: [A]})).toEqual({ok: false, reason: "not_single_root"})
	})
})

describe("analyzePointRead: relation and relations", () => {
	it("accepts a relation with its edge and entity card", () => {
		expect(analyze("relation", RELATION, {id: A}).ok).toBe(true)
		expect(analyze("relations", RELATIONS, {ids: [A, B]}).ok).toBe(true)
	})

	it("accepts nested {id} objects for the edge and notes their other fields as uncompared", () => {
		const query = `query Q($id: UUID!) { relation(id: $id) { spaceId fromEntity { id name } type { id name } toEntity { id name types { id } } entity { id name } } }`
		const result = analyze("relation", query, {id: A})
		expect(result.ok && result.plan.field === "relation" && result.plan.relation.uncompared.sort()).toEqual([
			"nested_entity",
			"nested_names",
		])
	})

	it("compares only the edge when the relation has no entity card, and only the card when the edge is partial", () => {
		const edge = `query Q($id: UUID!) { relation(id: $id) { id spaceId fromEntityId typeId toEntityId } }`
		expect(analyze("relation", edge, {id: A}).ok).toBe(true)
		const partial = `query Q($id: UUID!, $spaceId: UUID) { relation(id: $id) { id entity { ${CARD} } } }`
		const result = analyze("relation", partial, {id: A})
		expect(result.ok && result.plan.field === "relation" && result.plan.relation.uncompared).toEqual([
			"relation_edge",
		])
	})

	it("refuses a relation with nothing a card can settle, and other filters or arguments", () => {
		expect(
			analyze("relation", "query Q($id: UUID!) { relation(id: $id) { id position verified } }", {id: A}),
		).toEqual({
			ok: false,
			reason: "selection",
		})
		expect(analyze("relation", RELATION.replace("relation(id: $id)", "relation(id: $id, x: 1)"), {id: A})).toEqual({
			ok: false,
			reason: "args",
		})
		const byEntity = `query Q($id: UUID!) { relations(filter: {entityId: {is: $id}, spaceId: {is: "${SPACE}"}}) { id spaceId fromEntityId typeId toEntityId } }`
		expect(analyze("relations", byEntity, {id: A})).toEqual({ok: false, reason: "filter"})
	})

	it("refuses directives and unknown relation fields", () => {
		expect(analyze("relation", RELATION.replace("fromEntityId", "fromEntityId @skip(if: false)"), {id: A})).toEqual(
			{
				ok: false,
				reason: "directives",
			},
		)
		expect(analyze("relation", RELATION.replace("typeId toEntityId", "typeId toEntityId bogus"), {id: A})).toEqual({
			ok: false,
			reason: "selection",
		})
	})
})

describe("entities shadow", () => {
	const vars = {ids: [A, B], spaceId: SPACE}
	const body = (nodes: unknown[]) => ({data: {entities: nodes}})

	it("matches each requested id against its own card, whatever the order", async () => {
		const {shadows, readCard} = setup()
		await run(shadows, "entities", ENTITIES, body([cardOf(B, "Bob"), cardOf(A, "Ada")]), vars)
		expect(totals("entities")).toEqual({match: 1})
		expect(readCard).toHaveBeenCalledTimes(2)
		expect(readCard).toHaveBeenCalledWith(
			expect.objectContaining({geoSpaceId: GEO_SPACE, entityId: hex(A), names: true}),
		)
	})

	it("counts one mismatch, with a log, when any entity differs", async () => {
		const {shadows, log} = setup({[hex(A)]: geoRows(A, "Ada"), [hex(B)]: geoRows(B, "Robert")})
		await run(shadows, "entities", ENTITIES, body([cardOf(A, "Ada"), cardOf(B, "Bob")]), vars)
		expect(totals("entities")).toEqual({mismatch: 1})
		expect(log.warn).toHaveBeenCalledWith(
			"gateway entities shadow mismatch",
			expect.objectContaining({items: 2, mismatches: 1}),
		)
	})

	it("treats an id PostGraphile did not return as an empty card, which geo-db must also lack", async () => {
		const {shadows} = setup({[hex(A)]: geoRows(A, "Ada")})
		await run(shadows, "entities", ENTITIES, body([cardOf(A, "Ada")]), vars)
		expect(totals("entities")).toEqual({match: 1})
		const withB = setup()
		await run(withB.shadows, "entities", ENTITIES, body([cardOf(A, "Ada")]), vars)
		expect(totals("entities")).toEqual({match: 1, mismatch: 1})
	})

	it("counts failed when a geo-db read fails", async () => {
		const {shadows} = setup({fail: [], [hex(A)]: []})
		await run(shadows, "entities", ENTITIES, body([cardOf(A, "Ada"), cardOf(B, "Bob")]), vars)
		expect(totals("entities")).toEqual({failed: 1})
	})

	it("counts unsupported_selection with a reason and never asks geo-db", async () => {
		const {shadows, readCard} = setup()
		await run(shadows, "entities", ENTITIES.replace("first: 10", "first: 1"), body([]), vars)
		expect(totals("entities")).toEqual({unsupported_selection: 1})
		expect(totals("entities", "gaia_api_gateway_shadow_unsupported_total")).toEqual({args: 1})
		expect(readCard).not.toHaveBeenCalled()
	})

	it("counts a full page as truncated and counts nothing for an errored or odd answer", async () => {
		const {shadows, readCard} = setup()
		const full = {...cardOf(A, "Ada"), valuesList: Array.from({length: 1000}, () => cardOf(A, "x").valuesList[0])}
		await run(shadows, "entities", ENTITIES, body([full, cardOf(B, "Bob")]), vars)
		expect(totals("entities", "gaia_api_gateway_shadow_unsupported_total")).toEqual({truncated: 1})
		await run(shadows, "entities", ENTITIES, {errors: [{message: "boom"}]}, vars)
		await run(shadows, "entities", ENTITIES, {data: {entities: {nodes: []}}}, vars)
		expect(readCard).not.toHaveBeenCalled()
		expect(totals("entities")).toEqual({unsupported_selection: 1})
	})
})

describe("relation shadow", () => {
	const vars = {id: A, spaceId: SPACE}
	const node = (entity: unknown, over: Record<string, unknown> = {}) => ({
		id: A,
		spaceId: SPACE,
		fromEntityId: B,
		typeId: REL_TYPE,
		toEntityId: PERSON,
		entityId: A,
		entity,
		...over,
	})
	const edgeRows = (extra: string[] = []) => [`${REL_TYPE}\t${PERSON}\ts9\t${hex(B)}\t1`, ...extra]
	const outputs = (edge = true) => ({[hex(A)]: geoRows(A, "Ada"), [hex(B)]: edge ? edgeRows() : []})

	it("matches when the edge is in the from-entity's card and the entity card agrees", async () => {
		const {shadows, readCard} = setup(outputs())
		await run(shadows, "relation", RELATION, {data: {relation: node(cardOf(A, "Ada"))}}, vars)
		expect(totals("relation")).toEqual({match: 1})
		expect(readCard).toHaveBeenCalledWith(expect.objectContaining({entityId: hex(B)}))
		expect(readCard).toHaveBeenCalledWith(expect.objectContaining({entityId: hex(A)}))
	})

	it("is a mismatch when the edge is missing from geo-db", async () => {
		const {shadows, log} = setup(outputs(false))
		await run(shadows, "relation", RELATION, {data: {relation: node(cardOf(A, "Ada"))}}, vars)
		expect(totals("relation")).toEqual({mismatch: 1})
		expect(log.warn).toHaveBeenCalledWith(
			"gateway relation shadow mismatch",
			expect.objectContaining({
				first: expect.objectContaining({edge: expect.objectContaining({typeId: REL_TYPE})}),
			}),
		)
	})

	it("is a mismatch when the relation's entity card differs", async () => {
		const {shadows} = setup(outputs())
		await run(shadows, "relation", RELATION, {data: {relation: node(cardOf(A, "Someone"))}}, vars)
		expect(totals("relation")).toEqual({mismatch: 1})
	})

	it("does not compare a derived relation type, and says so", async () => {
		const {shadows, readCard} = setup(outputs())
		await run(shadows, "relation", RELATION, {data: {relation: node(cardOf(A, "Ada"), {typeId: RANK_TYPE})}}, vars)
		expect(totals("relation")).toEqual({match: 1})
		expect(totals("relation", "gaia_api_gateway_shadow_uncompared_total")).toEqual({relation_edge: 1})
		expect(readCard).toHaveBeenCalledTimes(1)
	})

	it("reads the edge from nested {id} objects", async () => {
		const query = `query Q($id: UUID!) { relation(id: $id) { spaceId fromEntity { id } type { id name } toEntity { id } } }`
		const {shadows} = setup(outputs())
		const answer = {spaceId: SPACE, fromEntity: {id: B}, type: {id: REL_TYPE, name: "T"}, toEntity: {id: PERSON}}
		await run(shadows, "relation", query, {data: {relation: answer}}, {id: A})
		expect(totals("relation")).toEqual({match: 1})
		expect(totals("relation", "gaia_api_gateway_shadow_uncompared_total")).toEqual({nested_names: 1})
	})

	it("counts nothing for a relation PostGraphile lacks, and failed when geo-db errors", async () => {
		const {shadows, readCard} = setup(outputs())
		await run(shadows, "relation", RELATION, {data: {relation: null}}, vars)
		expect(readCard).not.toHaveBeenCalled()
		expect(totals("relation")).toEqual({})
		const failing = setup({fail: []})
		await run(failing.shadows, "relation", RELATION, {data: {relation: node(cardOf(A, "Ada"))}}, vars)
		expect(totals("relation")).toEqual({failed: 1})
	})

	it("counts an unmapped space as failed", async () => {
		const {shadows} = setup(outputs())
		const other = "33333333-3333-4333-8333-333333333333"
		await run(shadows, "relation", RELATION, {data: {relation: node(cardOf(A, "Ada"), {spaceId: other})}}, vars)
		expect(totals("relation")).toEqual({failed: 1})
	})
})

describe("relations shadow", () => {
	const vars = {ids: [A, B], spaceId: SPACE}
	const node = (id: string, from: string, name: string) => ({
		id,
		spaceId: SPACE,
		fromEntityId: from,
		typeId: REL_TYPE,
		toEntityId: PERSON,
		entityId: id,
		entity: cardOf(id, name),
	})

	it("checks every relation in the answer", async () => {
		const {shadows, readCard} = setup({
			[hex(A)]: geoRows(A, "Ada"),
			[hex(B)]: geoRows(B, "Bob"),
			[hex(C)]: [`${REL_TYPE}\t${PERSON}\ts9\t${hex(C)}\t1`],
		})
		await run(shadows, "relations", RELATIONS, {data: {relations: [node(A, C, "Ada"), node(B, C, "Bob")]}}, vars)
		expect(totals("relations")).toEqual({match: 1})
		expect(readCard).toHaveBeenCalledTimes(4)
	})

	it("is a mismatch when one edge is missing", async () => {
		const {shadows} = setup({[hex(A)]: geoRows(A, "Ada"), [hex(B)]: geoRows(B, "Bob")})
		await run(shadows, "relations", RELATIONS, {data: {relations: [node(A, B, "Ada")]}}, vars)
		expect(totals("relations")).toEqual({mismatch: 1})
	})

	it("counts the by-entity filter as unsupported (filter)", async () => {
		const {shadows} = setup()
		const query = `query Q($id: UUID!) { relations(filter: {entityId: {is: $id}}) { id spaceId fromEntityId typeId toEntityId } }`
		await run(shadows, "relations", query, {data: {relations: []}}, {id: A})
		expect(totals("relations", "gaia_api_gateway_shadow_unsupported_total")).toEqual({filter: 1})
	})
})

describe("point-read shadows through the gateway", () => {
	const config = (fields: string) =>
		readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: fields, GRAPHQL_GATEWAY_SHADOW_SAMPLE: "1"})
	const request = (query: string, variables: Record<string, unknown>) =>
		new Request("http://api/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query, variables}),
		})
	const settle = () => new Promise((resolve) => setTimeout(resolve, 20))
	const answer = {data: {entities: [cardOf(A, "Ada"), cardOf(B, "Bob")]}}

	it("returns PostGraphile's response untouched and shadows in the background", async () => {
		const {shadows} = setup()
		const gateway = createGateway({
			config: config("entities=shadow"),
			delegate: () => Response.json(answer),
			shadows,
		})
		const response = await gateway(request(ENTITIES, {ids: [A, B], spaceId: SPACE}), {})
		expect(await response.json()).toEqual(answer)
		await settle()
		expect(totals("entities")).toEqual({match: 1})
	})

	it("does not shadow a field whose mode is not shadow, or when the gateway is off", async () => {
		const {shadows, readCard} = setup()
		const delegate = () => Response.json(answer)
		const offField = createGateway({config: config("entities=off"), delegate, shadows})
		await offField(request(ENTITIES, {ids: [A, B], spaceId: SPACE}), {})
		const off = createGateway({
			config: readGatewayConfig({GRAPHQL_GATEWAY_FIELDS: "entities=shadow", GRAPHQL_GATEWAY_SHADOW_SAMPLE: "1"}),
			delegate,
			shadows,
		})
		const response = await off(request(ENTITIES, {ids: [A, B], spaceId: SPACE}), {})
		expect(await response.json()).toEqual(answer)
		await settle()
		expect(readCard).not.toHaveBeenCalled()
	})

	it("passes the answer through even when geo-db hangs", async () => {
		const hang = {
			geoDb: {readCard: () => new Promise<never>(() => {})},
			geoSpaceOf: async () => GEO_SPACE,
			log: {info: vi.fn(), warn: vi.fn()},
		}
		const gateway = createGateway({
			config: config("entities=shadow"),
			delegate: () => Response.json(answer),
			shadows: createPointReadShadows(hang, ["entities"]),
		})
		const response = await gateway(request(ENTITIES, {ids: [A, B], spaceId: SPACE}), {})
		expect(await response.json()).toEqual(answer)
	})

	it("registers only the requested fields' series", () => {
		createPointReadShadows(
			{geoDb: {readCard: async () => []}, geoSpaceOf: async () => null, log: {info: vi.fn(), warn: vi.fn()}},
			["relation"],
		)
		const metrics = renderGatewayMetrics()
		expect(metrics).toContain('field="relation"')
		expect(metrics).not.toContain('field="entities"')
	})
})
