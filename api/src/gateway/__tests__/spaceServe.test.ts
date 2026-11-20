import {readFileSync} from "node:fs"
import {join} from "node:path"
import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow} from "../../geodb/card"
import {GeoDbError} from "../../geodb/client"
import {entityRef, idPart, recordEntityId} from "../../proposals/governance/recordKey"
import type {ResolveContext} from "../gateway"
import {
	createSpaceResolver,
	MAX_SPACES_PER_PAGE,
	planSpaceServe,
	SPACE_ADDRESS_COLUMNS,
	SPACE_ADDRESS_KIND,
	SPACE_TOPIC_COLUMNS,
	SPACE_TOPIC_KIND,
	spaceTypeOf,
} from "../spaceServe"

const A = "25a8a6b8c33b4b6ab8f3c2e0a2b1c0d9"
const B = "0d4f1a2b3c4d4e5f8a6b7c8d9e0f1a2b"
const KIND = conformingId("entity", "88b3d6ad-288c-529c-a212-0e1c24819185")
const EOA = conformingId("entity", "09d28123-178f-5828-b85a-81979389c746")
const DAO = conformingId("entity", "afd76215-db11-5cba-81b9-e7f77f865805")
const SYSTEM = conformingId("entity", "5f2f9d2b1c3a5c7e9d0b1a2c3d4e5f60")

const context = (query: string, variables: Record<string, unknown> = {}): ResolveContext => ({
	classification: {operationName: null, operationType: "query", rootFields: ["space"]},
	params: {query, operationName: null, variables},
	document: parse(query),
	signal: new AbortController().signal,
})

const card = (kind: string): GeoCardRow[] => [
	{prop: KIND, raw: SYSTEM},
	{prop: KIND, raw: kind},
]

const deps = (rowsOf: (entityId: string) => GeoCardRow[], held: (s: string) => boolean = () => true) => {
	const readCard = vi.fn(async ({entityId}: {entityId: string}) => rowsOf(entityId))
	return {readCard, deps: {geoDb: {readCard}, geoSpaceOf: async (s: string) => (held(s) ? `geo-${s}` : null)}}
}

describe("planSpaceServe", () => {
	const reason = (query: string, root: "space" | "spaces" = "space") => {
		try {
			planSpaceServe(parse(query), null, {}, root)
			return "ok"
		} catch (error) {
			return (error as {reason?: string}).reason
		}
	}
	it("accepts id, type and __typename of one Space or of an id list", () => {
		expect(reason(`{ space(id: "${A}") { id type __typename } }`)).toBe("ok")
		expect(reason(`{ s: space(id: "${A}") { t: type } }`)).toBe("ok")
		expect(reason(`{ spaces(filter: {id: {in: ["${A}", "${B}"]}}) { id type } }`, "spaces")).toBe("ok")
	})
	it("accepts address and topicId", () => {
		expect(reason(`{ space(id: "${A}") { id address topicId } }`)).toBe("ok")
		expect(reason(`{ spaces(filter: {id: {in: ["${A}"]}}) { address } }`, "spaces")).toBe("ok")
	})
	it("names the gap for everything else", () => {
		expect(reason(`{ space(id: "${A}") { id slug } }`)).toBe("selection")
		expect(reason(`{ space(id: "${A}") { id page { id } } }`)).toBe("selection")
		expect(reason(`{ space(id: "${A}") { id membersList { memberSpaceId } } }`)).toBe("selection")
		expect(reason(`{ space { id } }`)).toBe("args")
		expect(reason(`{ space(id: "nope") { id } }`)).toBe("args")
		expect(reason(`{ space(id: "${A}") @skip(if: false) { id } }`)).toBe("directives")
		expect(reason(`{ spaces { id } }`, "spaces")).toBe("filter")
		expect(reason(`{ spaces(first: 3) { id } }`, "spaces")).toBe("args")
		expect(reason(`{ spaces(filter: {address: {equalToInsensitive: "0x1"}}) { id } }`, "spaces")).toBe("filter")
		expect(reason(`{ spaces(filter: {id: {in: ["${A}"]}}, first: 1) { id } }`, "spaces")).toBe("args")
		expect(reason(`{ spaces(filter: {members: {some: {memberSpaceId: {is: "${A}"}}}}) { id } }`, "spaces")).toBe(
			"filter",
		)
		const many = Array.from({length: MAX_SPACES_PER_PAGE + 1}, (_, i) => `"${i.toString(16).padStart(32, "0")}"`)
		expect(reason(`{ spaces(filter: {id: {in: [${many}]}}) { id } }`, "spaces")).toBe("truncated")
	})
})

describe("spaceTypeOf", () => {
	it("reads the kind relation, and is null when absent or contradictory", () => {
		expect(spaceTypeOf(card(EOA))).toBe("PERSONAL")
		expect(spaceTypeOf(card(DAO))).toBe("DAO")
		expect(spaceTypeOf([{prop: KIND, raw: SYSTEM}])).toBeNull()
		expect(spaceTypeOf([...card(EOA), {prop: KIND, raw: DAO}])).toBeNull()
		expect(spaceTypeOf([{prop: "ff", raw: EOA}])).toBeNull()
	})
})

