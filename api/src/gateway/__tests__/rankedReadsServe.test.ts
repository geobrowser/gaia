import {parse} from "graphql"
import {Pool} from "pg"
import {afterAll, beforeAll, beforeEach, describe, expect, it, vi} from "vitest"
import type {DerivedCountsReader, DerivedScoresReader, ScoreCard, ScoreConfig, ScoreRow} from "../../geodb/derived"
import {
	__resetRankedReadsServeMetricsForTests,
	createRankedReadsServe,
	renderRankedReadsServeMetrics,
	type ScoresPorts,
	type ServeRead,
} from "../../geodb/rankedReadsServe"
import {graphqlServer} from "../../kg/postgraphile"
import type {ResolveContext} from "../gateway"
import {
	createBestOrderResolver,
	createTypeCountsResolver,
	planBestOrder,
	planTypeCountsServe,
} from "../rankedReadsServe"

/**
 * The wired ranked reads, against the real PostGraphile. Every served response must be the JSON PostGraphile gives for the same
 * query over the same rows: the fixture is seeded into Postgres, a truthful fake geo-db answers from the same rows, and the two
 * `data` objects are compared whole (key order and spelling included, via JSON.stringify of both).
 */

const undash = (id: string) => id.replaceAll("-", "")
const E = (n: number) => `31600000-0000-4000-8000-0000000000${n.toString(16).padStart(2, "0")}`
const HIGH = E(1)
const MID_A = E(2)
const MID_B = E(3)
const LOW = E(4)
const U1 = E(5)
const U2 = E(6)
const MISSING = E(0xff)
const SCORES: Record<string, number> = {[HIGH]: 0.9, [MID_A]: 0.5, [MID_B]: 0.5, [LOW]: 0.1}
const ALL = [HIGH, MID_A, MID_B, LOW, U1, U2]

const TOPIC = E(0x10)
const TYPE_X = E(0x21)
const TYPE_Y = E(0x22)
const TYPE_Z = E(0x23) // no rows: zero counts are absent
const BLOCKED = E(0x31)
// TYPE_X: HIGH, MID_A, MID_B (3). TYPE_Y: LOW and the blocked entity (1 once the blocklist applies).
const TOPIC_ROWS: [string, string][] = [
	[TYPE_X, HIGH],
	[TYPE_X, MID_A],
	[TYPE_X, MID_B],
	[TYPE_Y, LOW],
	[TYPE_Y, BLOCKED],
]

async function postgraphile(query: string, variables: Record<string, unknown> = {}) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query, variables}),
		}),
		{},
	)
	return (await response.json()) as {errors?: unknown; data?: Record<string, unknown>}
}

const context = (query: string, variables: Record<string, unknown> = {}): ResolveContext => ({
	classification: {operationName: null, operationType: "query", rootFields: []},
	params: {query, operationName: null, variables},
	document: parse(query),
	signal: new AbortController().signal,
})

const serveOf = (reads: ServeRead[], lag: number | null = 0) =>
	createRankedReadsServe({
		config: {reads: new Set(reads), timeoutMs: 1000, maxLagBlocks: 20, cacheMs: 0},
		freshness: {lagBlocks: async () => lag},
		log: {warn: vi.fn()},
	})

const CONFIG = {} as ScoreConfig
const card = (entityHex: string): ScoreCard => ({
	entityHex,
	propertyCount: 1,
	relationCount: 1,
	createdEpoch: 1,
	typeWeightMax: null,
})
const row = (ranking: number): ScoreRow => ({
	engaged: false,
	trendingSource: 0,
	counts: [0, 0, 0, 0, 0],
	quality: 0,
	intrinsic: 0,
	participation: 0,
	comment: 0,
	typeWeight: 1,
	watch: 0,
	ageCredit: 0,
	trending: 0,
	ranking,
	topicHex: null,
})

