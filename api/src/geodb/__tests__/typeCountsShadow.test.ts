import {beforeEach, describe, expect, it, vi} from "vitest"
import type {Database} from "../../forYou/queries"
import {GeoDbError} from "../client"
import type {DerivedCountsReader} from "../derived"
import {countsLine, countsRefusal, FormGatedError, refusalNeedsBackoff} from "../derived"
import {CLAIM_TYPE, DEBATE_TAG, DEBATE_WALK_TYPE} from "../rankedReads/feeds"
import {
	isFallback,
	NAME_PROPERTY,
	planTypeCounts,
	type TypeCountsCall,
	type TypeCountsPlan,
	typeCountsFrom,
} from "../rankedReads/typeCounts"
import {
	__resetTypeCountsShadowMetricsForTests,
	createPostgresTypeCountsSource,
	createTypeCountsShadow,
	FORM_BACKOFF_MS,
	formOf,
	nextUuid,
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
			expect((plan as TypeCountsPlan).group).toBe(hex(TOPIC))
			expect((plan as TypeCountsPlan).walked).toEqual([hex(CLAIM_TYPE), hex(NEWS)])
		}
	})

	it("takes a repeated topic as one", () => {
		expect((planTypeCounts(call({topics: [TOPIC, TOPIC.toUpperCase()]}), 0) as TypeCountsPlan).group).toBe(
			hex(TOPIC),
		)
	})

	it("does not serve no types, too many types, too many topics or too long a blocklist", () => {
		expect(planTypeCounts(call({types: []}), 0)).toBeNull()
		const many = Array.from({length: 65}, (_, i) => `00000000-0000-4000-8000-${i.toString(16).padStart(12, "0")}`)
		expect(planTypeCounts(call({types: many}), 0)).toBeNull()
		expect(planTypeCounts(call({types: many.slice(0, 64)}), 256)).not.toBeNull()
		expect(planTypeCounts(call(), 257)).toBeNull()
		expect(planTypeCounts(call({topics: many.slice(0, 17)}), 0)).toBeNull()
		expect(planTypeCounts(call({topics: many.slice(0, 16)}), 0)).not.toBeNull()
	})

	it("sends one topic as revision 13's form, several as revision 14's with match_all only for several", () => {
		const one = planTypeCounts(call({matchAll: true}), 0) as TypeCountsPlan
		expect(one).toMatchObject({group: hex(TOPIC), others: [], matchAll: false, graph: null, form: 13})
		const two = planTypeCounts(call({topics: [TOPIC, OTHER, TOPIC]}), 0) as TypeCountsPlan
		expect(two).toMatchObject({group: hex(TOPIC), others: [hex(OTHER)], matchAll: true, graph: null, form: 14})
		const pooled = planTypeCounts(call({topics: [TOPIC, OTHER], matchAll: false}), 0) as TypeCountsPlan
		expect(pooled).toMatchObject({matchAll: false, form: 14})
	})

	it("carries a Space filter as a graph block: the geo-db Spaces, the Name property, and the tag rule when Debate is walked", () => {
		const s1 = "11111111-1111-4111-8111-111111111111"
		const g1 = "22222222222222222222222222222222"
		const bare = planTypeCounts(call({spaces: [s1]}), 0, [g1]) as TypeCountsPlan
		expect(bare.form).toBe(15)
		expect(bare.graph).toEqual({spaces: [g1], nameProperty: hex(NAME_PROPERTY), tagged: null})
		// Claim swapped to Debate, and Debate asked outright, both walk Debate: both need the tag test (migration 0098).
		const swapped = planTypeCounts(call({spaces: [s1], debateTaggedClaims: true}), 0, [g1]) as TypeCountsPlan
		expect(swapped.graph?.tagged).toEqual({kind: hex(DEBATE_WALK_TYPE), relation: hex(DEBATE_TAG)})
		const direct = planTypeCounts(call({spaces: [s1], types: [DEBATE_WALK_TYPE]}), 0, [g1]) as TypeCountsPlan
		expect(direct.graph?.tagged).toEqual({kind: hex(DEBATE_WALK_TYPE), relation: hex(DEBATE_TAG)})
		// Distinct geo-db Spaces, however many gaia ones map to them.
		const merged = planTypeCounts(call({spaces: [s1, OTHER]}), 0, [g1, g1]) as TypeCountsPlan
		expect(merged.graph?.spaces).toEqual([g1])
	})

	it("does not serve a Space filter without its geo-db Spaces, or an empty one", () => {
		expect(planTypeCounts(call({spaces: [OTHER]}), 0)).toBeNull()
		expect(planTypeCounts(call({spaces: []}), 0, [])).toBeNull()
	})

	it("falls back on more than 64 Spaces, and takes exactly 64", () => {
		const spaces = (n: number) => Array.from({length: n}, (_, i) => i.toString(16).padStart(32, "0"))
		const sixtyFive = planTypeCounts(call({spaces: spaces(65)}), 0, spaces(65))
		expect(isFallback(sixtyFive)).toBe(true)
		expect(sixtyFive).toEqual({fallback: "too_many_spaces"})
		const sixtyFour = planTypeCounts(call({spaces: spaces(64)}), 0, spaces(64))
		expect(isFallback(sixtyFour)).toBe(false)
		expect((sixtyFour as TypeCountsPlan).graph?.spaces).toHaveLength(64)
	})

	it("makes types distinct and swaps Claim for Debate, counting both requested from one walk", () => {
		const plan = planTypeCounts(
			call({types: [CLAIM_TYPE, NEWS, CLAIM_TYPE, DEBATE_WALK_TYPE], debateTaggedClaims: true}),
			0,
		) as TypeCountsPlan
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
		expect((planTypeCounts(call({types: [CLAIM_TYPE]}), 0) as TypeCountsPlan).walked).toEqual([hex(CLAIM_TYPE)])
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
		sampleSpaces: async () => [],
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
				throw new GeoDbError("client_failed", "derived read refused: boom")
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
		// The retry answered, so failed_total is silent; the first-try counter still names the reason.
		expect(renderTypeCountsShadowMetrics()).toContain('first_try_failed_total{reason="timeout"} 1')
		expect(renderTypeCountsShadowMetrics()).toContain('first_try_failed_total{reason="client_failed"} 0')
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
		expect(count("failed")).toBe(18)
		expect(log.warn).toHaveBeenCalledTimes(5)
		t = 700_000
		await shadow.runOnce()
		expect(log.warn).toHaveBeenCalledTimes(10)
		expect(log.warn.mock.calls[5]?.[1]).toMatchObject({suppressed: 13})
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
	describe("Space filter, several topics and fallbacks", () => {
		const SPACE = "11111111-1111-4111-8111-111111111111"
		const GEO_SPACE = "99999999999999999999999999999999"
		const geoSpaceOf = async (id: string) => (id === hex(SPACE) ? GEO_SPACE : null)
		const answer = (walked: string[]) => ({
			walked: 6,
			counts: walked.map((w) =>
				w === hex(NEWS) ? 2 : w === hex(CLAIM_TYPE) || w === hex(DEBATE_WALK_TYPE) ? 4 : 0,
			),
		})
		const base = {config: {enabled: true, intervalMs: 1, topics: 1}, log}
		const withSpace = (over: Partial<TypeCountsSource> = {}) => source({sampleSpaces: async () => [SPACE], ...over})

		it("asks the Space filter with the geo-db Space, the Name property and the tag rule, and matches", async () => {
			const forms: unknown[] = []
			const shadow = createTypeCountsShadow({
				...base,
				source: withSpace(),
				geoSpaceOf,
				reader: reader(async (_c, _g, walked, _b, form) => {
					forms.push(form)
					return answer(walked)
				}),
			})
			await shadow.runOnce()
			expect(count("match")).toBe(4)
			const graphs = forms.filter((f) => (f as {graph: unknown} | undefined)?.graph)
			expect(graphs).toHaveLength(2)
			expect((graphs[0] as {graph: unknown}).graph).toEqual({
				spaces: [GEO_SPACE],
				nameProperty: hex(NAME_PROPERTY),
				tagged: null,
			})
			expect((graphs[1] as {graph: {tagged: unknown}}).graph.tagged).toEqual({
				kind: hex(DEBATE_WALK_TYPE),
				relation: hex(DEBATE_TAG),
			})
		})

		it("sends gaia's Space ids to Postgres, not geo-db's", async () => {
			const asked: (string[] | null)[] = []
			const shadow = createTypeCountsShadow({
				...base,
				source: withSpace({
					pgCounts: async (c) => {
						asked.push(c.spaces)
						return new Map([
							[NEWS, 2],
							[CLAIM_TYPE, 4],
						])
					},
				}),
				geoSpaceOf,
				reader: reader(async (_c, _g, walked) => answer(walked)),
			})
			await shadow.runOnce()
			expect(asked.filter((s) => s !== null)).toEqual([[SPACE], [SPACE]])
		})

		it("asks no Space filter without a Space map, and fails a Space with no geo-db twin as unmapped", async () => {
			let graphAsked = 0
			const noMap = createTypeCountsShadow({
				...base,
				source: withSpace(),
				reader: reader(async (_c, _g, walked, _b, form) => {
					if (form?.graph) graphAsked++
					return answer(walked)
				}),
			})
			await noMap.runOnce()
			expect(graphAsked).toBe(0)
			expect(count("match")).toBe(2)
			__resetTypeCountsShadowMetricsForTests()
			const unmapped = createTypeCountsShadow({
				...base,
				source: withSpace(),
				geoSpaceOf: async () => null,
				reader: reader(async (_c, _g, walked) => answer(walked)),
			})
			await unmapped.runOnce()
			expect(count("failed")).toBe(2)
			expect(renderTypeCountsShadowMetrics()).toContain('failed_total{reason="unmapped_space"} 2')
		})

		it("asks two topics in revision 14's form, pooled and match_all, and holds them", async () => {
			const forms: {others?: string[]; matchAll?: boolean}[] = []
			const shadow = createTypeCountsShadow({
				config: {enabled: true, intervalMs: 1, topics: 2},
				log,
				source: source({sampleTopics: async () => [TOPIC, OTHER]}),
				reader: reader(async (_c, _g, walked, _b, form) => {
					if (form) forms.push(form)
					return answer(walked)
				}),
			})
			await shadow.runOnce()
			expect(forms).toEqual([
				{others: [hex(OTHER)], matchAll: true, graph: null},
				{others: [hex(OTHER)], matchAll: false, graph: null},
			])
			expect(count("match")).toBe(6)
		})

		it("falls back, never mismatches, past 64 Spaces", async () => {
			const spaces = Array.from(
				{length: 65},
				(_, i) => `00000000-0000-4000-8000-${i.toString(16).padStart(12, "0")}`,
			)
			const shadow = createTypeCountsShadow({
				...base,
				source: withSpace({sampleSpaces: async () => spaces}),
				geoSpaceOf: async (id) => id,
				reader: reader(async (_c, _g, walked) => answer(walked)),
			})
			await shadow.runOnce()
			expect(count("fallback")).toBe(2)
			expect(count("mismatch")).toBe(0)
			expect(count("failed")).toBe(0)
			expect(renderTypeCountsShadowMetrics()).toContain('fallback_total{reason="too_many_spaces"} 2')
		})

		it("falls back on ResourceExhausted, without a retry or a mismatch", async () => {
			let graphCalls = 0
			const shadow = createTypeCountsShadow({
				...base,
				source: withSpace(),
				geoSpaceOf,
				reader: reader(async (_c, _g, walked, _b, form) => {
					if (!form?.graph) return answer(walked)
					graphCalls++
					throw new GeoDbError("client_failed", "derived read refused: ResourceExhausted")
				}),
			})
			await shadow.runOnce()
			expect(graphCalls).toBe(2)
			expect(count("fallback")).toBe(2)
			expect(count("match")).toBe(2)
			expect(count("mismatch")).toBe(0)
			expect(count("failed")).toBe(0)
			expect(renderTypeCountsShadowMetrics()).toContain('fallback_total{reason="resource_exhausted"} 2')
		})

		it("treats a serve below revision 15 as a fallback, probes once, then backs off and probes again later", async () => {
			let t = 0
			let graphCalls = 0
			const shadow = createTypeCountsShadow({
				...base,
				now: () => t,
				source: withSpace(),
				geoSpaceOf,
				reader: reader(async (_c, _g, walked, _b, form) => {
					if (!form?.graph) return answer(walked)
					graphCalls++
					// A revision 13 or 14 serve answers the graph block as the malformed request it is.
					throw new GeoDbError("client_failed", "derived read refused: InvalidWirePayload")
				}),
			})
			await shadow.runOnce()
			// The first graph call is refused (it kills the held worker); the second is not sent at all.
			expect(graphCalls).toBe(1)
			expect(count("fallback")).toBe(2)
			expect(count("failed")).toBe(0)
			expect(count("mismatch")).toBe(0)
			expect(renderTypeCountsShadowMetrics()).toContain('fallback_total{reason="serve_revision"} 1')
			expect(renderTypeCountsShadowMetrics()).toContain('fallback_total{reason="backoff"} 1')
			t = FORM_BACKOFF_MS - 1
			await shadow.runOnce()
			expect(graphCalls).toBe(1)
			t = FORM_BACKOFF_MS + 1
			await shadow.runOnce()
			expect(graphCalls).toBe(2)
		})

		it("does not back off a coded revision refusal (the pipe survives it) and asks again next pass", async () => {
			let graphCalls = 0
			const shadow = createTypeCountsShadow({
				...base,
				now: () => 0,
				source: withSpace(),
				geoSpaceOf,
				reader: reader(async (_c, _g, walked, _b, form) => {
					if (!form?.graph) return answer(walked)
					graphCalls++
					throw new GeoDbError(
						"client_failed",
						"derived read refused: code=InvalidWirePayload refused: DefiniteRefusal X",
					)
				}),
			})
			await shadow.runOnce()
			expect(graphCalls).toBe(2)
			expect(count("fallback")).toBe(2)
			expect(count("failed")).toBe(0)
			expect(renderTypeCountsShadowMetrics()).toContain('fallback_total{reason="serve_revision"} 2')
			expect(renderTypeCountsShadowMetrics()).toContain('fallback_total{reason="backoff"} 0')
		})

		it("falls back, without a back-off, on a form the reader gated before sending", async () => {
			let graphCalls = 0
			const shadow = createTypeCountsShadow({
				...base,
				now: () => 0,
				source: withSpace(),
				geoSpaceOf,
				reader: reader(async (_c, _g, walked, _b, form) => {
					if (!form?.graph) return answer(walked)
					graphCalls++
					throw new FormGatedError(14, 15)
				}),
			})
			await shadow.runOnce()
			expect(graphCalls).toBe(2)
			expect(count("failed")).toBe(0)
			expect(renderTypeCountsShadowMetrics()).toContain('fallback_total{reason="serve_revision"} 2')
		})

		it("treats a revision 13 serve's refusal of several topics as a fallback that also holds the graph form back", async () => {
			let wide = 0
			const shadow = createTypeCountsShadow({
				config: {enabled: true, intervalMs: 1, topics: 2},
				log,
				source: withSpace({sampleTopics: async () => [TOPIC, OTHER]}),
				geoSpaceOf,
				reader: reader(async (_c, _g, walked, _b, form) => {
					if (form?.graph || form?.others?.length) {
						wide++
						throw new GeoDbError("client_failed", "derived read refused: InvalidWirePayload")
					}
					return answer(walked)
				}),
			})
			await shadow.runOnce()
			// Topic 1's graph call probes and is refused (holding the graph form back); the several-topic call probes its own form and
			// is refused too; the plain calls still match.
			expect(wide).toBe(2)
			expect(count("failed")).toBe(0)
			expect(count("mismatch")).toBe(0)
			expect(count("match")).toBe(4)
		})
	})
})

