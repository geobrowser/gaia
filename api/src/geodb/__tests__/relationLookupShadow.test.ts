import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow} from "../card"
import {type GeoDbCardClient, GeoDbError} from "../client"
import {
	__resetRelationLookupShadowMetricsForTests,
	createRelationLookupShadow,
	MAX_RELATION_IDS,
	readRelationLookupShadowConfig,
	renderRelationLookupShadowMetrics,
} from "../relationLookupShadow"

const REL_A = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
const REL_B = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
const REL_CLOSED = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
const FROM_A = "11111111-1111-4111-8111-111111111111"
const FROM_B = "22222222-2222-4222-8222-222222222222"

const compact = (id: string) => id.replaceAll("-", "")
const fromRow = (from: string): GeoCardRow[] => [{prop: "p", raw: "r", entity: conformingId("entity", from)}]

/** geo-db's answers by (compact) relation id; an id not in the table is a closed or unknown relation: an empty page. */
const geoAnswers = (table: Record<string, string>) =>
	vi.fn(async ({relationId}: {relationId: string}) => {
		const from = table[relationId]
		return from ? fromRow(from) : []
	})

const count = (name: string, label: string, value: string) => {
	const m = renderRelationLookupShadowMetrics().match(new RegExp(`${name}\\{${label}="${value}"\\} (\\d+)`))
	return Number(m?.[1])
}
const result = (r: string) => count("gaia_api_relation_lookup_shadow_total", "result", r)
const relations = (o: string) => count("gaia_api_relation_lookup_shadow_relations_total", "outcome", o)
const unsupported = (r: string) => count("gaia_api_relation_lookup_shadow_unsupported_total", "reason", r)
const skipped = (r: string) => count("gaia_api_relation_lookup_shadow_skipped_total", "reason", r)
const settle = () => new Promise((r) => setTimeout(r, 0))

const make = (client: Partial<GeoDbCardClient>, over: {sampleEvery?: number; maxInFlight?: number} = {}) => {
	const warn = vi.fn()
	const shadow = createRelationLookupShadow({
		config: {
			enabled: true,
			sampleEvery: over.sampleEvery ?? 1,
			maxPerMinute: 1000,
			maxInFlight: over.maxInFlight ?? 2,
			concurrency: 8,
		},
		geoDb: client as GeoDbCardClient,
		log: {warn},
	})
	return {shadow, warn}
}

beforeEach(() => __resetRelationLookupShadowMetricsForTests())

describe("readRelationLookupShadowConfig", () => {
	it("is off unless the flag says on, and defaults the concurrency to 8", () => {
		expect(readRelationLookupShadowConfig({}).enabled).toBe(false)
		expect(readRelationLookupShadowConfig({GEODB_RELATION_LOOKUP_SHADOW: "0"}).enabled).toBe(false)
		const on = readRelationLookupShadowConfig({GEODB_RELATION_LOOKUP_SHADOW: "on"})
		expect(on.enabled).toBe(true)
		expect(on.concurrency).toBe(8)
		expect(readRelationLookupShadowConfig({GEODB_RELATION_LOOKUP_SHADOW_CONCURRENCY: "99"}).concurrency).toBe(16)
	})
	it("exports nothing until the shadow is mounted", () => {
		expect(renderRelationLookupShadowMetrics()).toBe("")
	})
})

