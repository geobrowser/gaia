import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow} from "../card"
import {type GeoDbCardClient, GeoDbError} from "../client"
import {
	__resetProposalDiffShadowMetricsForTests,
	baseStatements,
	compareBase,
	createProposalDiffShadow,
	readProposalDiffShadowConfig,
	renderProposalDiffShadowMetrics,
	type ServedProposalDiff,
} from "../proposalDiffShadow"

const SPACE = "22222222222242228222222222222222"
const GEO_SPACE = "44444444444444448444444444444444"
const ENTITY = "11111111111141118111111111111111"
const PROP = "55555555555545558555555555555555"
const TYPE = "66666666666646668666666666666666"
const TARGET = "77777777777747778777777777777777"
const OTHER_SPACE = "88888888888848888888888888888888"

const served = (over: Partial<ServedProposalDiff> = {}): ServedProposalDiff => ({
	route: "proposal",
	spaceId: SPACE,
	mode: "active",
	entities: [
		{
			entityId: ENTITY,
			values: [
				{propertyId: PROP, spaceId: SPACE, before: "old"},
				{propertyId: PROP, spaceId: SPACE, before: null},
				{propertyId: TYPE, spaceId: OTHER_SPACE, before: "elsewhere"},
			],
			relations: [
				{typeId: TYPE, spaceId: SPACE, before: {toEntityId: TARGET}},
				{typeId: TYPE, spaceId: SPACE, before: null},
			],
		},
	],
	...over,
})

const geoRows = (over: GeoCardRow[] | null = null): GeoCardRow[] =>
	over ?? [
		{entity: conformingId("entity", ENTITY), prop: conformingId("entity", PROP), raw: "old"},
		{
			entity: conformingId("entity", ENTITY),
			prop: conformingId("entity", TYPE),
			raw: conformingId("entity", TARGET),
		},
	]

const count = (name: string, label: string, value: string) => {
	const m = renderProposalDiffShadowMetrics().match(new RegExp(`${name}\\{${label}="${value}"\\} (\\d+)`))
	return Number(m?.[1])
}
const result = (r: string) => count("gaia_api_proposal_diff_shadow_total", "result", r)
const statements = (o: string) => count("gaia_api_proposal_diff_shadow_statements_total", "outcome", o)
const unsupported = (r: string) => count("gaia_api_proposal_diff_shadow_unsupported_total", "reason", r)
const skipped = (r: string) => count("gaia_api_proposal_diff_shadow_skipped_total", "reason", r)
const based = (r: string) => count("gaia_api_proposal_diff_shadow_base_total", "resolved", r)
const settle = () => new Promise((r) => setTimeout(r, 0))

const make = (
	client: Partial<GeoDbCardClient>,
	config = {sampleEvery: 1},
	geoSpaceOf = async () => GEO_SPACE as string | null,
) => {
	const warn = vi.fn()
	const shadow = createProposalDiffShadow({
		config: {enabled: true, sampleEvery: config.sampleEvery, maxPerMinute: 1000, maxInFlight: 2},
		geoDb: client as GeoDbCardClient,
		geoSpaceOf,
		log: {warn},
	})
	return {shadow, warn}
}

beforeEach(() => __resetProposalDiffShadowMetricsForTests())

describe("readProposalDiffShadowConfig", () => {
	it("is off unless the flag says on", () => {
		expect(readProposalDiffShadowConfig({}).enabled).toBe(false)
		expect(readProposalDiffShadowConfig({GEODB_PROPOSAL_DIFF_SHADOW: "0"}).enabled).toBe(false)
		expect(readProposalDiffShadowConfig({GEODB_PROPOSAL_DIFF_SHADOW: "on"}).enabled).toBe(true)
	})
})

describe("baseStatements and compareBase (golden)", () => {
	it("takes only base-side, request-Space statements, after the id remap", () => {
		expect(baseStatements(served())).toEqual([
			{
				entityId: conformingId("entity", ENTITY),
				values: [conformingId("entity", PROP)],
				relations: [{prop: conformingId("entity", TYPE), target: conformingId("entity", TARGET)}],
			},
		])
	})

	it("counts found and missing base statements", () => {
		const base = baseStatements(served())
		expect(compareBase(base, geoRows())).toEqual({found: 2, missing: 0})
		expect(compareBase(base, [])).toEqual({found: 0, missing: 2})
		expect(compareBase(base, geoRows().slice(0, 1))).toEqual({found: 1, missing: 1})
	})
})