describe("createSpaceResolver", () => {
	it("serves space(id) from the Space's own card, in the Space", async () => {
		const {deps: d, readCard} = deps(() => card(DAO))
		const out = await createSpaceResolver(d, "space")(context(`{ space(id: "${A}") { id type __typename } }`))
		expect(out).toEqual({ok: true, data: {space: {id: A, type: "DAO", __typename: "Space"}}})
		expect(readCard).toHaveBeenCalledWith({geoSpaceId: `geo-${A}`, entityId: conformingId("entity", A)})
	})
	it("serves a spaces page sorted by id, one read per Space", async () => {
		const {deps: d, readCard} = deps((e) => (e === conformingId("entity", A) ? card(EOA) : card(DAO)))
		const query = `{ spaces(filter: {id: {in: ["${A}", "${B}", "${A}"]}}) { id type } }`
		const out = await createSpaceResolver(d, "spaces")(context(query))
		expect(out).toEqual({
			ok: true,
			data: {
				spaces: [
					{id: B, type: "DAO"},
					{id: A, type: "PERSONAL"},
				],
			},
		})
		expect(readCard).toHaveBeenCalledTimes(2)
		// Each Space is read in its own Space: a page that read every Space in the first one's would still pass the shape above.
		expect(readCard).toHaveBeenCalledWith({geoSpaceId: `geo-${A}`, entityId: conformingId("entity", A)})
		expect(readCard).toHaveBeenCalledWith({geoSpaceId: `geo-${B}`, entityId: conformingId("entity", B)})
	})
	it("declines a Space geo-db has not founded, naming unmapped_space through the thrown reason", async () => {
		const {deps: d} = deps(
			() => card(DAO),
			() => false,
		)
		const run = createSpaceResolver(d, "space")(context(`{ space(id: "${A}") { id } }`))
		await expect(run).rejects.toMatchObject({reason: "unmapped_space"})
		await expect(run).rejects.toBeInstanceOf(GeoDbError)
	})
	it("declines empty when the system entity or its kind is not loaded, and never reads for an unsupported shape", async () => {
		const empty = deps(() => [])
		expect(await createSpaceResolver(empty.deps, "space")(context(`{ space(id: "${A}") { id } }`))).toEqual({
			ok: false,
			reason: "empty",
		})
		const noKind = deps(() => [{prop: KIND, raw: SYSTEM}])
		expect(await createSpaceResolver(noKind.deps, "space")(context(`{ space(id: "${A}") { type } }`))).toEqual({
			ok: false,
			reason: "empty",
		})
		const unsupported = deps(() => card(DAO))
		expect(await createSpaceResolver(unsupported.deps, "space")(context(`{ space(id: "${A}") { slug } }`))).toEqual(
			{
				ok: false,
				reason: "selection",
			},
		)
		expect(unsupported.readCard).not.toHaveBeenCalled()
	})
	it("declines a whole page when any Space is not served", async () => {
		const {deps: d} = deps((e) => (e === conformingId("entity", A) ? card(DAO) : []))
		const query = `{ spaces(filter: {id: {in: ["${A}", "${B}"]}}) { id } }`
		expect(await createSpaceResolver(d, "spaces")(context(query))).toEqual({ok: false, reason: "empty"})
	})
})

const GOV = "ab".repeat(16)
const GAIA_TOPIC = "7e57c0de7e57c0de7e57c0de7e57c0de"
const TOPIC = "0d4f1a2b3c4d4e5f8a6b7c8d9e0f1a2b"
const ADDRESS = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
const addressRecord = recordEntityId(SPACE_ADDRESS_KIND, [idPart(A)])
const topicRecord = recordEntityId(SPACE_TOPIC_KIND, [idPart(A)])

