import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow, type GeoVersionRow} from "../card"
import {type GeoDbCardClient, GeoDbError} from "../client"
import {
	__resetEntityDiffShadowMetricsForTests,
	afterStatements,
	compareAfter,
	createEntityDiffShadow,
	readEntityDiffShadowConfig,
	renderEntityDiffShadowMetrics,
	type ServedEntityDiff,
	type ServedReviewDiff,
} from "../entityDiffShadow"

const SPACE = "22222222-2222-4222-8222-222222222222"
const GEO_SPACE = "44444444444444448444444444444444"
const ENTITY = "11111111-1111-4111-8111-111111111111"
const PROP = "55555555-5555-4555-8555-555555555555"
const TYPE = "66666666-6666-4666-8666-666666666666"
const TARGET = "77777777-7777-4777-8777-777777777777"
const OTHER_SPACE = "88888888-8888-4888-8888-888888888888"
const EDIT = "99999999-9999-4999-8999-999999999999"
const REL_ADDED = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
const REL_REMOVED = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
const OLDER_EDIT = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"

const cid = (id: string) => conformingId("entity", id)

const served = (over: Partial<ServedEntityDiff> = {}): ServedEntityDiff => ({
	route: "entity_diff",
	spaceId: SPACE,
	entityId: ENTITY,
	toEditId: EDIT,
	values: [
		{propertyId: PROP, spaceId: SPACE, before: "old", after: "new"},
		{propertyId: TYPE, spaceId: SPACE, before: "gone", after: null},
		{propertyId: TYPE, spaceId: OTHER_SPACE, before: null, after: "elsewhere"},
	],
	relations: [
		{relationId: REL_ADDED, typeId: TYPE, spaceId: SPACE, before: null, after: {toEntityId: TARGET}},
		{relationId: REL_REMOVED, typeId: TYPE, spaceId: SPACE, before: {toEntityId: TARGET}, after: null},
	],
	...over,
})

const review = (over: Partial<ServedReviewDiff> = {}): ServedReviewDiff => ({
	route: "review",
	spaceId: SPACE,
	entities: [
		{
			entityId: ENTITY,
			values: [{propertyId: PROP, spaceId: SPACE, before: "old"}],
			relations: [{typeId: TYPE, spaceId: SPACE, before: {toEntityId: TARGET}}],
		},
	],
	...over,
})

const card = (): GeoCardRow[] => [
	{entity: cid(ENTITY), prop: cid(PROP), raw: "new"},
	{entity: cid(ENTITY), prop: cid(TYPE), raw: cid(TARGET)},
]

const version = (editId: string, seq: number): GeoVersionRow =>
	({space: GEO_SPACE, seq, timestampNs: "1", editId, authors: []}) as unknown as GeoVersionRow

const count = (name: string, labels: Record<string, string>) => {
	const sel = Object.entries(labels)
		.map(([k, v]) => `${k}="${v}"`)
		.join(",")
	const m = renderEntityDiffShadowMetrics().match(new RegExp(`${name}\\{${sel}\\} (\\d+)`))
	return Number(m?.[1])
}
const result = (route: string, r: string) => count("gaia_api_entity_diff_shadow_total", {route, result: r})
const outcome = (route: string, o: string) => count("gaia_api_entity_diff_shadow_statements_total", {route, outcome: o})
const unsupported = (route: string, r: string) =>
	count("gaia_api_entity_diff_shadow_unsupported_total", {route, reason: r})
const skipped = (r: string) => count("gaia_api_entity_diff_shadow_skipped_total", {reason: r})
const settle = () => new Promise((r) => setTimeout(r, 0))

const make = (
	client: Partial<GeoDbCardClient>,
	config = {sampleEvery: 1},
	geoSpaceOf = async () => GEO_SPACE as string | null,
) => {
	const warn = vi.fn()
	const shadow = createEntityDiffShadow({
		config: {enabled: true, sampleEvery: config.sampleEvery, maxPerMinute: 1000, maxInFlight: 2},
		geoDb: client as GeoDbCardClient,
		geoSpaceOf,
		log: {warn},
	})
	return {shadow, warn}
}

const head =
	(editId = EDIT) =>
	async () => [version(editId, 2), version(OLDER_EDIT, 1)]

beforeEach(() => __resetEntityDiffShadowMetricsForTests())

describe("readEntityDiffShadowConfig", () => {
	it("is off unless the flag says on, and clamps the tunables", () => {
		expect(readEntityDiffShadowConfig({}).enabled).toBe(false)
		expect(readEntityDiffShadowConfig({GEODB_ENTITY_DIFF_SHADOW: "0"}).enabled).toBe(false)
		const on = readEntityDiffShadowConfig({
			GEODB_ENTITY_DIFF_SHADOW: "on",
			GEODB_ENTITY_DIFF_SHADOW_SAMPLE_EVERY: "x",
		})
		expect(on).toEqual({enabled: true, sampleEvery: 10, maxPerMinute: 30, maxInFlight: 2})
	})
})

