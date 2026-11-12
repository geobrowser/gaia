import {beforeEach, describe, expect, it, vi} from "vitest"
import {GeoDbError} from "../client"
import type {DerivedCountsReader, DerivedScoresReader, ScoreCard, ScoreConfig, ScoreRow} from "../derived"
import {FormGatedError} from "../derived"
import {CLAIM_TYPE} from "../rankedReads/feeds"
import type {TypeCountsCall} from "../rankedReads/typeCounts"
import {
	__resetRankedReadsServeMetricsForTests,
	APPLIED_BLOCK_KEY,
	appliedBlockOf,
	createAppliedBlockFreshness,
	createRankedReadsServe,
	parseServeReads,
	type RankedReadsServe,
	readRankedReadsServeConfig,
	renderRankedReadsServeMetrics,
	type ScoresPorts,
	SERVE_READS,
	ServeDecline,
	type ServeRead,
	serveBestOrder,
	serveEntityColumns,
	serveFeeds,
	serveTrending,
	serveTypeCounts,
} from "../rankedReadsServe"

const TOPIC = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
const NEWS = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
const SPACE = "dddddddd-dddd-4ddd-8ddd-dddddddddddd"
const hex = (u: string) => u.replace(/-/g, "")
const A = "aa".repeat(16)
const B = "bb".repeat(16)
const C = "cc".repeat(16)

const metric = (name: string, labels: string) =>
	Number(new RegExp(`${name}\\{${labels}\\} (\\d+)`).exec(renderRankedReadsServeMetrics())?.[1])
const servedBy = (read: string, source: string) =>
	metric("gaia_api_ranked_read_serve_total", `read="${read}",source="${source}"`)
const fellBack = (read: string, reason: string) =>
	metric("gaia_api_ranked_read_serve_fallback_total", `read="${read}",reason="${reason}"`)

const fresh = (lag: number | null) => ({lagBlocks: async () => lag})
function serveOf(
	reads: ServeRead[],
	over: {freshness?: {lagBlocks(): Promise<number | null>}; timeoutMs?: number; maxLagBlocks?: number} = {},
): RankedReadsServe {
	return createRankedReadsServe({
		config: {
			reads: new Set(reads),
			timeoutMs: over.timeoutMs ?? 50,
			maxLagBlocks: over.maxLagBlocks ?? 20,
			cacheMs: 0,
		},
		freshness: over.freshness ?? fresh(0),
		log: {warn: vi.fn()},
	})
}

beforeEach(() => __resetRankedReadsServeMetricsForTests())

describe("config", () => {
	it("lists nothing by default and exports nothing", () => {
		expect(readRankedReadsServeConfig({}).reads.size).toBe(0)
		expect(readRankedReadsServeConfig({GEODB_RANKED_READS_SERVE: ""}).reads.size).toBe(0)
		createRankedReadsServe({config: readRankedReadsServeConfig({})})
		expect(renderRankedReadsServeMetrics()).toBe("")
	})

	it("parses the list like GRAPHQL_GATEWAY_FIELDS: bare or =on lists, =off and unknown names do not", () => {
		expect([...parseServeReads("typeCounts, bestOrder=on,feeds=off,bogus,entityColumns=shadow")].sort()).toEqual([
			"bestOrder",
			"typeCounts",
		])
		expect(parseServeReads(SERVE_READS.join(",")).size).toBe(SERVE_READS.length)
	})

	it("reads the budget and lag bound, keeping the defaults for a bad value", () => {
		expect(readRankedReadsServeConfig({})).toMatchObject({timeoutMs: 250, maxLagBlocks: 20, cacheMs: 5000})
		expect(
			readRankedReadsServeConfig({
				GEODB_RANKED_READS_SERVE_TIMEOUT_MS: "100",
				GEODB_RANKED_READS_SERVE_MAX_LAG_BLOCKS: "5",
				GEODB_RANKED_READS_SERVE_CACHE_MS: "0",
			}),
		).toMatchObject({timeoutMs: 100, maxLagBlocks: 5, cacheMs: 0})
		expect(
			readRankedReadsServeConfig({
				GEODB_RANKED_READS_SERVE_TIMEOUT_MS: "0",
				GEODB_RANKED_READS_SERVE_MAX_LAG_BLOCKS: "x",
			}),
		).toMatchObject({timeoutMs: 250, maxLagBlocks: 20})
	})
})