describe("wired ranked reads against PostGraphile", () => {
	let pool: Pool
	const rankingOf = async () => {
		// geo-db's served ranking: the stored score where there is one, and a default row (0.01) where there is none, as geo-db's does.
		const {rows} = await pool.query<{id: string; score: number}>(
			"SELECT entity_id::text AS id, ranking_score::float8 AS score FROM entity_ranking_scores WHERE entity_id = ANY($1::uuid[])",
			[ALL],
		)
		return new Map(rows.map((r) => [undash(r.id), r.score]))
	}

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await clean()
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT x, '1790000000', '0', '1790000000', '0' FROM unnest($1::uuid[]) AS t(x)`,
			[[...ALL, BLOCKED]],
		)
		for (const [id, score] of Object.entries(SCORES))
			await pool.query(
				"INSERT INTO entity_ranking_scores (entity_id, quality_score, ranking_score) VALUES ($1, $2, $2)",
				[id, score],
			)
		for (const [type, entity] of TOPIC_ROWS)
			await pool.query(
				"INSERT INTO entity_topic_ranking (topic_id, type_id, entity_id, ranking_score) VALUES ($1, $2, $3, 1)",
				[TOPIC, type, entity],
			)
		await pool.query("INSERT INTO entity_feed_blocklist (entity_id) VALUES ($1)", [BLOCKED])
	})

	async function clean() {
		await pool.query("DELETE FROM entity_feed_blocklist WHERE entity_id = $1", [BLOCKED])
		await pool.query("DELETE FROM entity_topic_ranking WHERE topic_id = $1", [TOPIC])
		await pool.query("DELETE FROM entity_ranking_scores WHERE entity_id = ANY($1::uuid[])", [ALL])
		await pool.query("DELETE FROM entities WHERE id = ANY($1::uuid[])", [[...ALL, BLOCKED]])
	}

	afterAll(async () => {
		await clean()
		await pool.end()
	})

	beforeEach(() => __resetRankedReadsServeMetricsForTests())

	describe("entitiesInBestOrder", () => {
		const ports = async (): Promise<ScoresPorts> => {
			const ranking = await rankingOf()
			return {
				reader: {
					scores: async (_c: ScoreConfig, cards: ScoreCard[]) =>
						cards.map((c) => row(ranking.get(c.entityHex) ?? 0.01)),
				} as unknown as DerivedScoresReader,
				// Every entity of the fixture has an entities row and so a card; MISSING has neither.
				cards: async (ids) =>
					new Map(ids.filter((id) => ALL.map(undash).includes(id)).map((id) => [id, card(id)])),
				config: async () => CONFIG,
				// What createPostgresScoresPorts reads: the ids with an entity_ranking_scores row.
				scored: async (ids) => new Set(ids.filter((id) => ranking.has(id))),
			}
		}
		const same = async (query: string, variables: Record<string, unknown> = {}) => {
			const resolver = createBestOrderResolver(serveOf(["bestOrder"]), await ports())
			const served = await resolver(context(query, variables))
			const pg = await postgraphile(query, variables)
			expect(pg.errors).toBeUndefined()
			expect(served.ok).toBe(true)
			if (!served.ok) return
			expect(JSON.stringify(served.data)).toBe(JSON.stringify(pg.data))
			return served.data
		}
		const list = (ids: string[]) => ids.map((id) => `"${id}"`).join(", ")

		it("is identical: scored by Best's order, unscored last in request order, missing and repeated ids dropped", async () => {
			const data = await same(
				`{ entitiesInBestOrder(entityIds: [${list([U2, LOW, MID_A, U1, HIGH, MISSING, MID_B, HIGH])}]) { id __typename } }`,
			)
			expect((data?.entitiesInBestOrder as {id: string}[]).map((e) => e.id)).toEqual(
				[HIGH, MID_B, MID_A, LOW, U2, U1].map(undash),
			)
		})

		it("is identical for an alias, a variable, a fragment, one field and an empty list", async () => {
			await same(`{ best: entitiesInBestOrder(entityIds: [${list([LOW, HIGH])}]) { key: id } }`)
			await same(`query Q($ids: [UUID]) { entitiesInBestOrder(entityIds: $ids) { id } }`, {ids: [MID_A, HIGH]})
			await same(
				`{ entitiesInBestOrder(entityIds: [${list([LOW, HIGH])}]) { ...F } } fragment F on Entity { id __typename }`,
			)
			await same(`{ entitiesInBestOrder(entityIds: []) { id } }`)
			await same(`{ entitiesInBestOrder(entityIds: [${list([MISSING])}]) { __typename } }`)
		})

		it("declines what it cannot answer exactly, so PostGraphile answers", async () => {
			const resolver = createBestOrderResolver(serveOf(["bestOrder"]), await ports())
			const decline = async (query: string) => {
				const out = await resolver(context(query))
				return out.ok ? "served" : out.reason
			}
			const ids = `entityIds: [${list([HIGH])}]`
			expect(await decline(`{ entitiesInBestOrder(${ids}) { id rankingScore } }`)).toBe("selection")
			expect(await decline(`{ entitiesInBestOrder(${ids}) { id name } }`)).toBe("selection")
			expect(await decline(`{ entitiesInBestOrder(${ids}, first: 1) { id } }`)).toBe("args")
			expect(await decline(`{ entitiesInBestOrder { id } }`)).toBe("args")
			expect(await decline(`{ entitiesInBestOrder(entityIds: null) { id } }`)).toBe("args")
			expect(await decline(`{ entitiesInBestOrder(entityIds: ["nope"]) { id } }`)).toBe("args")
			expect(await decline(`{ entitiesInBestOrder(${ids}) { id } entity(id: "${HIGH}") { id } }`)).toBe(
				"not_single_root",
			)
			const many = Array.from({length: 1001}, () => HIGH)
			expect(await decline(`{ entitiesInBestOrder(entityIds: [${list(many)}]) { id } }`)).toBe("truncated")
		})

		it("declines when geo-db fails, is behind or the read is not listed, and says why in the serve counters", async () => {
			const base = await ports()
			const failing = {...base, config: async () => null}
			const q = `{ entitiesInBestOrder(entityIds: [${list([HIGH])}]) { id } }`
			const out = await createBestOrderResolver(serveOf(["bestOrder"]), failing)(context(q))
			expect(out).toEqual({ok: false, reason: "ranked_read_fallback"})
			const stale = await createBestOrderResolver(serveOf(["bestOrder"], 99), base)(context(q))
			expect(stale.ok).toBe(false)
			const unlisted = await createBestOrderResolver(serveOf([]), base)(context(q))
			expect(unlisted.ok).toBe(false)
			expect(renderRankedReadsServeMetrics()).toContain(
				'gaia_api_ranked_read_serve_fallback_total{read="bestOrder"',
			)
		})

		it("plans the same ids the SQL takes: 32-hex or dashed, any case", () => {
			const plan = planBestOrder(
				parse(`{ entitiesInBestOrder(entityIds: ["${HIGH.toUpperCase()}", "${undash(LOW)}"]) { id } }`),
				null,
				{},
			)
			expect(plan.ids).toEqual([undash(HIGH), undash(LOW)])
		})
	})

	describe("topicFeedTypeCounts", () => {
		const reader = (): DerivedCountsReader =>
			({
				// A truthful geo-db: the count of Postgres rows per walked type, minus the blocked entities it is told about.
				typeCounts: async (_collection: number, topicHex: string, types: string[], blocked: string[]) => {
					const counts = types.map(
						(t) =>
							TOPIC_ROWS.filter(
								([type, entity]) =>
									undash(type) === t &&
									undash(TOPIC) === topicHex &&
									!blocked.includes(undash(entity)),
							).length,
					)
					return {walked: counts.length, counts}
				},
			}) as unknown as DerivedCountsReader
		const ports = () => ({
			reader: reader(),
			blocklist: async () =>
				(await pool.query<{id: string}>("SELECT entity_id::text AS id FROM entity_feed_blocklist")).rows.map(
					(r) => undash(r.id),
				),
		})
		const same = async (query: string, variables: Record<string, unknown> = {}) => {
			const resolver = createTypeCountsResolver(serveOf(["typeCounts"]), ports())
			const served = await resolver(context(query, variables))
			const pg = await postgraphile(query, variables)
			expect(pg.errors).toBeUndefined()
			expect(served.ok).toBe(true)
			if (!served.ok) return
			// The SQL fixes no row order, so the rows are compared by type id; every row's spelling must be identical.
			const byType = (data: Record<string, unknown>) =>
				Object.values(data).map((rows) =>
					[...(rows as {typeId?: string}[])].sort((a, b) => ((a.typeId ?? "") < (b.typeId ?? "") ? -1 : 1)),
				)
			expect(JSON.stringify(byType(served.data))).toBe(JSON.stringify(byType(pg.data ?? {})))
			return served.data
		}
		const args = `topicIds: ["${TOPIC}"], typeIds: ["${TYPE_X}", "${TYPE_Y}", "${TYPE_Z}"]`

		it("is identical: bigint counts as strings, dashless type ids, zero counts and blocked entities absent", async () => {
			const data = await same(`{ topicFeedTypeCounts(${args}) { typeId entityCount __typename } }`)
			expect(data?.topicFeedTypeCounts).toEqual([
				{typeId: undash(TYPE_X), entityCount: "3", __typename: "TopicFeedTypeCountsRecord"},
				{typeId: undash(TYPE_Y), entityCount: "1", __typename: "TopicFeedTypeCountsRecord"},
			])
		})

		it("is identical for an alias, variables, explicit flags and one selected field", async () => {
			await same(`{ c: topicFeedTypeCounts(${args}, matchAll: false) { t: typeId n: entityCount } }`)
			await same(`{ topicFeedTypeCounts(${args}, debateTaggedClaims: false, matchAll: true) { entityCount } }`)
			await same(
				`query Q($t: [UUID!]!, $y: [UUID!]!) { topicFeedTypeCounts(topicIds: $t, typeIds: $y) { typeId entityCount } }`,
				{t: [TOPIC], y: [TYPE_X]},
			)
			await same(`{ topicFeedTypeCounts(topicIds: ["${E(0x99)}"], typeIds: ["${TYPE_X}"]) { typeId } }`)
		})

		it("declines what it cannot answer exactly", async () => {
			const resolver = createTypeCountsResolver(serveOf(["typeCounts"]), ports())
			const decline = async (query: string) => {
				const out = await resolver(context(query))
				return out.ok ? "served" : out.reason
			}
			expect(await decline(`{ topicFeedTypeCounts(${args}) { typeId other: entityCount nope } }`)).toBe(
				"selection",
			)
			expect(await decline(`{ topicFeedTypeCounts(${args}, extra: 1) { typeId } }`)).toBe("args")
			expect(await decline(`{ topicFeedTypeCounts(typeIds: ["${TYPE_X}"]) { typeId } }`)).toBe("args")
			expect(await decline(`{ topicFeedTypeCounts(${args}, matchAll: null) { typeId } }`)).toBe("args")
			expect(await decline(`{ topicFeedTypeCounts(${args}, spaceIds: ["x"]) { typeId } }`)).toBe("args")
		})

		it("plans an omitted spaceIds and null spaceIds as no filter, and keeps a Space filter", () => {
			const plan = (extra: string) =>
				planTypeCountsServe(parse(`{ topicFeedTypeCounts(${args}${extra}) { typeId } }`), null, {}).call
			expect(plan("").spaces).toBeNull()
			expect(plan(", spaceIds: null").spaces).toBeNull()
			expect(plan(`, spaceIds: ["${E(0x40)}"]`).spaces).toEqual([undash(E(0x40))])
			expect(plan("")).toMatchObject({matchAll: true, debateTaggedClaims: false})
		})

		it("declines when geo-db fails", async () => {
			const failing = {
				...ports(),
				reader: {
					typeCounts: async () => {
						throw new Error("boom")
					},
				} as unknown as DerivedCountsReader,
			}
			const out = await createTypeCountsResolver(
				serveOf(["typeCounts"]),
				failing,
			)(context(`{ topicFeedTypeCounts(${args}) { typeId } }`))
			expect(out).toEqual({ok: false, reason: "ranked_read_fallback"})
		})
	})
})