describe("afterStatements and compareAfter (golden)", () => {
	it("takes the after side in the request's Space: values and relations the diff leaves in place", () => {
		expect(afterStatements(served())).toEqual({
			entityId: cid(ENTITY),
			values: [cid(PROP)],
			relations: [{prop: cid(TYPE), target: cid(TARGET)}],
			removedRelations: [cid(REL_REMOVED)],
		})
	})

	it("counts found and missing after statements against the card", () => {
		const after = afterStatements(served())
		expect(compareAfter(after, card())).toEqual({found: 2, missing: 0})
		expect(compareAfter(after, [])).toEqual({found: 0, missing: 2})
		expect(compareAfter(after, card().slice(0, 1))).toEqual({found: 1, missing: 1})
	})
})

describe("createEntityDiffShadow: entity diff", () => {
	it("counts a match when toEditId is geo-db's newest version and the card holds the after side", async () => {
		const readEntityVersions = vi.fn(head())
		const readCard = vi.fn(async () => card())
		const {shadow, warn} = make({readEntityVersions, readCard})
		shadow.entityDiff(served())
		await settle()
		expect(readEntityVersions).toHaveBeenCalledWith({entityId: cid(ENTITY), first: 100})
		expect(readCard).toHaveBeenCalledWith({geoSpaceId: GEO_SPACE, entityId: cid(ENTITY)})
		expect(result("entity_diff", "match")).toBe(1)
		expect(outcome("entity_diff", "found")).toBe(2)
		expect(warn).not.toHaveBeenCalled()
	})

	it("counts a mismatch and logs counts without ids", async () => {
		const {shadow, warn} = make({readEntityVersions: head(), readCard: async () => []})
		shadow.entityDiff(served())
		await settle()
		expect(result("entity_diff", "mismatch")).toBe(1)
		expect(
			count("gaia_api_entity_diff_shadow_mismatch_total", {route: "entity_diff", field: "after_missing"}),
		).toBe(1)
		expect(outcome("entity_diff", "missing")).toBe(2)
		expect(warn).toHaveBeenCalledWith("entity diff shadow mismatch", {
			route: "entity_diff",
			entities: 1,
			found: 0,
			missing: 2,
		})
	})

	it("does not compare a diff whose to-edit is not the newest version", async () => {
		const readCard = vi.fn(async () => card())
		const {shadow} = make({readEntityVersions: head(OLDER_EDIT), readCard})
		shadow.entityDiff(served())
		await settle()
		expect(readCard).not.toHaveBeenCalled()
		expect(unsupported("entity_diff", "not_head")).toBe(1)
		const empty = make({readEntityVersions: async () => [], readCard})
		empty.shadow.entityDiff(served())
		await settle()
		expect(unsupported("entity_diff", "not_head")).toBe(2)
	})

	it("reports a diff with nothing on the after side, an unmapped Space and an old client", async () => {
		const {shadow} = make({readEntityVersions: head(), readCard: async () => card()})
		shadow.entityDiff(
			served({values: [{propertyId: PROP, spaceId: SPACE, before: "old", after: null}], relations: []}),
		)
		expect(unsupported("entity_diff", "no_after")).toBe(1)
		const unmapped = make(
			{readEntityVersions: head(), readCard: async () => card()},
			{sampleEvery: 1},
			async () => null,
		)
		unmapped.shadow.entityDiff(served())
		await settle()
		expect(unsupported("entity_diff", "unmapped_space")).toBe(1)
		make({}).shadow.entityDiff(served())
		await settle()
		expect(unsupported("entity_diff", "client")).toBe(1)
	})

	it("counts a failed read and a busy pool, and samples", async () => {
		const {shadow} = make({
			readEntityVersions: async () => {
				throw new GeoDbError("timeout", "slow")
			},
			readCard: async () => card(),
		})
		shadow.entityDiff(served())
		await settle()
		expect(result("entity_diff", "failed")).toBe(1)
		const busy = make({
			readEntityVersions: async () => {
				throw new GeoDbError("overloaded", "full")
			},
			readCard: async () => card(),
		})
		busy.shadow.entityDiff(served())
		await settle()
		expect(unsupported("entity_diff", "busy")).toBe(1)
		const sampled = make({readEntityVersions: head(), readCard: async () => card()}, {sampleEvery: 2})
		sampled.shadow.entityDiff(served())
		sampled.shadow.entityDiff(served())
		await settle()
		expect(skipped("sample")).toBe(1)
	})

	it("is silent when the flag is off", () => {
		const shadow = createEntityDiffShadow({
			config: {enabled: false, sampleEvery: 1, maxPerMinute: 1, maxInFlight: 1},
			geoDb: {} as GeoDbCardClient,
			geoSpaceOf: async () => GEO_SPACE,
			log: {warn: vi.fn()},
		})
		shadow.entityDiff(served())
		shadow.review(review())
		expect(renderEntityDiffShadowMetrics()).not.toContain('result="match"} 1')
	})
})

