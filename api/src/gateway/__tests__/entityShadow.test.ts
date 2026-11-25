import {readFileSync} from "node:fs"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {RELATION_FACET_PROPERTIES, rowsFromGeoDbOutput} from "../../geodb/card"
import {GeoDbError} from "../../geodb/client"
import {readGatewayConfig} from "../config"
import {analyzeEntityOperation, createEntityShadow, extractPostgresCard} from "../entityShadow"
import {createGateway} from "../gateway"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"

// Recorded answers for one entity in one space: PostGraphile's JSON and the `geodb-client pipe` output
// (property, value, statement id, entity, weight; tab-separated, `\t` `\n` `\` escaped in cells).
const ENTITY = "073ff16b-e7fe-4591-a984-c325d629d92d"
const ENTITY_HEX = "073ff16be7fe4591a984c325d629d92d"
const SPACE = "11111111-1111-4111-8111-111111111111"
const GEO_SPACE = "22222222222242228222222222222222"
const NAME = "a126ca530c8e48d5b88882c734c38935"
const POINTS = "5ab2b8a5a1a14e5d9a6e8fb2d4f0c9a1"
const TYPES = "8f151ba4de204e3c9cb499ddf96f48f1"
const PERSON = "7ed45f2bc48b419e8e4664d5ff680b0d"

const QUERY = `query Card($id: UUID!, $spaceId: UUID) {
  entity(id: $id) {
    id
    valuesList(first: 1000, filter: {spaceId: {is: $spaceId}}) {
      spaceId propertyId text integer float boolean datetime date decimal
    }
    relationsList(first: 1000, filter: {spaceId: {is: $spaceId}}) { typeId toEntityId toSpaceId position }
  }
}`

const nullValues = {integer: null, float: null, boolean: null, datetime: null, date: null, decimal: null}
const POSTGRAPHILE = {
	data: {
		entity: {
			id: ENTITY,
			valuesList: [
				{spaceId: SPACE, propertyId: NAME, text: "Ada\tLovelace\nnote", ...nullValues},
				{spaceId: SPACE, propertyId: POINTS, text: null, ...nullValues, integer: "42"},
				{
					spaceId: SPACE,
					propertyId: "e1c3ad9f5a3a4a1c8c0bd1b45b4d6a10",
					text: null,
					...nullValues,
					boolean: true,
				},
			],
			relationsList: [{typeId: TYPES, toEntityId: PERSON, toSpaceId: SPACE, position: "a"}],
		},
	},
}
const GEODB_OUTPUT = [
	`${NAME}\tAda\\tLovelace\\nnote\ts1\t${ENTITY_HEX}\t1`,
	`${POINTS}\t42\ts2\t${ENTITY_HEX}\t1`,
	`e1c3ad9f5a3a4a1c8c0bd1b45b4d6a10\ttrue\ts3\t${ENTITY_HEX}\t1`,
	`${TYPES}\t${PERSON}\ts4\t${ENTITY_HEX}\t1`,
	// a backlink's entity, which the card query also returns
	`${NAME}\tSomeone else\ts5\tffffffffffffffffffffffffffffffff\t1`,
	"",
].join("\n")

const variables = {id: ENTITY, spaceId: SPACE}
const analyze = (query: string, vars: Record<string, unknown> = variables) =>
	analyzeEntityOperation(parse(query), null, vars)

function setup(output: string | Error = GEODB_OUTPUT) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const readCard = vi.fn(async ({entityId}: {geoSpaceId: string; entityId: string}) => {
		if (output instanceof Error) throw output
		return rowsFromGeoDbOutput(output, entityId)
	})
	const shadow = createEntityShadow({
		geoDb: {readCard},
		geoSpaceOf: async (space) => (space === SPACE.replaceAll("-", "") ? GEO_SPACE : null),
		log,
	})
	return {log, readCard, shadow}
}

const run = (
	shadow: ReturnType<typeof setup>["shadow"],
	query = QUERY,
	body: unknown = POSTGRAPHILE,
	vars: Record<string, unknown> = variables,
) => {
	const document = parse(query)
	return shadow({
		classification: {operationName: "Card", operationType: "query", rootFields: ["entity"]},
		params: {query, operationName: "Card", variables: vars},
		document,
		response: Response.json(body),
	})
}