describe("run", () => {
	it("never asks geo-db for a read that is not listed", async () => {
		const serve = serveOf(["feeds"])
		const geo = vi.fn(async () => "geo")
		expect(await serve.run("typeCounts", geo, async () => "pg")).toBe("pg")
		expect(geo).not.toHaveBeenCalled()
		expect(servedBy("typeCounts", "postgres")).toBe(0)
	})

	it.each(SERVE_READS)("%s: a listed read serves geo-db's answer", async (read) => {
		const serve = serveOf([read])
		const pg = vi.fn(async () => "pg")
		expect(await serve.run(read, async () => "geo", pg)).toBe("geo")
		expect(pg).not.toHaveBeenCalled()
		expect(servedBy(read, "geodb")).toBe(1)
	})

	it.each(SERVE_READS)("%s: falls back to Postgres on an error, counted by read and reason", async (read) => {
		const serve = serveOf([read])
		expect(
			await serve.run(
				read,
				async () => {
					throw new Error("boom")
				},
				async () => "pg",
			),
		).toBe("pg")
		expect(fellBack(read, "error")).toBe(1)
		expect(servedBy(read, "postgres")).toBe(1)
		expect(servedBy(read, "geodb")).toBe(0)
		// A GeoDbError counts under its own reason, a decline under its.
		await serve.run(
			read,
			async () => {
				throw new GeoDbError("overloaded", "busy")
			},
			async () => "pg",
		)
		await serve.run(
			read,
			async () => {
				throw new ServeDecline("not_served")
			},
			async () => "pg",
		)
		expect(fellBack(read, "overloaded")).toBe(1)
		expect(fellBack(read, "not_served")).toBe(1)
	})

	it.each(SERVE_READS)("%s: falls back to Postgres on a timeout and aborts the geo-db read", async (read) => {
		const serve = serveOf([read], {timeoutMs: 10})
		let aborted = false
		const answer = await serve.run(
			read,
			(signal) =>
				new Promise<string>(() => {
					signal.addEventListener("abort", () => {
						aborted = true
					})
				}),
			async () => "pg",
		)
		expect(answer).toBe("pg")
		expect(aborted).toBe(true)
		expect(fellBack(read, "timeout")).toBe(1)
	})

	it("a Postgres failure on the fallback is the caller's own", async () => {
		const serve = serveOf(["typeCounts"])
		await expect(
			serve.run(
				"typeCounts",
				async () => {
					throw new Error("geo")
				},
				async () => {
					throw new Error("pg down")
				},
			),
		).rejects.toThrow("pg down")
	})
})

describe("the lag guard", () => {
	it.each(SERVE_READS)(
		"%s: serves at the bound and falls back to Postgres past it, without asking geo-db",
		async (read) => {
			const geo = vi.fn(async () => "geo")
			const atBound = serveOf([read], {freshness: fresh(20), maxLagBlocks: 20})
			expect(await atBound.run(read, geo, async () => "pg")).toBe("geo")
			const behind = serveOf([read], {freshness: fresh(21), maxLagBlocks: 20})
			geo.mockClear()
			expect(await behind.run(read, geo, async () => "pg")).toBe("pg")
			expect(geo).not.toHaveBeenCalled()
			expect(fellBack(read, "lag")).toBe(1)
		},
	)

	it("fails closed when the lag cannot be told: no freshness source, an unknown lag, or a throwing source", async () => {
		for (const freshness of [
			fresh(null),
			{
				lagBlocks: async () => {
					throw new Error("x")
				},
			},
		]) {
			const serve = serveOf(["bestOrder"], {freshness})
			expect(
				await serve.run(
					"bestOrder",
					async () => "geo",
					async () => "pg",
				),
			).toBe("pg")
		}
		const none = createRankedReadsServe({
			config: {reads: new Set<ServeRead>(["bestOrder"]), timeoutMs: 50, maxLagBlocks: 20, cacheMs: 0},
		})
		expect(
			await none.run(
				"bestOrder",
				async () => "geo",
				async () => "pg",
			),
		).toBe("pg")
		expect(fellBack("bestOrder", "lag_unknown")).toBe(3)
	})

	it("exports every series at zero once a read is listed", () => {
		serveOf(["typeCounts"])
		expect(servedBy("feeds", "geodb")).toBe(0)
		expect(fellBack("trending", "lag")).toBe(0)
	})
})

