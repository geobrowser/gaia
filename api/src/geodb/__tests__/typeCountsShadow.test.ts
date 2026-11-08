import {beforeEach, describe, expect, it, vi} from "vitest"
import {GeoDbError} from "../client"
import type {DerivedCountsReader} from "../derived"
import {CLAIM_TYPE, DEBATE_WALK_TYPE} from "../rankedReads/feeds"
import {planTypeCounts, type TypeCountsCall, typeCountsFrom} from "../rankedReads/typeCounts"
import {
	__resetTypeCountsShadowMetricsForTests,
	createTypeCountsShadow,
	readTypeCountsShadowConfig,
	renderTypeCountsShadowMetrics,
	type TypeCountsSource,
} from "../typeCountsShadow"

const TOPIC = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
const OTHER = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
const NEWS = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
const ABSENT = "00000000-0000-4000-8000-000000000001"
const hex = (u: string) => u.replace(/-/g, "")
const call = (over: Partial<TypeCountsCall> = {}): TypeCountsCall => ({
	topics: [TOPIC],
	types: [CLAIM_TYPE, NEWS],
	spaces: null,
	matchAll: true,
	debateTaggedClaims: false,
	...over,
})

describe("planTypeCounts", () => {
	it("serves one topic with no Space filter, either match mode", () => {
		for (const matchAll of [true, false]) {
			const plan = planTypeCounts(call({matchAll}), 0)
			expect(plan?.group).toBe(hex(TOPIC))
			expect(plan?.walked).toEqual([hex(CLAIM_TYPE), hex(NEWS)])
		}
	})

	it("takes a repeated topic as one", () => {
		expect(planTypeCounts(call({topics: [TOPIC, TOPIC.toUpperCase()]}), 0)?.group).toBe(hex(TOPIC))
	})

	it("does not serve several topics, a Space filter, no types, too many types or too long a blocklist", () => {
		expect(planTypeCounts(call({topics: [TOPIC, OTHER]}), 0)).toBeNull()
		expect(planTypeCounts(call({spaces: [OTHER]}), 0)).toBeNull()
		expect(planTypeCounts(call({types: []}), 0)).toBeNull()
		const many = Array.from({length: 65}, (_, i) => `00000000-0000-4000-8000-${i.toString(16).padStart(12, "0")}`)
		expect(planTypeCounts(call({types: many}), 0)).toBeNull()
		expect(planTypeCounts(call({types: many.slice(0, 64)}), 256)).not.toBeNull()
		expect(planTypeCounts(call(), 257)).toBeNull()
	})

	it("makes types distinct and swaps Claim for Debate, counting both requested from one walk", () => {
		const plan = planTypeCounts(
			call({types: [CLAIM_TYPE, NEWS, CLAIM_TYPE, DEBATE_WALK_TYPE], debateTaggedClaims: true}),
			0,
		)
		expect(plan?.walked).toEqual([hex(DEBATE_WALK_TYPE), hex(NEWS)])
		expect(plan?.requested).toEqual([
			{type: CLAIM_TYPE, walked: 0},
			{type: NEWS, walked: 1},
			{type: DEBATE_WALK_TYPE, walked: 0},
		])
		const counts = typeCountsFrom(plan as NonNullable<typeof plan>, [4, 0])
		expect(counts).toEqual(
			new Map([
				[CLAIM_TYPE, 4],
				[DEBATE_WALK_TYPE, 4],
			]),
		)
	})

	it("leaves Claim alone without the swap", () => {
		expect(planTypeCounts(call({types: [CLAIM_TYPE]}), 0)?.walked).toEqual([hex(CLAIM_TYPE)])
	})
})

describe("readTypeCountsShadowConfig", () => {
	it("is off by default and bounds its numbers", () => {
		expect(readTypeCountsShadowConfig({}).enabled).toBe(false)
		const c = readTypeCountsShadowConfig({GEODB_TYPE_COUNTS_SHADOW: "1", GEODB_TYPE_COUNTS_SHADOW_TOPICS: "9999"})
		expect(c).toMatchObject({enabled: true, topics: 50, intervalMs: 300_000})
	})
})