describe("countsLine", () => {
	const hexOf = (c: string) => c.repeat(32)
	const TYPES = [hexOf("a"), hexOf("b")]

	it("keeps revision 13's line byte for byte", () => {
		expect(countsLine(1, hexOf("1"), TYPES, [])).toBe(`counts\t1\t0x${hexOf("1")}\t${TYPES.join(",")}\t-`)
		expect(countsLine(1, hexOf("1"), TYPES, [hexOf("c")])).toBe(
			`counts\t1\t0x${hexOf("1")}\t${TYPES.join(",")}\t${hexOf("c")}`,
		)
	})

	it("writes several topics with all: for match_all, and without it for the pooled union", () => {
		const t = [hexOf("1"), hexOf("2")]
		expect(countsLine(1, t[0] as string, TYPES, [], {others: [t[1] as string], matchAll: true})).toBe(
			`counts\t1\tall:0x${t[0]},0x${t[1]}\t${TYPES.join(",")}\t-`,
		)
		expect(countsLine(1, t[0] as string, TYPES, [], {others: [t[1] as string]})).toBe(
			`counts\t1\t0x${t[0]},0x${t[1]}\t${TYPES.join(",")}\t-`,
		)
	})

	it("writes the graph block as the client's fourth list: name/spaces[/kind/relation]", () => {
		const spaces = [hexOf("5"), hexOf("6")]
		const line = countsLine(1, hexOf("1"), TYPES, [], {
			graph: {spaces, nameProperty: hexOf("7"), tagged: {kind: hexOf("8"), relation: hexOf("9")}},
		})
		expect(line).toBe(
			`counts\t1\t0x${hexOf("1")}\t${TYPES.join(",")}\t-\t${hexOf("7")}/${spaces.join(",")}/${hexOf("8")}/${hexOf("9")}`,
		)
		const untagged = countsLine(1, hexOf("1"), TYPES, [hexOf("c")], {
			graph: {spaces: [hexOf("5")], nameProperty: hexOf("7"), tagged: null},
		})
		expect(untagged.split("\t").slice(4)).toEqual([hexOf("c"), `${hexOf("7")}/${hexOf("5")}`])
	})

	it("refuses a graph block outside geo-db's grammar before sending it", () => {
		const sp = (n: number) => Array.from({length: n}, (_, i) => i.toString(16).padStart(32, "0"))
		const graph = (spaces: string[]) => ({graph: {spaces, nameProperty: hexOf("7"), tagged: null}})
		expect(() => countsLine(1, hexOf("1"), TYPES, [], graph([]))).toThrow(GeoDbError)
		expect(() => countsLine(1, hexOf("1"), TYPES, [], graph(sp(65)))).toThrow(GeoDbError)
		expect(() => countsLine(1, hexOf("1"), TYPES, [], graph(sp(64)))).not.toThrow()
		expect(() => countsLine(1, hexOf("1"), TYPES, [], graph([hexOf("5"), hexOf("5")]))).toThrow(GeoDbError)
		expect(() => countsLine(1, hexOf("1"), TYPES, [], {matchAll: true})).toThrow(GeoDbError)
		expect(() => countsLine(1, hexOf("1"), TYPES, [], {others: [hexOf("1")]})).toThrow(GeoDbError)
	})
})