describe("createProposalDiffShadow", () => {
	it("counts a match when geo-db holds every base statement", async () => {
		const readCards = vi.fn(async () => geoRows())
		const {shadow, warn} = make({readCards})
		shadow.diff(served())
		await settle()
		expect(readCards).toHaveBeenCalledWith({geoSpaceIds: [GEO_SPACE], entityIds: [conformingId("entity", ENTITY)]})
		expect(result("match")).toBe(1)
		expect(statements("found")).toBe(2)
		expect(warn).not.toHaveBeenCalled()
	})

	it("counts a mismatch and logs counts without ids", async () => {
		const {shadow, warn} = make({readCards: async () => []})
		shadow.diff(served({route: "group"}))
		await settle()
		expect(result("mismatch")).toBe(1)
		expect(count("gaia_api_proposal_diff_shadow_mismatch_total", "field", "base_missing")).toBe(1)
		expect(statements("missing")).toBe(2)
		expect(warn).toHaveBeenCalledWith("proposal diff shadow mismatch", {
			route: "group",
			entities: 1,
			found: 0,
			missing: 2,
		})
	})

	it("reads an executed or closed base at the version before its timestamp", async () => {
		const readVersionAt = vi.fn(async () => ({seq: 7, timestampNs: "1", editId: ENTITY, authors: []}))
		const readCardsAt = vi.fn(async () => geoRows())
		const readCards = vi.fn(async () => geoRows())
		const {shadow} = make({readVersionAt, readCardsAt, readCards})
		shadow.diff(served({mode: "historical", baseTimestamp: 1_700_000_000n}))
		await settle()
		// strictly before the timestamp: the instant one nanosecond earlier
		expect(readVersionAt).toHaveBeenCalledWith({geoSpaceId: GEO_SPACE, instantNs: "1699999999999999999"})
		expect(readCardsAt).toHaveBeenCalledWith({
			geoSpaceId: GEO_SPACE,
			entityIds: [conformingId("entity", ENTITY)],
			seq: 7,
		})
		expect(readCards).not.toHaveBeenCalled()
		expect(result("match")).toBe(1)
		expect(statements("found")).toBe(2)
		expect(unsupported("as_of")).toBe(0)
	})

	describe("a base whose last edit's id is known", () => {
		const BASE_EDIT = "00000000000040008000000000000009"
		const historical = (over: Partial<ServedProposalDiff> = {}) =>
			served({mode: "historical", baseTimestamp: 1_700_000_000n, baseEditId: BASE_EDIT, ...over})
		const version = (seq: number) => ({seq, timestampNs: "1", editId: BASE_EDIT, authors: []})

		it("reads the version that edit sealed in place of resolving the timestamp", async () => {
			const readVersionOfEdit = vi.fn(async () => version(4))
			const readVersionAt = vi.fn(async () => version(7))
			const readCardsAt = vi.fn(async () => geoRows())
			const {shadow} = make({readVersionOfEdit, readVersionAt, readCardsAt})
			shadow.diff(historical())
			await settle()
			expect(readVersionOfEdit).toHaveBeenCalledWith({geoSpaceId: GEO_SPACE, editId: BASE_EDIT})
			expect(readVersionAt).not.toHaveBeenCalled()
			expect(readCardsAt).toHaveBeenCalledWith({
				geoSpaceId: GEO_SPACE,
				entityIds: [conformingId("entity", ENTITY)],
				seq: 4,
			})
			expect(result("match")).toBe(1)
			expect(based("edit_id")).toBe(1)
			expect(based("timestamp")).toBe(0)
		})

		it("falls back to the timestamp where the Space holds no version of that edit", async () => {
			const readVersionOfEdit = vi.fn(async () => null)
			const readVersionAt = vi.fn(async () => version(7))
			const readCardsAt = vi.fn(async () => geoRows())
			const {shadow} = make({readVersionOfEdit, readVersionAt, readCardsAt})
			shadow.diff(historical())
			await settle()
			expect(readVersionAt).toHaveBeenCalledWith({geoSpaceId: GEO_SPACE, instantNs: "1699999999999999999"})
			expect(readCardsAt).toHaveBeenCalledWith(expect.objectContaining({seq: 7}))
			expect(result("match")).toBe(1)
			expect(based("edit_miss")).toBe(1)
			expect(based("edit_id")).toBe(0)
		})

		it("falls back where the client or the node cannot answer the read, but not on a timeout", async () => {
			const readVersionAt = vi.fn(async () => version(7))
			const readCardsAt = vi.fn(async () => geoRows())
			const noRead = make({readVersionAt, readCardsAt})
			noRead.shadow.diff(historical())
			const refused = make({
				readVersionOfEdit: async () => {
					throw new GeoDbError("client_failed", "UnknownOpcode")
				},
				readVersionAt,
				readCardsAt,
			})
			refused.shadow.diff(historical())
			await settle()
			expect(readVersionAt).toHaveBeenCalledTimes(2)
			expect(based("edit_unsupported")).toBe(2)
			expect(result("match")).toBe(2)

			const slow = make({
				readVersionOfEdit: async () => {
					throw new GeoDbError("timeout", "slow")
				},
				readVersionAt,
				readCardsAt,
			})
			slow.shadow.diff(historical())
			await settle()
			expect(readVersionAt).toHaveBeenCalledTimes(2)
			expect(result("failed")).toBe(1)
		})

		it("counts a base with no edit id as resolved by the timestamp", async () => {
			const readVersionOfEdit = vi.fn(async () => version(4))
			const {shadow} = make({
				readVersionOfEdit,
				readVersionAt: async () => version(7),
				readCardsAt: async () => geoRows(),
			})
			shadow.diff(historical({baseEditId: undefined}))
			await settle()
			expect(readVersionOfEdit).not.toHaveBeenCalled()
			expect(based("timestamp")).toBe(1)
			expect(based("edit_id")).toBe(0)
		})
	})

	it("counts a historical base statement the version lacks as a mismatch", async () => {
		const {shadow} = make({
			readVersionAt: async () => ({seq: 3, timestampNs: "1", editId: ENTITY, authors: []}),
			readCardsAt: async () => [],
		})
		shadow.diff(served({mode: "historical", baseTimestamp: 5n}))
		await settle()
		expect(result("mismatch")).toBe(1)
		expect(statements("missing")).toBe(2)
	})

	it("fails, without reading cards, when the version cannot be named", async () => {
		const readCardsAt = vi.fn(async () => geoRows())
		const {shadow} = make({
			readVersionAt: async () => {
				throw new GeoDbError("client_failed", "refused")
			},
			readCardsAt,
		})
		shadow.diff(served({mode: "historical", baseTimestamp: 5n}))
		await settle()
		expect(readCardsAt).not.toHaveBeenCalled()
		expect(result("failed")).toBe(1)
	})

	it("leaves a historical diff with no base timestamp, or a client with no at-version read, unsupported", async () => {
		const readCards = vi.fn(async () => geoRows())
		const {shadow} = make({readCards})
		shadow.diff(served({mode: "historical"}))
		shadow.diff(served({mode: "historical", baseTimestamp: 5n}))
		await settle()
		expect(readCards).not.toHaveBeenCalled()
		expect(unsupported("as_of")).toBe(1)
		expect(unsupported("client")).toBe(1)
	})

	it("reports a page with nothing on the base side, an unmapped Space and an old client", async () => {
		const {shadow} = make({readCards: async () => geoRows()})
		shadow.diff(
			served({
				entities: [
					{entityId: ENTITY, values: [{propertyId: PROP, spaceId: SPACE, before: null}], relations: []},
				],
			}),
		)
		expect(unsupported("no_base")).toBe(1)
		const unmapped = make({readCards: async () => geoRows()}, {sampleEvery: 1}, async () => null)
		unmapped.shadow.diff(served())
		await settle()
		expect(unsupported("unmapped_space")).toBe(1)
		make({}).shadow.diff(served())
		await settle()
		expect(unsupported("client")).toBe(1)
	})

	it("counts a failed read and a busy pool, and samples", async () => {
		const {shadow} = make({
			readCards: async () => {
				throw new GeoDbError("timeout", "slow")
			},
		})
		shadow.diff(served())
		await settle()
		expect(result("failed")).toBe(1)
		const busy = make({
			readCards: async () => {
				throw new GeoDbError("overloaded", "full")
			},
		})
		busy.shadow.diff(served())
		await settle()
		expect(unsupported("busy")).toBe(1)
		const sampled = make({readCards: async () => geoRows()}, {sampleEvery: 2})
		sampled.shadow.diff(served())
		sampled.shadow.diff(served())
		await settle()
		expect(skipped("sample")).toBe(1)
	})

	it("is silent when the flag is off", () => {
		const shadow = createProposalDiffShadow({
			config: {enabled: false, sampleEvery: 1, maxPerMinute: 1, maxInFlight: 1},
			geoDb: {} as GeoDbCardClient,
			geoSpaceOf: async () => GEO_SPACE,
			log: {warn: vi.fn()},
		})
		shadow.diff(served())
		expect(result("match") + result("mismatch")).toBe(0)
	})
})