describe("createRelationLookupShadow", () => {
	it("asks geo-db once per relation id and counts a match when every from-entity agrees", async () => {
		const readRelationEntity = geoAnswers({[compact(REL_A)]: FROM_A, [compact(REL_B)]: FROM_B})
		const {shadow, warn} = make({readRelationEntity})
		shadow.lookup(
			[REL_A, REL_B],
			new Map([
				[REL_A, FROM_A],
				[REL_B, FROM_B],
			]),
		)
		await settle()
		expect(readRelationEntity).toHaveBeenCalledTimes(2)
		expect(readRelationEntity).toHaveBeenCalledWith({relationId: conformingId("entity", REL_A)})
		expect(result("match")).toBe(1)
		expect(relations("match")).toBe(2)
		expect(warn).not.toHaveBeenCalled()
	})

	it("matches a closed relation: Postgres (live table) and geo-db both hold nothing", async () => {
		const readRelationEntity = geoAnswers({[compact(REL_A)]: FROM_A})
		const {shadow} = make({readRelationEntity})
		shadow.lookup([REL_A, REL_CLOSED], new Map([[REL_A, FROM_A]]))
		await settle()
		expect(result("match")).toBe(1)
		expect(relations("match")).toBe(2)
		expect(relations("mismatch")).toBe(0)
	})

	it("counts a mismatch by kind and logs counts without ids", async () => {
		const readRelationEntity = geoAnswers({[compact(REL_A)]: FROM_B, [compact(REL_CLOSED)]: FROM_A})
		const {shadow, warn} = make({readRelationEntity})
		// A: different from-entity. B: Postgres has it, geo-db does not. CLOSED: geo-db has it, Postgres does not.
		shadow.lookup(
			[REL_A, REL_B, REL_CLOSED],
			new Map([
				[REL_A, FROM_A],
				[REL_B, FROM_B],
			]),
		)
		await settle()
		expect(result("mismatch")).toBe(1)
		expect(relations("mismatch")).toBe(3)
		expect(count("gaia_api_relation_lookup_shadow_mismatch_total", "field", "from_entity")).toBe(1)
		expect(count("gaia_api_relation_lookup_shadow_mismatch_total", "field", "missing_in_geodb")).toBe(1)
		expect(count("gaia_api_relation_lookup_shadow_mismatch_total", "field", "extra_in_geodb")).toBe(1)
		expect(warn).toHaveBeenCalledWith("relation lookup shadow mismatch", {relations: 3, matched: 0, mismatched: 3})
		expect(JSON.stringify(warn.mock.calls)).not.toContain(compact(REL_A))
	})

	it("counts a failed read, never throws, and never calls back into the served map", async () => {
		const {shadow} = make({
			readRelationEntity: async () => {
				throw new GeoDbError("timeout", "slow")
			},
		})
		const served = new Map([[REL_A, FROM_A]])
		expect(() => shadow.lookup([REL_A], served)).not.toThrow()
		await settle()
		expect(result("failed")).toBe(1)
		expect(relations("failed")).toBe(1)
		expect([...served]).toEqual([[REL_A, FROM_A]])
	})

	it("keeps at most `concurrency` reads in flight", async () => {
		let live = 0
		let peak = 0
		const readRelationEntity = vi.fn(async () => {
			live++
			peak = Math.max(peak, live)
			await new Promise((r) => setTimeout(r, 1))
			live--
			return []
		})
		const {shadow} = make({readRelationEntity})
		const ids = Array.from({length: 30}, (_, i) => `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`)
		shadow.lookup(ids, new Map())
		await new Promise((r) => setTimeout(r, 50))
		expect(readRelationEntity).toHaveBeenCalledTimes(30)
		expect(peak).toBe(8)
		expect(result("match")).toBe(1)
	})

	it("is unsupported on a client without the method, and past the id cap", async () => {
		const {shadow} = make({})
		shadow.lookup([REL_A], new Map())
		expect(unsupported("client")).toBe(1)
		const {shadow: capped} = make({readRelationEntity: geoAnswers({})})
		capped.lookup(
			Array.from(
				{length: MAX_RELATION_IDS + 1},
				(_, i) => `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`,
			),
			new Map(),
		)
		expect(unsupported("too_many")).toBe(1)
	})

	it("samples, and does nothing when off", async () => {
		const readRelationEntity = geoAnswers({})
		const {shadow} = make({readRelationEntity}, {sampleEvery: 2})
		shadow.lookup([REL_A], new Map())
		shadow.lookup([REL_A], new Map())
		await settle()
		expect(readRelationEntity).toHaveBeenCalledTimes(1)
		expect(skipped("sample")).toBe(1)
		const off = createRelationLookupShadow({
			config: {enabled: false, sampleEvery: 1, maxPerMinute: 1, maxInFlight: 1, concurrency: 8},
			geoDb: {readRelationEntity} as unknown as GeoDbCardClient,
			log: {warn: vi.fn()},
		})
		off.lookup([REL_A], new Map())
		await settle()
		expect(readRelationEntity).toHaveBeenCalledTimes(1)
	})

	it("an empty id list is not compared", async () => {
		const readRelationEntity = geoAnswers({})
		const {shadow} = make({readRelationEntity})
		shadow.lookup([], new Map())
		await settle()
		expect(readRelationEntity).not.toHaveBeenCalled()
		expect(result("match")).toBe(0)
	})
})