/**
 * The geodb-client turns the line above into geo-db's wire request; docs/api/10 (revisions 13 to 15) gives its layout. This
 * encodes a line by that layout and holds the bytes, so a change to either side's grammar shows here. Integers are little-endian;
 * a topic is a `u128` (the hex read as a number, written little-endian), a type, blocked entity, Space and property a plain 16 bytes.
 */
function wireOf(line: string): Buffer {
	const [verb, collection, group, types, blocked, graph] = line.split("\t") as [
		string,
		string,
		string,
		string,
		string,
		string?,
	]
	expect(verb).toBe("counts")
	const out: Buffer[] = []
	const u = (n: number, bytes: number) => {
		const b = Buffer.alloc(bytes)
		b.writeUIntLE(n, 0, bytes)
		out.push(b)
	}
	const id16 = (h: string) => Buffer.from(h, "hex")
	const u128 = (h: string) => Buffer.from(h, "hex").reverse()
	const matchAll = group.startsWith("all:")
	const topics = group
		.replace(/^all:/, "")
		.split(",")
		.map((t) => t.replace(/^0x/, ""))
	const typeIds = types.split(",")
	const blockedIds = blocked === "-" ? [] : blocked.split(",")
	u(Number(collection), 4)
	out.push(u128(topics[0] as string))
	if (topics.length > 1 || matchAll || graph !== undefined) {
		u(0xffff, 2)
		u((matchAll ? 1 : 0) | (graph !== undefined ? 2 : 0), 1)
		u(topics.length - 1, 1)
		for (const t of topics.slice(1)) out.push(u128(t))
	} else u(0, 2)
	u(typeIds.length, 2)
	for (const t of typeIds) out.push(id16(t))
	u(blockedIds.length, 2)
	for (const b of blockedIds) out.push(id16(b))
	if (graph !== undefined) {
		const [name, spaces, kind, relation] = graph.split("/") as [string, string, string?, string?]
		const spaceIds = spaces.split(",")
		u(spaceIds.length, 2)
		for (const sp of spaceIds) out.push(id16(sp))
		out.push(id16(name))
		if (kind !== undefined && relation !== undefined) {
			u(1, 1)
			out.push(id16(kind), id16(relation))
		} else u(0, 1)
	}
	return Buffer.concat(out)
}

