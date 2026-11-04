import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import type {GeoCardRow} from "../../geodb/card"
import {createEntityResolver, type EntityServeDeps, planEntityServe} from "../entityServe"
import type {ResolveContext} from "../gateway"

const ENTITY = "6b9f649e38b24b9a8e3c1b6a0b3c3c3a"
const SPACE = "25a8a6b8c33b4b6ab8f3c2e0a2b1c0d9"
const TYPE = "8f151ba4de204e3c9cb499ddf96f48f1"
const TARGET = "0d4f1a2b3c4d4e5f8a6b7c8d9e0f1a2b"
const MEMBER = "dbe3bdf637a5458d9206d737fb104793"

const RELATIONS = `relationsList(filter: {spaceId: {is: "${SPACE}"}}) { typeId toEntityId __typename }`

const context = (query: string, variables: Record<string, unknown> = {}): ResolveContext => {
	const document = parse(query)
	return {
		classification: {operationName: null, operationType: "query", rootFields: ["entity"]},
		params: {query, operationName: null, variables},
		document,
		signal: new AbortController().signal,
	}
}

const deps = (rows: GeoCardRow[], overrides: Partial<EntityServeDeps> = {}) => {
	const readCard = vi.fn(async () => rows)
	return {
		readCard,
		deps: {geoDb: {readCard}, geoSpaceOf: async (s: string) => s, gaiaIdOf: (id: string) => id, ...overrides},
	}
}

describe("planEntityServe", () => {
	const reason = (query: string, variables: Record<string, unknown> = {}) => {
		try {
			planEntityServe(parse(query), null, variables)
			return "ok"
		} catch (error) {
			return (error as {reason?: string}).reason
		}
	}

	it("accepts the one supported shape, with aliases, fragments and variables", () => {
		expect(reason(`{ entity(id: "${ENTITY}") { id ${RELATIONS} } }`)).toBe("ok")
		expect(
			reason(
				`query Q($id: UUID!, $s: UUID!) { e: entity(id: $id) { ...F } } fragment F on Entity { __typename r: relationsList(filter: {spaceId: {is: $s}}) { typeId } }`,
				{id: ENTITY, s: SPACE},
			),
		).toBe("ok")
	})

	it("names the gap for everything else", () => {
		expect(reason(`{ entity(id: "${ENTITY}") { id name } }`)).toBe("selection")
		expect(reason(`{ entity(id: "${ENTITY}") { id } }`)).toBe("no_space")
		expect(reason(`{ entity(id: "${ENTITY}") { relationsList { typeId } } }`)).toBe("no_space")
		expect(reason(`{ entity(id: "${ENTITY}") { relationsList(filter: {spaceId: {is: null}}) { typeId } } }`)).toBe(
			"no_space",
		)
		expect(reason(`{ entity(id: "${ENTITY}") { relationsList(first: 5) { typeId } } }`)).toBe("args")
		expect(
			reason(`{ entity(id: "${ENTITY}") { relationsList(filter: {typeId: {is: "${TYPE}"}}) { typeId } } }`),
		).toBe("filter")
		expect(
			reason(
				`{ entity(id: "${ENTITY}") { relationsList(filter: {spaceId: {is: "${SPACE}"}}) { id position } } }`,
			),
		).toBe("selection")
		expect(reason(`{ entity(id: "not-a-uuid") { ${RELATIONS} } }`)).toBe("args")
		expect(reason(`{ entity(id: "${ENTITY}") @skip(if: false) { ${RELATIONS} } }`)).toBe("directives")
		expect(reason(`{ entity(id: "${ENTITY}") { ${RELATIONS} } spaces { id } }`)).toBe("not_single_root")
	})
})

describe("createEntityResolver", () => {
	const query = `{ entity(id: "${ENTITY}") { __typename id rels: ${RELATIONS} } }`

	it("declines with legacy_id, without reading geo-db, while there is no reverse id map", async () => {
		const {deps: d, readCard} = deps([{prop: TYPE, raw: TARGET}], {gaiaIdOf: undefined})
		expect(await createEntityResolver(d)(context(query))).toEqual({ok: false, reason: "legacy_id"})
		expect(readCard).not.toHaveBeenCalled()
	})

	it("declines an unsupported selection before reading geo-db", async () => {
		const {deps: d, readCard} = deps([])
		expect(await createEntityResolver(d)(context(`{ entity(id: "${ENTITY}") { name } }`))).toEqual({
			ok: false,
			reason: "selection",
		})
		expect(readCard).not.toHaveBeenCalled()
	})

	it("serves relations in PostGraphile's shape and selection order, skipping values and membership", async () => {
		const {deps: d} = deps([
			{prop: TYPE, raw: TARGET},
			{prop: TYPE, raw: "plain text"},
			{prop: MEMBER, raw: TARGET},
		])
		expect(await createEntityResolver(d)(context(query))).toEqual({
			ok: true,
			data: {
				entity: {
					__typename: "Entity",
					id: ENTITY,
					rels: [{typeId: TYPE, toEntityId: TARGET, __typename: "Relation"}],
				},
			},
		})
		const result = await createEntityResolver(d)(context(query))
		expect(result.ok && Object.keys(result.data.entity as object)).toEqual(["__typename", "id", "rels"])
	})

	it("declines an empty card, an unmapped id and a derived property", async () => {
		expect(await createEntityResolver(deps([]).deps)(context(query))).toEqual({ok: false, reason: "empty"})
		const unmapped = deps([{prop: TYPE, raw: TARGET}], {gaiaIdOf: (id) => (id === TARGET ? null : id)}).deps
		expect(await createEntityResolver(unmapped)(context(query))).toEqual({ok: false, reason: "legacy_id"})
		const derived = deps([{prop: "890deffb384349fa826974000e3dcef6", raw: TARGET}]).deps
		expect(await createEntityResolver(derived)(context(query))).toEqual({ok: false, reason: "selection"})
	})

	it("lets a geo-db failure through for the gateway to count", async () => {
		const failing = {
			geoDb: {readCard: async () => Promise.reject(Object.assign(new Error("x"), {reason: "timeout"}))},
		}
		const d = {...deps([]).deps, ...failing}
		await expect(createEntityResolver(d)(context(query))).rejects.toMatchObject({reason: "timeout"})
	})
})