describe("applied-block freshness", () => {
	const row = (value: string) => ({value: Buffer.from(value), version: 1}) as never
	const key = Buffer.from(APPLIED_BLOCK_KEY).toString("hex")

	it("parses the row's block", () => {
		expect(appliedBlockOf('105:{"0":105,"1":110}')).toBe(105)
		expect(appliedBlockOf("x:{}")).toBeNull()
		expect(appliedBlockOf("")).toBeNull()
	})

	it("is the head minus the applied block, never negative", async () => {
		const get = vi.fn(async () => row('100:{"0":100}'))
		const f = createAppliedBlockFreshness({reader: {get} as never, pgHead: async () => 130, cacheMs: 0})
		expect(await f.lagBlocks()).toBe(30)
		expect(get).toHaveBeenCalledWith(10, key)
		const ahead = createAppliedBlockFreshness({reader: {get} as never, pgHead: async () => 90, cacheMs: 0})
		expect(await ahead.lagBlocks()).toBe(0)
	})

	it("is unknown when the row, the head or a read is missing", async () => {
		const missingRow = createAppliedBlockFreshness({
			reader: {get: async () => null} as never,
			pgHead: async () => 5,
			cacheMs: 0,
		})
		expect(await missingRow.lagBlocks()).toBeNull()
		const noHead = createAppliedBlockFreshness({
			reader: {get: async () => row("1:{}")} as never,
			pgHead: async () => null,
			cacheMs: 0,
		})
		expect(await noHead.lagBlocks()).toBeNull()
		const broken = createAppliedBlockFreshness({
			reader: {
				get: async () => {
					throw new Error("pipe")
				},
			} as never,
			pgHead: async () => 5,
			cacheMs: 0,
		})
		expect(await broken.lagBlocks()).toBeNull()
		const malformed = createAppliedBlockFreshness({
			reader: {get: async () => row("nope")} as never,
			pgHead: async () => 5,
			cacheMs: 0,
		})
		expect(await malformed.lagBlocks()).toBeNull()
	})

	it("caches for cacheMs, one read at a time", async () => {
		let t = 0
		const get = vi.fn(async () => row("100:{}"))
		const f = createAppliedBlockFreshness({
			reader: {get} as never,
			pgHead: async () => 110,
			cacheMs: 1000,
			now: () => t,
		})
		await Promise.all([f.lagBlocks(), f.lagBlocks()])
		expect(get).toHaveBeenCalledTimes(1)
		t = 999
		await f.lagBlocks()
		expect(get).toHaveBeenCalledTimes(1)
		t = 1000
		await f.lagBlocks()
		expect(get).toHaveBeenCalledTimes(2)
	})
})