describe("the graph block against geo-db's documented layout (revision 15)", () => {
	const S1 = "0123456789abcdef0123456789abcdef"
	const S2 = "fedcba9876543210fedcba9876543210"
	const NAME = hex(NAME_PROPERTY)

	it("lays a tagged block out as flags, Space count, Spaces, name property, tagged flag, kind, relation", () => {
		const plan = planTypeCounts(
			call({
				topics: [TOPIC, OTHER],
				types: [CLAIM_TYPE],
				spaces: [TOPIC, OTHER],
				matchAll: true,
				debateTaggedClaims: true,
			}),
			1,
			[S1, S2],
		) as TypeCountsPlan
		const line = countsLine(1, plan.group, plan.walked, [hex(NEWS)], {
			others: plan.others,
			matchAll: plan.matchAll,
			graph: plan.graph,
		})
		const bytes = wireOf(line)
		const expected = Buffer.concat([
			Buffer.from("01000000", "hex"), // collection 1
			Buffer.from(hex(TOPIC), "hex").reverse(), // group u128 LE
			Buffer.from("ffff", "hex"), // multi-topic marker
			Buffer.from("03", "hex"), // flags: match_all | graph
			Buffer.from("01", "hex"), // one other topic
			Buffer.from(hex(OTHER), "hex").reverse(),
			Buffer.from("0100", "hex"), // one type: Debate (Claim swapped)
			Buffer.from(hex(DEBATE_WALK_TYPE), "hex"),
			Buffer.from("0100", "hex"), // one blocked
			Buffer.from(hex(NEWS), "hex"),
			Buffer.from("0200", "hex"), // two Spaces
			Buffer.from(S1, "hex"),
			Buffer.from(S2, "hex"),
			Buffer.from(NAME, "hex"), // name property
			Buffer.from("01", "hex"), // tagged
			Buffer.from(hex(DEBATE_WALK_TYPE), "hex"), // kind
			Buffer.from(hex(DEBATE_TAG), "hex"), // relation
		])
		expect(bytes.toString("hex")).toBe(expected.toString("hex"))
		// geo-db's own bound, `type_count_bytes_max`'s derivation: head 4+16+2, flags 1, topics 1, types 2+16n, blocked 2+16m, graph.
		expect(bytes.length).toBe(4 + 16 + 2 + 1 + 1 + 16 + 2 + 16 + 2 + 16 + (2 + 2 * 16 + 16 + 1 + 32))
	})

	it("lays an untagged single-topic block out with flags 0x02 and no tagged ids", () => {
		const plan = planTypeCounts(call({types: [NEWS], spaces: [OTHER]}), 0, [S1]) as TypeCountsPlan
		const bytes = wireOf(
			countsLine(1, plan.group, plan.walked, [], {
				others: plan.others,
				matchAll: plan.matchAll,
				graph: plan.graph,
			}),
		)
		expect(bytes.subarray(20, 24).toString("hex")).toBe("ffff0200") // marker, flags 0x02, no other topics
		const graphAt = 4 + 16 + 2 + 1 + 1 + 2 + 16 + 2
		expect(bytes.subarray(graphAt).toString("hex")).toBe(`0100${S1}${NAME}00`)
	})

	it("a 64-Space block is the size geo-db's derivation allows", () => {
		const spaces = Array.from({length: 64}, (_, i) => i.toString(16).padStart(32, "0"))
		const plan = planTypeCounts(call({types: [DEBATE_WALK_TYPE], spaces}), 0, spaces) as TypeCountsPlan
		const bytes = wireOf(countsLine(1, plan.group, plan.walked, [], {graph: plan.graph}))
		expect(bytes.length).toBe(4 + 16 + 2 + 1 + 1 + 2 + 16 + 2 + (2 + 64 * 16 + 16 + 1 + 32))
	})
})