describe("keyed records: address and topicId", () => {
	const addressRows = (address: string | null, key = entityRef(A)): GeoCardRow[] => [
		{prop: SPACE_ADDRESS_COLUMNS.space, raw: key},
		...(address === null ? [] : [{prop: SPACE_ADDRESS_COLUMNS.address, raw: address}]),
	]
	const topicRows = (topic: string | null, key = entityRef(A)): GeoCardRow[] => [
		{prop: SPACE_TOPIC_COLUMNS.space, raw: key},
		...(topic === null ? [] : [{prop: SPACE_TOPIC_COLUMNS.topic, raw: topic}]),
	]
	// Reads of the Space's own card are answered by `own`, the governance Space's records by record id.
	const make = (records: Record<string, GeoCardRow[]>, extra: Record<string, unknown> = {}) => {
		const readCard = vi.fn(async ({geoSpaceId, entityId}: {geoSpaceId: string; entityId: string}) =>
			geoSpaceId === GOV ? (records[entityId] ?? []) : card(DAO),
		)
		const gaiaIdsOf = vi.fn(async (ids: readonly string[]) => new Map(ids.map((i) => [i, GAIA_TOPIC])))
		return {
			readCard,
			gaiaIdsOf,
			deps: {
				geoDb: {readCard},
				geoSpaceOf: async (s: string) => `geo-${s}`,
				systemSpaceOf: async () => GOV,
				gaiaIdsOf,
				...extra,
			},
		}
	}
	const run = (d: ReturnType<typeof make>["deps"], query: string, root: "space" | "spaces" = "space") =>
		createSpaceResolver(d, root)(context(query))

	it("derives the record ids from the feeder's pins", () => {
		const feeder = readFileSync(join(__dirname, "../../../../geodb-feeder/src/recordKinds.ts"), "utf8")
		for (const id of [
			SPACE_ADDRESS_KIND,
			SPACE_TOPIC_KIND,
			...Object.values(SPACE_ADDRESS_COLUMNS),
			...Object.values(SPACE_TOPIC_COLUMNS),
		])
			expect(feeder).toContain(id)
		expect(SPACE_ADDRESS_KIND).not.toBe(SPACE_TOPIC_KIND)
	})
	it("serves the address exactly as the record holds it, from the governance Space", async () => {
		const m = make({[addressRecord]: addressRows(ADDRESS)})
		const out = await run(m.deps, `{ space(id: "${A}") { id address } }`)
		expect(out).toEqual({ok: true, data: {space: {id: A, address: ADDRESS}}})
		expect(m.readCard).toHaveBeenCalledWith({geoSpaceId: GOV, entityId: addressRecord})
	})
	it("serves topicId through the reverse id map, and null when the record holds no topic", async () => {
		const m = make({[topicRecord]: topicRows(TOPIC)})
		expect(await run(m.deps, `{ space(id: "${A}") { topicId } }`)).toEqual({
			ok: true,
			data: {space: {topicId: GAIA_TOPIC}},
		})
		expect(m.gaiaIdsOf).toHaveBeenCalledWith([TOPIC])
		const none = make({[topicRecord]: topicRows(null)})
		expect(await run(none.deps, `{ space(id: "${A}") { topicId } }`)).toEqual({
			ok: true,
			data: {space: {topicId: null}},
		})
	})
	it("serves a page, one record read of each kind per Space", async () => {
		const bAddress = recordEntityId(SPACE_ADDRESS_KIND, [idPart(B)])
		const m = make({
			[addressRecord]: addressRows(ADDRESS),
			[bAddress]: addressRows(ADDRESS.toLowerCase(), entityRef(B)),
		})
		const out = await run(m.deps, `{ spaces(filter: {id: {in: ["${B}", "${A}"]}}) { id address } }`, "spaces")
		expect(out).toEqual({
			ok: true,
			data: {
				spaces: [
					{id: B, address: ADDRESS.toLowerCase()},
					{id: A, address: ADDRESS},
				],
			},
		})
	})
	it("declines rather than guesses", async () => {
		const q = `{ space(id: "${A}") { address } }`
		// No record: the feeder may lag or never have backfilled the Space.
		expect(await run(make({}).deps, q)).toEqual({ok: false, reason: "empty"})
		expect(await run(make({[topicRecord]: []}).deps, `{ space(id: "${A}") { topicId } }`)).toEqual({
			ok: false,
			reason: "empty",
		})
		// A record for another Space (key column differs), no address cell, or two different addresses.
		expect(await run(make({[addressRecord]: addressRows(ADDRESS, entityRef(B))}).deps, q)).toEqual({
			ok: false,
			reason: "empty",
		})
		expect(await run(make({[addressRecord]: addressRows(null)}).deps, q)).toEqual({ok: false, reason: "empty"})
		const two = [...addressRows(ADDRESS), {prop: SPACE_ADDRESS_COLUMNS.address, raw: "0x1"}]
		expect(await run(make({[addressRecord]: two}).deps, q)).toEqual({ok: false, reason: "empty"})
		// The governance Space is not founded in this process.
		const noGov = make({[addressRecord]: addressRows(ADDRESS)}, {systemSpaceOf: async () => null})
		expect(await run(noGov.deps, q)).toEqual({ok: false, reason: "not_enabled"})
		// A topic the reverse id map does not know is a legacy id geo-db holds under a hash.
		const legacy = make({[topicRecord]: topicRows(TOPIC)}, {gaiaIdsOf: async () => new Map()})
		expect(await run(legacy.deps, `{ space(id: "${A}") { topicId } }`)).toEqual({ok: false, reason: "legacy_id"})
		const noMap = make({[topicRecord]: topicRows(TOPIC)}, {gaiaIdsOf: undefined})
		expect(await run(noMap.deps, `{ space(id: "${A}") { topicId } }`)).toEqual({ok: false, reason: "legacy_id"})
	})
	it("does not read the governance Space when only id and type are asked", async () => {
		const m = make({})
		await run(m.deps, `{ space(id: "${A}") { id type } }`)
		expect(m.readCard).toHaveBeenCalledTimes(1)
	})
})