describe("typeCounts", () => {
	const call = (over: Partial<TypeCountsCall> = {}): TypeCountsCall => ({
		topics: [TOPIC],
		types: [CLAIM_TYPE, NEWS],
		spaces: null,
		matchAll: true,
		debateTaggedClaims: false,
		...over,
	})
	const reader = (typeCounts: DerivedCountsReader["typeCounts"]) => ({typeCounts}) as DerivedCountsReader

	it("serves the counts of the types geo-db walked, dropping zeros, through run", async () => {
		const typeCounts = vi.fn(async (..._a: unknown[]) => ({walked: 4, counts: [4, 0]}))
		const ports = {reader: reader(typeCounts), blocklist: async () => [B]}
		const serve = serveOf(["typeCounts"])
		const out = await serve.run(
			"typeCounts",
			() => serveTypeCounts(ports, call()),
			async () => new Map<string, number>(),
		)
		expect([...out]).toEqual([[CLAIM_TYPE, 4]])
		expect(typeCounts).toHaveBeenCalledWith(1, hex(TOPIC), [hex(CLAIM_TYPE), hex(NEWS)], [B], undefined)
		expect(servedBy("typeCounts", "geodb")).toBe(1)
	})

	it("sends several topics and a Space filter as the wider forms", async () => {
		const typeCounts = vi.fn(async (..._a: unknown[]) => ({walked: 1, counts: [1, 2]}))
		const geoSpaceOf = vi.fn(async () => "ee".repeat(16))
		const ports = {reader: reader(typeCounts), geoSpaceOf, blocklist: async () => []}
		await serveTypeCounts(ports, call({spaces: [SPACE]}))
		expect(geoSpaceOf).toHaveBeenCalledWith(hex(SPACE))
		expect(typeCounts.mock.calls[0]?.[4]).toMatchObject({graph: {spaces: ["ee".repeat(16)]}})
	})

	it.each([
		["a Space filter with no space map", call({spaces: [SPACE]}), undefined, "not_served"],
		["no types", call({types: []}), undefined, "not_served"],
		["a Space with no geo-db twin", call({spaces: [SPACE]}), async () => null, "unmapped_space"],
	])("falls back to Postgres on %s", async (_name, c, geoSpaceOf, reason) => {
		const ports = {reader: reader(vi.fn()), geoSpaceOf, blocklist: async () => []}
		const serve = serveOf(["typeCounts"])
		const out = await serve.run(
			"typeCounts",
			() => serveTypeCounts(ports, c),
			async () => "pg" as never,
		)
		expect(out).toBe("pg")
		expect(fellBack("typeCounts", reason)).toBe(1)
	})

	it("falls back on a blocklist past 256, more than 64 Spaces, a serve below the form, a read budget refusal and a failed read", async () => {
		const serve = serveOf(["typeCounts"])
		const ask = (ports: Parameters<typeof serveTypeCounts>[0], c = call()) =>
			serve.run(
				"typeCounts",
				() => serveTypeCounts(ports, c),
				async () => "pg" as never,
			)
		const blockedMany = Array.from({length: 257}, (_, i) => i.toString(16).padStart(32, "0"))
		expect(await ask({reader: reader(vi.fn()), blocklist: async () => blockedMany})).toBe("pg")
		const spaces = Array.from({length: 65}, (_, i) => `00000000-0000-4000-8000-${i.toString(16).padStart(12, "0")}`)
		expect(
			await ask({reader: reader(vi.fn()), geoSpaceOf: async () => A, blocklist: async () => []}, call({spaces})),
		).toBe("pg")
		const gated = reader(async () => {
			throw new FormGatedError(13, 15)
		})
		expect(await ask({reader: gated, blocklist: async () => []})).toBe("pg")
		const exhausted = reader(async () => {
			throw new GeoDbError("client_failed", "derived read refused: code=ResourceExhausted budget")
		})
		expect(await ask({reader: exhausted, blocklist: async () => []})).toBe("pg")
		const broken = reader(async () => {
			throw new GeoDbError("timeout", "slow")
		})
		expect(await ask({reader: broken, blocklist: async () => []})).toBe("pg")
		expect(fellBack("typeCounts", "not_served")).toBe(1)
		expect(fellBack("typeCounts", "too_many_spaces")).toBe(1)
		expect(fellBack("typeCounts", "serve_revision")).toBe(1)
		expect(fellBack("typeCounts", "resource_exhausted")).toBe(1)
		expect(fellBack("typeCounts", "timeout")).toBe(1)
	})
})