describe("countsRefusal", () => {
	const refused = (m: string) => new GeoDbError("client_failed", `derived read refused: ${m}`)
	it("tells a too-big call from a serve below the form's revision, and leaves every other error alone", () => {
		expect(countsRefusal(refused("ResourceExhausted"))).toBe("exhausted")
		expect(countsRefusal(refused("InvalidWirePayload"))).toBe("revision")
		expect(countsRefusal(refused("want put|delete|clear|get|scan|counts<tab>..."))).toBe("revision")
		expect(countsRefusal(refused("NotFoundOrRestricted"))).toBeNull()
		expect(countsRefusal(new GeoDbError("timeout", "derived read took longer than 500 ms"))).toBeNull()
		expect(countsRefusal(new GeoDbError("client_failed", "geodb-client derived pipe exited (1)"))).toBeNull()
		expect(countsRefusal(new Error("InvalidWirePayload"))).toBeNull()
	})

	it("reads the code= token first and maps it", () => {
		expect(countsRefusal(refused("code=ResourceExhausted refused: DefiniteRefusal ResourceExhausted"))).toBe(
			"exhausted",
		)
		expect(countsRefusal(refused("code=InvalidWirePayload refused: DefiniteRefusal InvalidWirePayload"))).toBe(
			"revision",
		)
		expect(countsRefusal(refused("code=UnknownOpcode refused: DefiniteRefusal UnknownOpcode"))).toBe("revision")
		// The text after the token is never matched: a token that names neither is not a revision refusal, whatever it says.
		expect(countsRefusal(refused("code=MalformedLine want counts<tab> InvalidWirePayload"))).toBeNull()
		expect(countsRefusal(refused("code=NotFoundOrRestricted refused: ResourceExhausted"))).toBeNull()
		expect(countsRefusal(new FormGatedError(14, 15))).toBe("revision")
	})

	it("asks for a back-off only on a refusal with no token", () => {
		expect(refusalNeedsBackoff(refused("InvalidWirePayload"))).toBe(true)
		expect(refusalNeedsBackoff(refused("code=InvalidWirePayload refused: x"))).toBe(false)
		expect(refusalNeedsBackoff(new FormGatedError(14, 15))).toBe(false)
	})
})

