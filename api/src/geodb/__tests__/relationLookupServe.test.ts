import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow} from "../card"
import type {GeoDbCardClient} from "../client"
import {
	__resetRelationLookupShadowMetricsForTests,
	createRelationLookupServeWithoutShadow,
	createRelationLookupShadow,
	MAX_RELATION_IDS,
} from "../relationLookupShadow"
import {__resetServeGateForTests, renderServeGateMetrics} from "../serveGate"

const REL_A = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
const REL_B = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
const REL_CLOSED = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
const FROM_A = "11111111-1111-4111-8111-111111111111"
const FROM_B = "22222222-2222-4222-8222-222222222222"

const compact = (id: string) => id.replaceAll("-", "")
const fromRow = (from: string): GeoCardRow[] => [{prop: "p", raw: "r", entity: conformingId("entity", from)}]
const geoAnswers = (table: Record<string, string>) =>
	vi.fn(async ({relationId}: {relationId: string}) => {
		const from = table[relationId]
		return from ? fromRow(from) : []
	})
const settle = () => new Promise((r) => setTimeout(r, 0))
const fallback = (reason: string) => {
	const m = renderServeGateMetrics().match(
		new RegExp(`serve_fallback_total\\{family="versions",read="relation_lookup",reason="${reason}"\\} (\\d+)`),
	)
	return Number(m?.[1])
}
const served = () =>
	Number(renderServeGateMetrics().match(/serve_total\{family="versions",read="relation_lookup"\} (\d+)/)?.[1])

const make = (client: Partial<GeoDbCardClient>, serveOn = true, ttl = 60_000, timeoutMs = 500) =>
	createRelationLookupShadow({
		config: {enabled: true, sampleEvery: 1, maxPerMinute: 1000, maxInFlight: 4, concurrency: 8},
		geoDb: client as GeoDbCardClient,
		log: {warn: vi.fn()},
		serve: {enabled: serveOn, matchTtlMs: ttl, timeoutMs},
	})

const pg = new Map([
	[REL_A, FROM_A],
	[REL_B, FROM_B],
])
const table = {[compact(REL_A)]: FROM_A, [compact(REL_B)]: FROM_B}

beforeEach(() => {
	__resetRelationLookupShadowMetricsForTests()
	__resetServeGateForTests()
})

describe("relation lookup serve", () => {
	it("exports no serve series and serves nothing while the switch is off", async () => {
		const shadow = make({readRelationEntity: geoAnswers(table)}, false)
		shadow.lookup([REL_A], pg)
		await settle()
		expect(await shadow.serve([REL_A])).toBeNull()
		expect(renderServeGateMetrics()).toBe("")
	})

	it("serves the map Postgres would have built, once every id's last comparison matched", async () => {
		const shadow = make({readRelationEntity: geoAnswers(table)})
		shadow.lookup([REL_A, REL_B], pg)
		await settle()
		const got = await shadow.serve([REL_A, REL_B])
		// Postgres holds the from-entity in dashed form; the served map must be what Postgres's map is, id for id, after the
		// caller's own normalisation (proposal-diff compacts it).
		expect(got).not.toBeNull()
		for (const [id, from] of pg) expect(compact(got?.get(id) ?? "")).toBe(compact(from))
		expect(served()).toBe(1)
	})

	it("falls back no_comparison for a relation the shadow has not seen, all-or-nothing", async () => {
		const read = geoAnswers(table)
		const shadow = make({readRelationEntity: read})
		shadow.lookup([REL_A], new Map([[REL_A, FROM_A]]))
		await settle()
		read.mockClear()
		expect(await shadow.serve([REL_A, REL_B])).toBeNull()
		expect(read).not.toHaveBeenCalled()
		expect(fallback("no_comparison")).toBe(1)
	})

	it("falls back on a mismatch, and on a closed relation that could open later", async () => {
		const shadow = make({readRelationEntity: geoAnswers({[compact(REL_A)]: FROM_B})})
		shadow.lookup([REL_A], new Map([[REL_A, FROM_A]]))
		await settle()
		expect(await shadow.serve([REL_A])).toBeNull()
		expect(fallback("mismatch")).toBe(1)

		const closed = make({readRelationEntity: geoAnswers({})})
		closed.lookup([REL_CLOSED], new Map())
		await settle()
		// matched (both absent), but not exact: serving would answer from geo-db for a relation Postgres calls closed
		const got = await closed.serve([REL_CLOSED])
		expect(got === null || got.size === 0).toBe(true)
	})

	it("falls back unsupported when the comparison failed, and past the id cap", async () => {
		const shadow = make({
			readRelationEntity: vi.fn(async () => {
				throw new Error("down")
			}),
		})
		shadow.lookup([REL_A], new Map([[REL_A, FROM_A]]))
		await settle()
		expect(await shadow.serve([REL_A])).toBeNull()
		expect(fallback("unsupported")).toBe(1)
		const many = Array.from({length: MAX_RELATION_IDS + 1}, (_, i) => `${i}`)
		expect(await shadow.serve(many)).toBeNull()
		expect(fallback("unsupported")).toBe(2)
	})

	it("falls back stale once the match is older than the TTL", async () => {
		const shadow = make({readRelationEntity: geoAnswers(table)}, true, 1)
		shadow.lookup([REL_A], new Map([[REL_A, FROM_A]]))
		await settle()
		await new Promise((r) => setTimeout(r, 10))
		expect(await shadow.serve([REL_A])).toBeNull()
		expect(fallback("stale")).toBe(1)
	})

	it("falls back error or timeout when the serving read fails or is slow", async () => {
		let mode: "ok" | "throw" | "hang" = "ok"
		const read = vi.fn(async ({relationId}: {relationId: string}) => {
			if (mode === "throw") throw new Error("boom")
			if (mode === "hang") return new Promise<GeoCardRow[]>(() => {})
			return table[relationId] ? fromRow(table[relationId]) : []
		})
		const shadow = make({readRelationEntity: read}, true, 60_000, 20)
		shadow.lookup([REL_A], new Map([[REL_A, FROM_A]]))
		await settle()
		mode = "throw"
		expect(await shadow.serve([REL_A])).toBeNull()
		expect(fallback("error")).toBe(1)
		mode = "hang"
		expect(await shadow.serve([REL_A])).toBeNull()
		expect(fallback("timeout")).toBe(1)
	})

	it("falls back differs when geo-db's id is one the loader remapped", async () => {
		const LEGACY = "33333333-3333-3333-3333-333333333333"
		const shadow = make({readRelationEntity: geoAnswers({[compact(REL_A)]: LEGACY})})
		shadow.lookup([REL_A], new Map([[REL_A, LEGACY]]))
		await settle()
		const got = await shadow.serve([REL_A])
		if (conformingId("entity", LEGACY) !== compact(LEGACY)) {
			expect(got).toBeNull()
			expect(fallback("differs")).toBe(1)
		} else expect(got).not.toBeNull()
	})

	it("counts shadow_off when the family has no shadow", async () => {
		const serve = createRelationLookupServeWithoutShadow()
		expect(await serve([REL_A])).toBeNull()
		expect(fallback("shadow_off")).toBe(1)
	})
})