const CONFIG: ScoreConfig = {
	tauSeconds: 1,
	wilsonZ: 1,
	priorPositive: 1,
	priorNegative: 1,
	qualityFloor: 0,
	intrinsicCap: 1,
	intrinsicPropertyTarget: 1,
	intrinsicRelationTarget: 1,
	participationWeight: 1,
	participationCap: 1,
	commentWeight: 1,
	commentCap: 1,
}
const card = (entityHex: string): ScoreCard => ({
	entityHex,
	propertyCount: 1,
	relationCount: 1,
	createdEpoch: 1,
	typeWeightMax: null,
})
const scoreRow = (ranking: number): ScoreRow => ({
	engaged: false,
	trendingSource: 0,
	counts: [3, 1, 4, 2, 5],
	quality: 0.5,
	intrinsic: 0.25,
	participation: 1,
	comment: 0.5,
	typeWeight: 1,
	watch: 0,
	ageCredit: 0.1,
	trending: 0.2,
	ranking,
	topicHex: null,
})
function scoresPorts(ranking: Record<string, number>, over: Partial<ScoresPorts> = {}): ScoresPorts {
	return {
		reader: {
			scores: async (_config: ScoreConfig, cards: ScoreCard[]) =>
				cards.map((c) => scoreRow(ranking[c.entityHex] ?? 0)),
		} as unknown as DerivedScoresReader,
		cards: async (ids) => new Map(ids.filter((id) => id in ranking).map((id) => [id, card(id)] as const)),
		config: async () => CONFIG,
		...over,
	}
}

describe("bestOrder", () => {
	it("orders by the ranking geo-db serves, ties by id descending, ids with no entity omitted", async () => {
		const ports = scoresPorts({[A]: 1, [B]: 5, [C]: 5})
		const serve = serveOf(["bestOrder"])
		const out = await serve.run(
			"bestOrder",
			() => serveBestOrder(ports, [A, B, "dd".repeat(16), C]),
			async () => ["pg"],
		)
		expect(out).toEqual([C, B, A])
		expect(servedBy("bestOrder", "geodb")).toBe(1)
	})

	it("falls back to Postgres on a failed scores read, a missing config and a short reply", async () => {
		const serve = serveOf(["bestOrder"])
		const ask = (ports: ScoresPorts) =>
			serve.run(
				"bestOrder",
				() => serveBestOrder(ports, [A]),
				async () => ["pg"],
			)
		expect(
			await ask(
				scoresPorts(
					{[A]: 1},
					{
						reader: {
							scores: async () => {
								throw new GeoDbError("client_failed", "x")
							},
						} as unknown as DerivedScoresReader,
					},
				),
			),
		).toEqual(["pg"])
		expect(await ask(scoresPorts({[A]: 1}, {config: async () => null}))).toEqual(["pg"])
		expect(
			await ask(scoresPorts({[A]: 1}, {reader: {scores: async () => []} as unknown as DerivedScoresReader})),
		).toEqual(["pg"])
		expect(fellBack("bestOrder", "client_failed")).toBe(2)
		expect(fellBack("bestOrder", "error")).toBe(1)
	})

	it("reads a list longer than one scores call in chunks", async () => {
		const ids = Array.from({length: 130}, (_, i) => i.toString(16).padStart(32, "0"))
		const scores = vi.fn(async (_c: ScoreConfig, cards: ScoreCard[]) => cards.map(() => scoreRow(1)))
		const ports = scoresPorts(Object.fromEntries(ids.map((id) => [id, 1])), {
			reader: {scores} as unknown as DerivedScoresReader,
		})
		expect(await serveBestOrder(ports, ids)).toHaveLength(130)
		expect(scores).toHaveBeenCalledTimes(3)
	})

	it("is the lag guard's to refuse: a stale geo-db never reaches the scores read", async () => {
		const scores = vi.fn()
		const serve = serveOf(["bestOrder"], {freshness: fresh(99)})
		const ports = scoresPorts({[A]: 1}, {reader: {scores} as unknown as DerivedScoresReader})
		expect(
			await serve.run(
				"bestOrder",
				() => serveBestOrder(ports, [A]),
				async () => ["pg"],
			),
		).toEqual(["pg"])
		expect(scores).not.toHaveBeenCalled()
	})
})