describe("createTypeCountsShadow", () => {
	const log = {warn: vi.fn()}
	const count = (result: string) =>
		Number(new RegExp(`result="${result}"\\} (\\d+)`).exec(renderTypeCountsShadowMetrics())?.[1])
	beforeEach(() => {
		__resetTypeCountsShadowMetricsForTests()
		log.warn.mockClear()
	})

	/** Postgres says Claim 4 (Debate 4 under the swap) and News 2, for any topic. */
	const source = (over: Partial<TypeCountsSource> = {}): TypeCountsSource => ({
		sampleTopics: async () => [TOPIC],
		typesOf: async () => [NEWS],
		blocklist: async () => [OTHER],
		pgCounts: async () =>
			new Map([
				[NEWS, 2],
				[CLAIM_TYPE, 4],
			]),
		...over,
	})
	const reader = (typeCounts: DerivedCountsReader["typeCounts"]) => ({typeCounts}) as DerivedCountsReader

	it("exports nothing until mounted", () => {
		expect(renderTypeCountsShadowMetrics()).toBe("")
	})

	it("counts a match, sends the blocklist as hex and the swapped type once", async () => {
		const seen: {walked: string[]; blocked: string[]}[] = []
		const shadow = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 1},
			source: source(),
			reader: reader(async (_c, group, walked, blocked) => {
				expect(group).toBe(hex(TOPIC))
				seen.push({walked, blocked})
				return {
					walked: 6,
					counts: walked.map((w) =>
						w === hex(NEWS) ? 2 : w === hex(CLAIM_TYPE) || w === hex(DEBATE_WALK_TYPE) ? 4 : 0,
					),
				}
			}),
			log,
		})
		await shadow.runOnce()
		expect(count("match")).toBe(2)
		expect(count("mismatch")).toBe(0)
		expect(seen[0]?.blocked).toEqual([hex(OTHER)])
		// types: held (News), Claim, an absent one; the second call walks Debate where Claim was asked.
		expect(seen[1]?.walked).toContain(hex(DEBATE_WALK_TYPE))
		expect(seen[1]?.walked).not.toContain(hex(CLAIM_TYPE))
	})

	it("counts a mismatch and logs no ids", async () => {
		const shadow = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 1},
			source: source(),
			reader: reader(async (_c, _g, walked) => ({walked: 1, counts: walked.map(() => 1)})),
			log,
		})
		await shadow.runOnce()
		expect(count("mismatch")).toBe(2)
		expect(JSON.stringify(log.warn.mock.calls)).not.toContain(TOPIC)
	})

	it("counts a refusal as failed and a long blocklist as not served", async () => {
		const refused = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 1},
			source: source(),
			reader: reader(async () => {
				throw new GeoDbError("client_failed", "refused: InvalidWirePayload")
			}),
			log,
		})
		await refused.runOnce()
		expect(count("failed")).toBe(2)
		const long = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 1},
			source: source({blocklist: async () => Array.from({length: 257}, () => OTHER)}),
			reader: reader(async () => {
				throw new Error("not asked")
			}),
			log,
		})
		await long.runOnce()
		expect(count("not_served")).toBe(2)
	})

	it("counts a failed Postgres read", async () => {
		const shadow = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 1},
			source: source({
				pgCounts: async () => {
					throw new Error("db down")
				},
			}),
			reader: reader(async () => ({walked: 0, counts: []})),
			log,
		})
		await shadow.runOnce()
		expect(count("failed")).toBe(2)
	})

	it("retries a timeout once, and counts a second answer as a match", async () => {
		let calls = 0
		const shadow = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 1},
			source: source(),
			reader: reader(async (_c, _g, walked) => {
				calls++
				if (calls === 1) throw new GeoDbError("timeout", "derived read took longer than 500 ms")
				return {
					walked: 6,
					counts: walked.map((w) =>
						w === hex(NEWS) ? 2 : w === hex(CLAIM_TYPE) || w === hex(DEBATE_WALK_TYPE) ? 4 : 0,
					),
				}
			}),
			log,
		})
		await shadow.runOnce()
		expect(count("match")).toBe(2)
		expect(count("failed")).toBe(0)
		expect(calls).toBe(3)
		expect(renderTypeCountsShadowMetrics()).toContain('retries_total{result="answered"} 1')
	})

	it("does not retry a refusal or a bad request, and splits failures by reason", async () => {
		let calls = 0
		const shadow = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 1},
			source: source(),
			reader: reader(async () => {
				calls++
				throw new GeoDbError("bad_request", "derived counts wants a group")
			}),
			log,
		})
		await shadow.runOnce()
		expect(calls).toBe(2)
		expect(count("failed")).toBe(2)
		expect(renderTypeCountsShadowMetrics()).toContain('failed_total{reason="bad_request"} 2')
	})

	it("counts a timeout that repeats as failed, retried once each, and logs its reason", async () => {
		let calls = 0
		const shadow = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 1},
			source: source(),
			reader: reader(async () => {
				calls++
				throw new GeoDbError("timeout", "derived read took longer than 500 ms")
			}),
			log,
		})
		await shadow.runOnce()
		expect(calls).toBe(4)
		expect(renderTypeCountsShadowMetrics()).toContain('failed_total{reason="timeout"} 2')
		expect(renderTypeCountsShadowMetrics()).toContain('retries_total{result="failed"} 2')
		expect(log.warn).toHaveBeenCalledWith(
			"type counts shadow failed",
			expect.objectContaining({reason: "timeout", position: 2}),
		)
	})

	it("rate-limits the failure log and reports what it held back", async () => {
		let t = 0
		const shadow = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 8},
			source: source({sampleTopics: async () => Array.from({length: 8}, () => TOPIC)}),
			reader: reader(async () => {
				throw new GeoDbError("client_failed", "refused")
			}),
			log,
			now: () => t,
		})
		await shadow.runOnce()
		expect(count("failed")).toBe(16)
		expect(log.warn).toHaveBeenCalledTimes(5)
		t = 700_000
		await shadow.runOnce()
		expect(log.warn).toHaveBeenCalledTimes(10)
		expect(log.warn.mock.calls[5]?.[1]).toMatchObject({suppressed: 11})
	})

	it("logs per-type counts by fixed label, never an id", async () => {
		const shadow = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 1},
			source: source(),
			// geo-db sees News 3 (Postgres 2), Claim 4 (equal), Debate walk 5 (Postgres 4 under the swap).
			reader: reader(async (_c, _g, walked) => ({
				walked: 9,
				counts: walked.map((w) =>
					w === hex(NEWS) ? 3 : w === hex(DEBATE_WALK_TYPE) ? 5 : w === hex(CLAIM_TYPE) ? 4 : 0,
				),
			})),
			log,
		})
		await shadow.runOnce()
		const calls = log.warn.mock.calls.filter((c) => c[0] === "type counts shadow disagreement")
		expect(calls).toHaveLength(2)
		expect(calls[0]?.[1]).toMatchObject({
			matchAll: true,
			debate: false,
			diffs: [{type: "other", postgres: 2, geodb: 3}],
		})
		expect(calls[1]?.[1]).toMatchObject({
			matchAll: false,
			debate: true,
			diffs: [
				{type: "other", postgres: 2, geodb: 3},
				{type: "claim", postgres: 4, geodb: 5},
			],
		})
		const text = JSON.stringify(log.warn.mock.calls)
		for (const id of [TOPIC, NEWS, CLAIM_TYPE, DEBATE_WALK_TYPE, ABSENT]) expect(text).not.toContain(id)
	})
})