describe("form labels and the topic sampler", () => {
	const log = {warn: vi.fn()}
	const form = (f: string, r: string) =>
		Number(
			new RegExp(`form_total\\{form="${f}",result="${r}"\\} (\\d+)`).exec(renderTypeCountsShadowMetrics())?.[1],
		)
	beforeEach(() => {
		__resetTypeCountsShadowMetricsForTests()
		log.warn.mockClear()
	})

	it("names each call shape", () => {
		expect(formOf(call())).toBe("plain")
		expect(formOf(call({debateTaggedClaims: true}))).toBe("debate")
		expect(formOf(call({spaces: [OTHER]}))).toBe("spaces")
		expect(formOf(call({spaces: [OTHER], debateTaggedClaims: true}))).toBe("spaces_debate")
		expect(formOf(call({topics: [TOPIC, OTHER]}))).toBe("topics")
	})

	it("counts each result under its form", async () => {
		const src: TypeCountsSource = {
			sampleTopics: async () => [TOPIC, OTHER],
			typesOf: async () => [NEWS],
			blocklist: async () => [],
			sampleSpaces: async () => [],
			pgCounts: async () => new Map([[NEWS, 2]]),
		}
		const shadow = createTypeCountsShadow({
			config: {enabled: true, intervalMs: 1, topics: 2},
			source: src,
			reader: {
				typeCounts: async (_c: unknown, _g: unknown, walked: string[]) => ({
					walked: 1,
					counts: walked.map((w) => (w === hex(NEWS) ? 2 : 0)),
				}),
			} as unknown as DerivedCountsReader,
			log,
		})
		await shadow.runOnce()
		expect(form("plain", "match") + form("plain", "mismatch")).toBe(2)
		expect(form("debate", "match") + form("debate", "mismatch")).toBe(2)
		expect(form("spaces", "match") + form("spaces", "mismatch")).toBe(0)
		expect(form("topics", "match") + form("topics", "mismatch")).toBeGreaterThan(0)
	})

	it("steps a uuid up by one, carrying and wrapping", () => {
		expect(nextUuid("00000000-0000-4000-8000-00000000000f")).toBe("00000000-0000-4000-8000-000000000010")
		expect(nextUuid("ffffffff-ffff-ffff-ffff-ffffffffffff")).toBe("00000000-0000-0000-0000-000000000000")
	})

	it("samples distinct topics by index step from a random start, wrapping once", async () => {
		const topics = [OTHER, TOPIC, NEWS].sort()
		const db = {
			execute: vi.fn(async (q: {queryChunks?: unknown[]}) => {
				const chunks = q.queryChunks ?? []
				const from = chunks.find((c) => typeof c === "string" && /^[0-9a-f]{8}-[0-9a-f-]{27}$/.test(c)) as
					| string
					| undefined
				const hit = from === undefined ? topics[0] : topics.find((t) => t >= from)
				return {rows: hit ? [{id: hit}] : []}
			}),
		} as unknown as Database
		const got = await createPostgresTypeCountsSource(db).sampleTopics(5)
		expect(new Set(got).size).toBe(got.length)
		expect([...got].sort()).toEqual(topics)
		expect((await createPostgresTypeCountsSource(db).sampleTopics(2)).length).toBe(2)
	})
})