const counts = () =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith("gaia_api_gateway_shadow_total{") && !l.endsWith(" 0"))
			.map((l) => [l.match(/result="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
	)

beforeEach(() => __resetGatewayMetricsForTests())

describe("analyzeEntityOperation: supported selections", () => {
	it("accepts the card selection with variables", () => {
		const result = analyze(QUERY)
		expect(result).toMatchObject({ok: true, plan: {entityId: ENTITY_HEX, spaceId: SPACE.replaceAll("-", "")}})
	})

	it("accepts literals, aliases, fragments, nested {id} objects and ignored columns", () => {
		const query = `
      query { card: entity(id: "${ENTITY}") { ...Parts __typename } }
      fragment Parts on Entity {
        id
        vs: valuesList(first: 50, filter: {spaceId: {is: "${SPACE}"}}) {
          property { id } text integer float boolean datetime date decimal language unit point time schedule spaceId
        }
        rs: relationsList(first: 50, filter: {spaceId: {is: "${SPACE}"}}) { type { id } toEntity { id } verified id }
      }`
		const result = analyze(query, {})
		expect(result.ok).toBe(true)
		if (result.ok) expect(result.plan.key).toBe("card")
	})

	it("takes a variable's declared default", () => {
		const result = analyze(QUERY.replace("$spaceId: UUID", `$spaceId: UUID = "${SPACE}"`), {id: ENTITY})
		expect(result.ok).toBe(true)
	})
})

describe("analyzeEntityOperation: unsupported selections", () => {
	const lists = (extra = "", args = `first: 10, filter: {spaceId: {is: "${SPACE}"}}`) =>
		`{ entity(id: "${ENTITY}") { id ${extra} valuesList(${args}) { propertyId text integer float boolean datetime date decimal } relationsList(${args}) { typeId toEntityId } } }`

	it.each([
		["a field no card can settle", lists("backlinks { id }"), "selection"],
		[
			"a connection beside relationsList",
			lists(`relations(first: 5, filter: {spaceId: {is: "${SPACE}"}}) { nodes { typeId toEntityId } }`),
			"selection",
		],
		[
			"a connection after a cursor",
			lists(
				`relations(first: 5, after: "abc", filter: {spaceId: {is: "${SPACE}"}}) { nodes { typeId toEntityId } }`,
			),
			"args",
		],
		["types with a field the card lacks", lists("types { id color }"), "selection"],
		["an unfiltered list that reads a column", lists("allValues: valuesList(first: 5) { text }"), "selection"],
		[
			"a second values list",
			lists(`other: valuesList(first: 1, filter: {spaceId: {is: "${SPACE}"}}) { text }`),
			"selection",
		],
		["only unfiltered lists", lists("", "first: 10"), "selection"],
		["a list without first", lists("", `filter: {spaceId: {is: "${SPACE}"}}`), "args"],
		["a list ordered", lists("", `first: 10, orderBy: TEXT_ASC, filter: {spaceId: {is: "${SPACE}"}}`), "args"],
		[
			"a filter on more than the space",
			lists("", `first: 10, filter: {spaceId: {is: "${SPACE}"}, text: {is: "x"}}`),
			"args",
		],
		["a space-in filter", lists("", `first: 10, filter: {spaceId: {in: ["${SPACE}"]}}`), "args"],
		["a malformed space", lists("", `first: 10, filter: {spaceId: {is: "nope"}}`), "args"],
		["a null space", lists("", `first: 10, filter: {spaceId: {is: null}}`), "no_space"],
		["a skip directive", lists("id @skip(if: false)"), "directives"],
	])("%s", (_, query, reason) => {
		expect(analyze(query, {})).toEqual({ok: false, reason})
	})

	it("refuses a missing space variable", () => {
		expect(analyze(QUERY, {id: ENTITY})).toEqual({ok: false, reason: "no_space"})
	})

	it("refuses an id that is not a uuid, and extra root arguments", () => {
		expect(analyze("{ entity(id: 1) { id } }", {})).toEqual({ok: false, reason: "args"})
		expect(analyze(`{ entity(id: "${ENTITY}", spaceId: "${SPACE}") { id } }`, {})).toEqual({
			ok: false,
			reason: "args",
		})
	})

	it("refuses more than one root field, or a root other than entity", () => {
		expect(analyze(`{ a: entity(id: "${ENTITY}") { id } b: entity(id: "${ENTITY}") { id } }`, {}).ok).toBe(false)
		expect(analyze(`{ entity(id: "${ENTITY}") { id } spaces { id } }`, {})).toEqual({
			ok: false,
			reason: "not_single_root",
		})
		expect(analyze("{ entities { id } }", {})).toEqual({ok: false, reason: "not_single_root"})
		expect(analyze(`mutation { entity(id: "${ENTITY}") { id } }`, {})).toEqual({
			ok: false,
			reason: "not_single_root",
		})
	})

	it("needs both lists and every compared column", () => {
		const onlyValues = `{ entity(id: "${ENTITY}") { valuesList(first: 5, filter: {spaceId: {is: "${SPACE}"}}) { propertyId text integer float boolean datetime date decimal } } }`
		expect(analyze(onlyValues, {})).toEqual({ok: false, reason: "selection"})
		const missingColumn = lists().replace(" decimal }", " }")
		expect(analyze(missingColumn, {})).toEqual({ok: false, reason: "selection"})
		expect(analyze(`{ entity(id: "${ENTITY}") { id } }`, {})).toEqual({ok: false, reason: "no_space"})
	})

	it("refuses two different spaces", () => {
		const other = "33333333-3333-4333-8333-333333333333"
		const query = lists().replace(`"${SPACE}"`, `"${other}"`)
		// only the first occurrence (the values list) moved
		expect(analyze(query, {})).toEqual({ok: false, reason: "args"})
	})
})

describe("extractPostgresCard", () => {
	const plan = (() => {
		const result = analyze(QUERY)
		if (!result.ok) throw new Error("plan")
		return result.plan
	})()

	it("reads value and relation rows", () => {
		const card = extractPostgresCard(POSTGRAPHILE, plan)
		expect(card?.values).toHaveLength(3)
		expect(card?.relations).toEqual([{type_id: TYPES, to_entity_id: PERSON, to_space_id: null}])
		expect(card?.truncated).toBe(false)
	})

	it("treats a missing entity as an empty card", () => {
		expect(extractPostgresCard({data: {entity: null}}, plan)).toEqual({values: [], relations: [], truncated: false})
	})

	it("is not comparable when PostGraphile errored or answered another shape", () => {
		expect(extractPostgresCard({errors: [{message: "x"}], data: null}, plan)).toBeNull()
		expect(extractPostgresCard({data: {entity: {id: ENTITY, valuesList: "x", relationsList: []}}}, plan)).toBeNull()
		expect(extractPostgresCard("nope", plan)).toBeNull()
	})

	it("flags a list that filled its page", () => {
		const small = analyze(QUERY.replaceAll("first: 1000", "first: 3"))
		if (!small.ok) throw new Error("plan")
		expect(extractPostgresCard(POSTGRAPHILE, small.plan)?.truncated).toBe(true)
	})
})

describe("entity shadow", () => {
	it("counts a match for the recorded pair and asks geo-db for the mapped space and conformed entity", async () => {
		const {shadow, readCard, log} = setup()
		await run(shadow)
		expect(counts()).toEqual({match: 1})
		expect(readCard).toHaveBeenCalledWith({geoSpaceId: GEO_SPACE, entityId: ENTITY_HEX, names: true})
		expect(log.warn).not.toHaveBeenCalled()
	})

	it("counts a mismatch and logs a sample when geo-db differs", async () => {
		const {shadow, log} = setup(GEODB_OUTPUT.replace("\t42\t", "\t43\t"))
		await run(shadow)
		expect(counts()).toEqual({mismatch: 1})
		expect(log.warn).toHaveBeenCalledWith(
			"gateway entity shadow mismatch",
			expect.objectContaining({entityId: ENTITY_HEX, onlyPostgres: 1, onlyGeoDb: 1}),
		)
	})

	it("counts an entity geo-db has no statements for as a mismatch, not a failure", async () => {
		await run(setup("").shadow)
		expect(counts()).toEqual({mismatch: 1})
	})

	it("counts failed when geo-db errors or the space is not mapped", async () => {
		await run(setup(new GeoDbError("timeout", "slow")).shadow)
		const unmapped = setup()
		await run(unmapped.shadow, QUERY, POSTGRAPHILE, {id: ENTITY, spaceId: "44444444-4444-4444-8444-444444444444"})
		expect(counts()).toEqual({failed: 2})
		expect(unmapped.readCard).not.toHaveBeenCalled()
	})

	it("counts unsupported_selection, with a reason, without asking geo-db", async () => {
		const {shadow, readCard} = setup()
		await run(shadow, "query Card($id: UUID!) { entity(id: $id) { id backlinks { id } } }")
		expect(counts()).toEqual({unsupported_selection: 1})
		expect(renderGatewayMetrics()).toContain(
			'gaia_api_gateway_shadow_unsupported_total{field="entity",reason="selection"} 1',
		)
		expect(readCard).not.toHaveBeenCalled()
	})

	it("counts a full page as unsupported (truncated) rather than a mismatch", async () => {
		const {shadow} = setup()
		await run(shadow, QUERY.replaceAll("first: 1000", "first: 3"))
		expect(counts()).toEqual({unsupported_selection: 1})
		expect(renderGatewayMetrics()).toContain('reason="truncated"} 1')
	})

	it("counts nothing when PostGraphile's own answer is an error", async () => {
		const {shadow, readCard} = setup()
		await run(shadow, QUERY, {errors: [{message: "boom"}], data: null})
		expect(counts()).toEqual({})
		expect(readCard).not.toHaveBeenCalled()
	})

	it("never throws, whatever the response is", async () => {
		const {shadow} = setup()
		const document = parse(QUERY)
		await expect(
			shadow({
				classification: {operationName: "Card", operationType: "query", rootFields: ["entity"]},
				params: {query: QUERY, operationName: "Card", variables},
				document,
				response: new Response("not json"),
			}),
		).resolves.toBeUndefined()
	})

	it("exports every series at zero once registered", () => {
		setup()
		expect(renderGatewayMetrics()).toContain('gaia_api_gateway_shadow_total{field="entity",result="mismatch"} 0')
	})
})

describe("entity shadow: relation facets (GEODB_RELATION_FACETS_SHADOW)", () => {
	const STATEMENT = "0a0a0a0a-0a0a-4a0a-8a0a-0a0a0a0a0a0a"
	const TO_SPACE = "33333333-3333-4333-8333-333333333333"
	const hex = (id: string) => id.replaceAll("-", "")
	const FACET_QUERY = QUERY.replace("typeId toEntityId toSpaceId position", "id typeId toEntityId toSpaceId verified")
	const body = (relation: Record<string, unknown> = {}) => ({
		data: {
			entity: {
				...POSTGRAPHILE.data.entity,
				relationsList: [
					{
						id: STATEMENT,
						typeId: TYPES,
						toEntityId: PERSON,
						toSpaceId: TO_SPACE,
						verified: true,
						...relation,
					},
				],
			},
		},
	})
	const heldBoth: [string, string][] = [
		[RELATION_FACET_PROPERTIES.toSpace, hex(TO_SPACE)],
		[RELATION_FACET_PROPERTIES.verified, "true"],
	]
	const claimOutput = (claims: [string, string][]) =>
		claims
			.map(([prop, raw]) => `${TYPES}\t${PERSON}\t${hex(STATEMENT)}\t${ENTITY_HEX}\t${prop}\t${raw}\t1`)
			.join("\n")

	function facetSetup(claims: [string, string][], on: boolean) {
		const log = {info: vi.fn(), warn: vi.fn()}
		const readCard = vi.fn(async ({entityId}: {geoSpaceId: string; entityId: string}) =>
			rowsFromGeoDbOutput(GEODB_OUTPUT, entityId),
		)
		const readRelationClaims = vi.fn(async ({entityId}: {geoSpaceId: string; entityId: string}) =>
			rowsFromGeoDbOutput(claimOutput(claims), entityId, {claims: true, claimCells: true}),
		)
		const shadow = createEntityShadow({
			geoDb: {readCard, readRelationClaims},
			geoSpaceOf: async (space) => (space === SPACE.replaceAll("-", "") ? GEO_SPACE : null),
			log,
			...(on ? {relationFacets: true} : {}),
		})
		return {shadow, log, readRelationClaims}
	}

	it("flag off: a facet difference is not looked at and claims are not read", async () => {
		const {shadow, readRelationClaims, log} = facetSetup([], false)
		await run(shadow, FACET_QUERY, body())
		expect(counts()).toEqual({match: 1})
		expect(readRelationClaims).not.toHaveBeenCalled()
		expect(log.warn).not.toHaveBeenCalled()
	})

	it("flag on: matches when geo-db holds the to-space and verified claims", async () => {
		const {shadow, readRelationClaims} = facetSetup(heldBoth, true)
		await run(shadow, FACET_QUERY, body())
		expect(counts()).toEqual({match: 1})
		expect(readRelationClaims).toHaveBeenCalledWith(expect.objectContaining({entityId: ENTITY_HEX}))
	})

	it("flag on: a facet difference is a mismatch, labelled relationFacets, and names what differs", async () => {
		const {shadow, log} = facetSetup([[RELATION_FACET_PROPERTIES.verified, "true"]], true)
		await run(shadow, FACET_QUERY, body())
		expect(counts()).toEqual({mismatch: 1})
		expect(log.warn).toHaveBeenCalledWith(
			"gateway entity shadow mismatch",
			expect.objectContaining({
				derivedMismatches: ["relationFacets"],
				relationFacets: [
					{statementId: hex(STATEMENT), diff: {toSpace: {postgres: hex(TO_SPACE), geoDb: null}}},
				],
			}),
		)
	})

	it("flag on: a relation list that selects no facet (or no id) reads no claims", async () => {
		const {shadow, readRelationClaims} = facetSetup([], true)
		await run(shadow)
		expect(counts()).toEqual({match: 1})
		expect(readRelationClaims).not.toHaveBeenCalled()
	})

	it("flag on: a row whose id is not a uuid is left uncompared, not judged", async () => {
		const {shadow, readRelationClaims} = facetSetup([], true)
		await run(shadow, FACET_QUERY, body({id: "not-a-uuid"}))
		expect(counts()).toEqual({match: 1})
		expect(readRelationClaims).not.toHaveBeenCalled()
		expect(renderGatewayMetrics()).toContain('reason="relation_facets"')
	})
})

describe("gateway wiring", () => {
	const body = {query: QUERY, operationName: "Card", variables}
	const request = () =>
		new Request("http://api/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify(body),
		})
	const pgResponse = () => Response.json(POSTGRAPHILE)
	const config = (fields: string, sample = "1") =>
		readGatewayConfig({
			GRAPHQL_GATEWAY: "on",
			GRAPHQL_GATEWAY_FIELDS: fields,
			GRAPHQL_GATEWAY_SHADOW_SAMPLE: sample,
		})
	const settle = () => new Promise((resolve) => setTimeout(resolve, 20))

	it("answers with PostGraphile's response untouched and compares in the background", async () => {
		const {shadow} = setup()
		const gateway = createGateway({
			config: config("entity=shadow"),
			delegate: pgResponse,
			shadows: new Map([["entity", shadow]]),
		})
		const response = await gateway(request(), {})
		expect(await response.json()).toEqual(POSTGRAPHILE)
		await settle()
		expect(counts()).toEqual({match: 1})
	})

	it("does not wait for a slow shadow and survives a throwing one", async () => {
		const slow = vi.fn(() => new Promise<void>(() => {}))
		const gateway = createGateway({
			config: config("entity=shadow"),
			delegate: pgResponse,
			shadows: new Map([["entity", slow]]),
		})
		expect((await gateway(request(), {})).status).toBe(200)
		const throwing = vi.fn(async () => {
			throw new Error("boom")
		})
		const second = createGateway({
			config: config("entity=shadow"),
			delegate: pgResponse,
			shadows: new Map([["entity", throwing]]),
		})
		expect((await second(request(), {})).status).toBe(200)
		await settle()
		expect(slow).toHaveBeenCalledTimes(1)
		expect(throwing).toHaveBeenCalledTimes(1)
	})

	it("shadows only when the mode is shadow, the draw is under the sample rate and the operation is entity alone", async () => {
		const shadow = vi.fn(async () => {})
		const run1 = async (cfg: ReturnType<typeof config>, random: number, query = QUERY) => {
			const gateway = createGateway({
				config: cfg,
				delegate: pgResponse,
				shadows: new Map([["entity", shadow]]),
				random: () => random,
			})
			await gateway(
				new Request("http://api/graphql", {method: "POST", body: JSON.stringify({query, variables})}),
				{},
			)
			await settle()
		}
		await run1(config("entity=on"), 0)
		await run1(config(""), 0)
		await run1(config("entity=shadow", "0.1"), 0.5)
		await run1(config("entity=shadow", "0"), 0)
		await run1(config("entity=shadow"), 0, 'query { entity(id: "x") { id } spaces { id } }')
		await run1(config("entity=shadow"), 0, 'mutation { entity(id: "x") { id } }')
		expect(shadow).not.toHaveBeenCalled()
		await run1(config("entity=shadow", "0.1"), 0.05)
		expect(shadow).toHaveBeenCalledTimes(1)
	})

	it("does not shadow an error response or a gateway that is off", async () => {
		const shadow = vi.fn(async () => {})
		const shadows = new Map([["entity", shadow]])
		await createGateway({
			config: config("entity=shadow"),
			delegate: () => Response.json({}, {status: 500}),
			shadows,
		})(request(), {})
		await createGateway({
			config: readGatewayConfig({GRAPHQL_GATEWAY_FIELDS: "entity=shadow"}),
			delegate: pgResponse,
			shadows,
		})(request(), {})
		await settle()
		expect(shadow).not.toHaveBeenCalled()
	})
})

describe("entity shadow: live relationsList, selection, and multi-root shapes", () => {
	const ENTITY_B = "173ff16b-e7fe-4591-a984-c325d629d92d"
	const ENTITY_B_HEX = "173ff16be7fe4591a984c325d629d92d"
	const TARGET_A = "7ed45f2b-c48b-419e-8e46-64d5ff680b0d"
	const TARGET_A_HEX = "7ed45f2bc48b419e8e4664d5ff680b0d"
	const TARGET_B = "8ed45f2b-c48b-419e-8e46-64d5ff680b0d"
	const TARGET_B_HEX = "8ed45f2bc48b419e8e4664d5ff680b0d"
	const REL_TYPE = "9f151ba4-de20-4e3c-9cb4-99ddf96f48f1"
	const REL_TYPE_HEX = "9f151ba4de204e3c9cb499ddf96f48f1"
	const COVER_TYPE = "af151ba4-de20-4e3c-9cb4-99ddf96f48f1"
	const COVER_TYPE_HEX = "af151ba4de204e3c9cb499ddf96f48f1"
	const TAGLINE_PROP = "bf151ba4-de20-4e3c-9cb4-99ddf96f48f1"
	const TAGLINE_PROP_HEX = "bf151ba4de204e3c9cb499ddf96f48f1"
	const DESC_PROP = "9b1f76ff-9711-404c-861e-59dc3fa7d037"
	const DESC_PROP_HEX = "9b1f76ff9711404c861e59dc3fa7d037"
	const SPACE_HEX = SPACE.replaceAll("-", "")

	const unsupportedCounts = () =>
		Object.fromEntries(
			renderGatewayMetrics()
				.split("\n")
				.filter(
					(l) =>
						l.startsWith('gaia_api_gateway_shadow_unsupported_total{field="entity"') && !l.endsWith(" 0"),
				)
				.map((l) => [l.match(/reason="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
		)

	function multiSetup(
		cards: Record<string, string>,
		extra: {everySpace?: boolean; relationsTypeFilter?: boolean} = {},
	) {
		const log = {info: vi.fn(), warn: vi.fn()}
		const readCard = vi.fn(async ({entityId}: {geoSpaceId: string; entityId: string}) =>
			rowsFromGeoDbOutput(cards[entityId] ?? "", entityId, {names: true}),
		)
		const readCardSpaces = vi.fn(async ({entityId}: {geoSpaceIds: string[] | "every"; entityId: string}) =>
			rowsFromGeoDbOutput(cards[entityId] ?? "", entityId, {names: true}).map((r) => ({...r, space: GEO_SPACE})),
		)
		const shadow = createEntityShadow({
			geoDb: {readCard, readCardSpaces},
			geoSpaceOf: async (space) => (space === SPACE_HEX ? GEO_SPACE : null),
			multiSpace: true,
			relationsTypeFilter: extra.relationsTypeFilter ?? true,
			...(extra.everySpace ? {allGeoSpaces: async () => [GEO_SPACE]} : {}),
			log,
		})
		return {shadow, readCard, readCardSpaces, log}
	}

	const invoke = (
		shadow: ReturnType<typeof multiSetup>["shadow"],
		query: string,
		body: unknown,
		vars: Record<string, unknown> = {},
		operationName: string | null = null,
	) =>
		shadow({
			classification: {operationName, operationType: "query", rootFields: ["entity"]},
			params: {query, operationName, variables: vars},
			document: parse(query),
			response: Response.json(body),
		})

	it("main.ts wires GEODB_RELATIONS_TYPE_FILTER_SHADOW into createEntityShadow", () => {
		const main = readFileSync(new URL("../../../main.ts", import.meta.url), "utf8")
		const start = main.indexOf("createEntityShadow({")
		expect(start).toBeGreaterThan(0)
		const block = main.slice(start, main.indexOf("createPointReadShadows(", start))
		expect(block).toContain("relationsTypeFilter: geoDbCardConfig.relationsTypeFilter === true")
	})

	it("shadows the live DebateTranscriptClaims shape (args=id;sel=relationsList with typeId + spaceId filter)", async () => {
		const query = `query DebateTranscriptClaims(
			$id: UUID!, $transcriptsPropertyId: UUID!, $blocksPropertyId: UUID!, $spaceId: UUID!, $first: Int
		) {
			entity(id: $id) {
				transcripts: relationsList(
					first: $first
					filter: {typeId: {is: $transcriptsPropertyId}, spaceId: {is: $spaceId}}
				) {
					position
					toEntity {
						id
						blocks: relationsList(
							first: $first
							filter: {typeId: {is: $blocksPropertyId}, spaceId: {is: $spaceId}}
						) { position toEntity { id } }
					}
				}
			}
		}`
		const vars = {
			id: ENTITY,
			transcriptsPropertyId: REL_TYPE,
			blocksPropertyId: COVER_TYPE,
			spaceId: SPACE,
			first: 100,
		}
		const body = {data: {entity: {transcripts: [{position: "a", toEntity: {id: TARGET_A, blocks: []}}]}}}
		const ok = multiSetup(
			{[ENTITY_HEX]: `${REL_TYPE_HEX}\t${TARGET_A_HEX}\ts1\t${ENTITY_HEX}\t-\t-\t1\n`},
			{everySpace: true},
		)
		await invoke(ok.shadow, query, body, vars, "DebateTranscriptClaims")
		expect(counts()).toEqual({match: 1})

		__resetGatewayMetricsForTests()
		const bad = multiSetup({[ENTITY_HEX]: ""}, {everySpace: true})
		await invoke(bad.shadow, query, body, vars, "DebateTranscriptClaims")
		expect(counts()).toEqual({mismatch: 1})
	})

	it("shadows the live ParticipantByline-style typed valuesList + relationsList shape using row spaceId without everySpace", async () => {
		const query = `{
			p0: entity(id: "${ENTITY}") {
				tagline: valuesList(first: 1, filter: {propertyId: {is: "${TAGLINE_PROP}"}, spaceId: {is: "${SPACE}"}}) { text }
				employment: relationsList(first: 200, filter: {typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE}"}}) {
					id entityId spaceId
					toEntity { id name }
					entity { valuesList(first: 200, filter: {spaceId: {is: "${SPACE}"}}) { property { id } text } }
				}
			}
		}`
		const body = {
			data: {
				p0: {
					tagline: [{text: "Mathematician"}],
					employment: [
						{
							id: "r1",
							entityId: ENTITY,
							spaceId: SPACE,
							toEntity: {id: TARGET_A, name: "Engine"},
							entity: {valuesList: []},
						},
					],
				},
			},
		}
		const card = [
			`${TAGLINE_PROP_HEX}\tMathematician\ts1\t${ENTITY_HEX}\t-\t-\t1`,
			`${REL_TYPE_HEX}\t${TARGET_A_HEX}\ts2\t${ENTITY_HEX}\t-\tEngine\t1`,
			"",
		].join("\n")
		// No allGeoSpaces: must resolve the Space from employment's typeless row spaceId.
		const {shadow, readCard} = multiSetup({[ENTITY_HEX]: card}, {everySpace: false})
		await invoke(shadow, query, body)
		expect(counts()).toEqual({match: 1})
		expect(readCard).toHaveBeenCalledWith({geoSpaceId: GEO_SPACE, entityId: ENTITY_HEX, names: true})
	})

	it("shadows the live selection shape with typeOnly relationsList and valuesList(propertyId: {in: []})", async () => {
		const query = `{
			entity(id: "${ENTITY}") {
				id name typeIds
				relationsList(filter: {typeId: {is: "${REL_TYPE}"}}, first: 1) {
					toEntity { valuesList(filter: {propertyId: {is: "${TAGLINE_PROP}"}}) { text } }
				}
				description
				valuesList(filter: {propertyId: {in: []}}) { propertyId spaceId text }
				cover: relationsList(filter: {typeId: {is: "${COVER_TYPE}"}}, first: 1) {
					toEntity { valuesList(filter: {propertyId: {is: "${TAGLINE_PROP}"}}) { text } }
				}
			}
		}`
		const body = {
			data: {
				entity: {
					id: ENTITY,
					name: "Ada",
					typeIds: [PERSON],
					relationsList: [{toEntity: {valuesList: [{text: "x"}]}}],
					description: "Pioneer",
					valuesList: [],
					cover: [{toEntity: {valuesList: [{text: "y"}]}}],
				},
			},
		}
		const card = [
			`${NAME}\tAda\ts1\t${ENTITY_HEX}\t-\t-\t1`,
			`${DESC_PROP_HEX}\tPioneer\ts2\t${ENTITY_HEX}\t-\t-\t1`,
			`${TYPES}\t${PERSON}\ts3\t${ENTITY_HEX}\t-\t-\t1`,
			`${REL_TYPE_HEX}\t${TARGET_A_HEX}\ts4\t${ENTITY_HEX}\t-\t-\t1`,
			`${COVER_TYPE_HEX}\t${TARGET_B_HEX}\ts5\t${ENTITY_HEX}\t-\t-\t1`,
			"",
		].join("\n")
		const {shadow} = multiSetup({[ENTITY_HEX]: card}, {everySpace: true})
		await invoke(shadow, query, body)
		expect(counts()).toEqual({match: 1})
	})

	it("shadows multiple aliased entity roots in one document (match and mismatch)", async () => {
		const query = `{
			p0: entity(id: "${ENTITY}") {
				tagline: valuesList(first: 1, filter: {propertyId: {is: "${TAGLINE_PROP}"}, spaceId: {is: "${SPACE}"}}) { text }
				employment: relationsList(first: 200, filter: {typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE}"}}) {
					spaceId toEntity { id }
				}
			}
			p1: entity(id: "${ENTITY_B}") {
				tagline: valuesList(first: 1, filter: {propertyId: {is: "${TAGLINE_PROP}"}, spaceId: {is: "${SPACE}"}}) { text }
				employment: relationsList(first: 200, filter: {typeId: {is: "${REL_TYPE}"}, spaceId: {is: "${SPACE}"}}) {
					spaceId toEntity { id }
				}
			}
		}`
		const body = {
			data: {
				p0: {tagline: [{text: "Ada"}], employment: [{spaceId: SPACE, toEntity: {id: TARGET_A}}]},
				p1: {tagline: [{text: "Charles"}], employment: [{spaceId: SPACE, toEntity: {id: TARGET_B}}]},
			},
		}
		const cardA = `${TAGLINE_PROP_HEX}\tAda\ts1\t${ENTITY_HEX}\t-\t-\t1\n${REL_TYPE_HEX}\t${TARGET_A_HEX}\ts2\t${ENTITY_HEX}\t-\t-\t1\n`
		const cardB = `${TAGLINE_PROP_HEX}\tCharles\ts3\t${ENTITY_B_HEX}\t-\t-\t1\n${REL_TYPE_HEX}\t${TARGET_B_HEX}\ts4\t${ENTITY_B_HEX}\t-\t-\t1\n`

		const ok = multiSetup({[ENTITY_HEX]: cardA, [ENTITY_B_HEX]: cardB})
		await invoke(ok.shadow, query, body)
		expect(counts()).toEqual({match: 1})

		__resetGatewayMetricsForTests()
		const bad = multiSetup({
			[ENTITY_HEX]: cardA,
			[ENTITY_B_HEX]: `${TAGLINE_PROP_HEX}\tWrong\ts3\t${ENTITY_B_HEX}\t-\t-\t1\n`,
		})
		await invoke(bad.shadow, query, body)
		expect(counts()).toEqual({mismatch: 1})
		expect(bad.log.warn).toHaveBeenCalledWith(
			"gateway entity shadow mismatch (light)",
			expect.objectContaining({firstKey: "p1", mismatches: 1}),
		)
	})

	it("shadows multi-root entity operations via root fragments", async () => {
		const query = `
			query { ...Batch }
			fragment Batch on Query {
				a: entity(id: "${ENTITY}") { id name spaceIds }
				b: entity(id: "${ENTITY_B}") { id name spaceIds }
			}`
		const body = {
			data: {
				a: {id: ENTITY, name: "Ada", spaceIds: [SPACE]},
				b: {id: ENTITY_B, name: "Charles", spaceIds: [SPACE]},
			},
		}
		const {shadow} = multiSetup({
			[ENTITY_HEX]: `${NAME}\tAda\ts1\t${ENTITY_HEX}\t-\t-\t1\n`,
			[ENTITY_B_HEX]: `${NAME}\tCharles\ts2\t${ENTITY_B_HEX}\t-\t-\t1\n`,
		})
		await invoke(shadow, query, body)
		expect(counts()).toEqual({match: 1})
	})

	it("tolerates arbitrary sibling roots via tolerates='*' without altering the gateway response", async () => {
		const query = `{ entity(id: "${ENTITY}") { id name spaceIds } spaces { id } }`
		const pgBody = {data: {entity: {id: ENTITY, name: "Ada", spaceIds: [SPACE]}, spaces: [{id: SPACE}]}}
		const {shadow} = multiSetup({[ENTITY_HEX]: `${NAME}\tAda\ts1\t${ENTITY_HEX}\t-\t-\t1\n`})
		expect(shadow.tolerates).toBe("*")
		const gateway = createGateway({
			config: readGatewayConfig({
				GRAPHQL_GATEWAY: "on",
				GRAPHQL_GATEWAY_FIELDS: "entity=shadow",
				GRAPHQL_GATEWAY_SHADOW_SAMPLE: "1",
			}),
			delegate: () => Response.json(pgBody),
			shadows: new Map([["entity", shadow]]),
		})
		const response = await gateway(
			new Request("http://api/graphql", {method: "POST", body: JSON.stringify({query})}),
			{},
		)
		expect(await response.json()).toEqual(pgBody)
		await new Promise((resolve) => setTimeout(resolve, 20))
		expect(counts()).toEqual({match: 1})
	})

	it("reports honest unsupported reasons in multi-root documents and keeps not_single_root when no entity root exists", async () => {
		await invoke(
			multiSetup({}).shadow,
			`{ a: entity(id: "not-a-uuid") { id } b: entity(id: "${ENTITY}") { id name spaceIds } }`,
			{data: {}},
		)
		expect(unsupportedCounts()).toEqual({args: 1})

		__resetGatewayMetricsForTests()
		await invoke(multiSetup({}).shadow, `{ entity(id: "${ENTITY}") { unknownField } spaces { id } }`, {data: {}})
		expect(unsupportedCounts()).toEqual({selection: 1})

		__resetGatewayMetricsForTests()
		await invoke(multiSetup({}).shadow, `{ spaces { id } }`, {data: {}})
		expect(unsupportedCounts()).toEqual({not_single_root: 1})
	})
})