describe("entityColumns", () => {
	it("serves the fourteen columns from the served row, NULL for an entity with no entities row", async () => {
		const ports = scoresPorts({[A]: 42.5})
		const serve = serveOf(["entityColumns"])
		const out = await serve.run(
			"entityColumns",
			() => serveEntityColumns(ports, [A, B]),
			async () => new Map(),
		)
		expect(out.get(A)).toMatchObject({
			ranking_score: 42.5,
			upvotes: 3,
			downvotes: 1,
			agrees: 4,
			disagrees: 2,
			commenters: 5,
		})
		expect(out.get(B)).toMatchObject({ranking_score: null, upvotes: null})
		expect(servedBy("entityColumns", "geodb")).toBe(1)
	})

	it("falls back to Postgres on an error and on a timeout", async () => {
		const serve = serveOf(["entityColumns"], {timeoutMs: 10})
		const failing = scoresPorts({[A]: 1}, {cards: async () => Promise.reject(new GeoDbError("overloaded", "x"))})
		expect(
			await serve.run(
				"entityColumns",
				() => serveEntityColumns(failing, [A]),
				async () => new Map([["pg", {}]]) as never,
			),
		).toEqual(new Map([["pg", {}]]))
		const slow = scoresPorts({[A]: 1}, {config: () => new Promise(() => {})})
		await serve.run(
			"entityColumns",
			() => serveEntityColumns(slow, [A]),
			async () => new Map() as never,
		)
		expect(fellBack("entityColumns", "overloaded")).toBe(1)
		expect(fellBack("entityColumns", "timeout")).toBe(1)
	})
})

describe("trending", () => {
	const graph = {entities: [], spaces: [], relations: [], values: [], listings: [], suggestions: []}

	it("serves the twin's answer over the source's rows", async () => {
		const source = {today: async () => "2026-11-12", graph: vi.fn(async () => graph)}
		const serve = serveOf(["trending"])
		const out = await serve.run(
			"trending",
			() => serveTrending(source, [], (t) => t.currentDebateAgendas()),
			async () => [{agenda: "pg"}] as never,
		)
		expect(out).toEqual([])
		expect(source.graph).toHaveBeenCalledWith("2026-11-12", [])
		expect(servedBy("trending", "geodb")).toBe(1)
	})

	it("falls back to Postgres when there is no geo-db row source, and when the source fails", async () => {
		const serve = serveOf(["trending"])
		expect(
			await serve.run(
				"trending",
				() => serveTrending(undefined, [], () => 1),
				async () => 2,
			),
		).toBe(2)
		expect(fellBack("trending", "no_source")).toBe(1)
		const failing = {
			today: async () => "2026-11-12",
			graph: async () => {
				throw new GeoDbError("timeout", "slow")
			},
		}
		expect(
			await serve.run(
				"trending",
				() => serveTrending(failing, [], () => 1),
				async () => 2,
			),
		).toBe(2)
		expect(fellBack("trending", "timeout")).toBe(1)
	})
})

describe("feeds", () => {
	const data = {
		entities: [],
		relations: [],
		names: [],
		scores: [],
		blocklist: [],
		exclusions: [],
		typeRanking: [],
		topicRanking: [],
	}

	it("serves the twin's answer over the source's rows", async () => {
		const serve = serveOf(["feeds"])
		const out = await serve.run(
			"feeds",
			() =>
				serveFeeds({feedData: async () => data}, (f) =>
					f.entitiesRankedForFeed(0, null, null, null, null).map((e) => e.id),
				),
			async () => ["pg"],
		)
		expect(out).toEqual([])
		expect(servedBy("feeds", "geodb")).toBe(1)
	})

	it("falls back to Postgres when there is no geo-db row source, and when the source fails", async () => {
		const serve = serveOf(["feeds"])
		expect(
			await serve.run(
				"feeds",
				() => serveFeeds(undefined, () => 1),
				async () => 2,
			),
		).toBe(2)
		expect(fellBack("feeds", "no_source")).toBe(1)
		const failing = {
			feedData: async () => {
				throw new Error("boom")
			},
		}
		expect(
			await serve.run(
				"feeds",
				() => serveFeeds(failing, () => 1),
				async () => 2,
			),
		).toBe(2)
		expect(fellBack("feeds", "error")).toBe(1)
	})
})