describe("createEntityDiffShadow: removed relations", () => {
	const removed = (relations: ServedEntityDiff["relations"]) => served({values: [], relations})
	const gone = {relationId: REL_REMOVED, typeId: TYPE, spaceId: SPACE, before: {toEntityId: TARGET}, after: null}

	it("counts a removed relation closed in geo-db (an empty page) as found", async () => {
		const readRelationEntity = vi.fn(async () => [] as GeoCardRow[])
		const {shadow} = make({readEntityVersions: head(), readCard: async () => card(), readRelationEntity})
		shadow.entityDiff(served())
		await settle()
		expect(readRelationEntity).toHaveBeenCalledWith({relationId: cid(REL_REMOVED)})
		expect(result("entity_diff", "match")).toBe(1)
		expect(outcome("entity_diff", "found")).toBe(3)
	})

	it("counts a removed relation geo-db still holds open as a removed_open mismatch", async () => {
		const readRelationEntity = async () => [{entity: cid(ENTITY), prop: "", raw: ""}] as GeoCardRow[]
		const {shadow, warn} = make({readEntityVersions: head(), readCard: async () => card(), readRelationEntity})
		shadow.entityDiff(served())
		await settle()
		expect(result("entity_diff", "mismatch")).toBe(1)
		expect(count("gaia_api_entity_diff_shadow_mismatch_total", {route: "entity_diff", field: "removed_open"})).toBe(
			1,
		)
		expect(outcome("entity_diff", "missing")).toBe(1)
		expect(warn).toHaveBeenCalledWith("entity diff shadow mismatch", {
			route: "entity_diff",
			entities: 1,
			found: 2,
			missing: 1,
		})
	})

	it("compares a diff of only removed relations, and skips them when the client cannot read relations", async () => {
		const {shadow} = make({
			readEntityVersions: head(),
			readCard: async () => card(),
			readRelationEntity: async () => [],
		})
		shadow.entityDiff(removed([gone]))
		await settle()
		expect(result("entity_diff", "match")).toBe(1)
		const old = make({readEntityVersions: head(), readCard: async () => card()})
		old.shadow.entityDiff(removed([gone]))
		await settle()
		expect(unsupported("entity_diff", "client")).toBe(1)
	})

	it("counts a failed relation read as failed", async () => {
		const {shadow} = make({
			readEntityVersions: head(),
			readCard: async () => card(),
			readRelationEntity: async () => {
				throw new GeoDbError("timeout", "slow")
			},
		})
		shadow.entityDiff(served())
		await settle()
		expect(result("entity_diff", "failed")).toBe(1)
	})
})

describe("createEntityDiffShadow: review", () => {
	it("counts a match when geo-db holds every base statement of the live graph", async () => {
		const readCards = vi.fn(async () => card())
		const {shadow} = make({readCards})
		shadow.review(review())
		await settle()
		expect(readCards).toHaveBeenCalledWith({geoSpaceIds: [GEO_SPACE], entityIds: [cid(ENTITY)]})
		expect(result("review", "match")).toBe(1)
		expect(outcome("review", "found")).toBe(2)
	})

	it("counts a mismatch, and reports a page with no base side", async () => {
		const {shadow, warn} = make({readCards: async () => []})
		shadow.review(review())
		await settle()
		expect(result("review", "mismatch")).toBe(1)
		expect(count("gaia_api_entity_diff_shadow_mismatch_total", {route: "review", field: "base_missing"})).toBe(1)
		expect(warn).toHaveBeenCalledWith("entity diff shadow mismatch", {
			route: "review",
			entities: 1,
			found: 0,
			missing: 2,
		})
		shadow.review(
			review({
				entities: [
					{entityId: ENTITY, values: [{propertyId: PROP, spaceId: SPACE, before: null}], relations: []},
				],
			}),
		)
		expect(unsupported("review", "no_base")).toBe(1)
	})
})

describe("metrics", () => {
	it("exports nothing until the shadow is mounted, then every series at zero", () => {
		expect(renderEntityDiffShadowMetrics()).toBe("")
		make({})
		const text = renderEntityDiffShadowMetrics()
		expect(text).toContain('gaia_api_entity_diff_shadow_total{route="entity_diff",result="match"} 0')
		expect(text).toContain('gaia_api_entity_diff_shadow_total{route="review",result="failed"} 0')
		expect(text).toContain('gaia_api_entity_diff_shadow_skipped_total{reason="rate"} 0')
	})
})
