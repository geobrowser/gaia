/**
 * `/internal/for-you` end to end against a real, migrated Postgres (DATABASE_URL), so the SQL in
 * queries.ts runs against the real 0100 / 0102 functions rather than a mock that would accept
 * anything. Fixtures use their own ids and are removed afterwards.
 */

import {drizzle} from "drizzle-orm/node-postgres"
import {Hono} from "hono"
import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {createInternalRouter, INTERNAL_TOKEN_HEADER} from "../router"

const TOKEN = "t".repeat(40)
const NAME = "a126ca53-0c8e-48d5-b888-82c734c38935"
const TOPICS = "806d52bc-27e9-4c91-93c0-57978b093351"

const u = (n: number) => `0f0f0000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`
const dashless = (id: string) => id.replaceAll("-", "")
const USER = u(1)
const NEWCOMER = u(2)
const SPACE = u(3)
const T_AI = u(0xa1)
const T_ENERGY = u(0xa2)
// Best's window, in Best's order: c1 (Energy), c2 (AI, voted on), c3 (AI), c4 (untagged), c5 (AI).
const C = [u(0xc1), u(0xc2), u(0xc3), u(0xc4), u(0xc5)] as const
const ALL = [USER, NEWCOMER, SPACE, T_AI, T_ENERGY, ...C]

let pool: Pool
let app: Hono

async function cleanup() {
	await pool.query("DELETE FROM relations WHERE from_entity_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM values WHERE entity_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM entity_ranking_scores WHERE entity_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM user_votes WHERE user_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM personalization.user_topic_signals WHERE user_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM personalization.feed_experiments WHERE id = 'it-for-you'")
	await pool.query("DELETE FROM entities WHERE id = ANY($1::uuid[])", [ALL])
}

const post = (body: unknown, token: string | null = TOKEN) =>
	app.request("/internal/for-you", {
		method: "POST",
		headers: {"content-type": "application/json", ...(token ? {[INTERNAL_TOKEN_HEADER]: token} : {})},
		body: JSON.stringify(body),
	})

describe.skipIf(!process.env.DATABASE_URL)("/internal/for-you", () => {
	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		app = new Hono()
		app.route("/internal", createInternalRouter(drizzle(pool), TOKEN))
		await cleanup()
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT id, '1790000000', '0', '1790000000', '0' FROM unnest($1::uuid[]) id`,
			[ALL],
		)
		await pool.query(
			`INSERT INTO values (id, property_id, entity_id, space_id, text) VALUES
			 ('it-fy-1', $1, $2, $4, 'AI safety'), ('it-fy-2', $1, $3, $4, 'Energy')`,
			[NAME, T_AI, T_ENERGY, SPACE],
		)
		// Scores 0.8 units apart (just under a day at tau 100000), Best's order c1 > c2 > c3 > c4 > c5.
		await pool.query(
			`INSERT INTO entity_ranking_scores (entity_id, quality_score, intrinsic_score, participation_score,
			   ranking_score, positive, negative, stance_positive, stance_negative, type_weight, updated_at)
			 SELECT c, 0.5, 0, 0, 17900 - (n - 1) * 0.8, 0, 0, 0, 0, 1, now()
			 FROM unnest($1::uuid[]) WITH ORDINALITY AS t(c, n)`,
			[C],
		)
		await pool.query(
			`INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
			 SELECT gen_random_uuid(), gen_random_uuid(), $1, f, t, $2, false
			 FROM (VALUES ($3::uuid, $4::uuid), ($5::uuid, $6::uuid), ($7::uuid, $6::uuid), ($8::uuid, $6::uuid)) v(f, t)`,
			[TOPICS, SPACE, C[0], T_ENERGY, C[1], T_AI, C[2], C[4]],
		)
		await pool.query(
			`INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
			 VALUES ($1, $2, 0, $3, 0, 1, now())`,
			[USER, C[1], SPACE],
		)
		// What the sweep would have stored: four fresh votes on AI safety.
		await pool.query(
			`INSERT INTO personalization.user_topic_signals
			   (user_id, topic_id, kind, decayed_count, event_count, last_event_at, top_object_id, computed_at)
			 VALUES ($1, $2, 'vote', 4, 4, now(), $3, now())`,
			[USER, T_AI, C[1]],
		)
	})

	afterAll(async () => {
		if (!pool) return
		await cleanup()
		await pool.end()
	})

	it("refuses a request without the token, or with the wrong one", async () => {
		expect((await post({userId: USER, candidateIds: C}, null)).status).toBe(401)
		expect((await post({userId: USER, candidateIds: C}, "x".repeat(40))).status).toBe(401)
	})

	it("re-orders Best's window by the user's interest, drops what they answered, and explains", async () => {
		const res = await post({userId: dashless(USER), candidateIds: C.map(dashless)})
		expect(res.status).toBe(200)
		expect(res.headers.get("cache-control")).toBe("private, no-store")
		const body = (await res.json()) as any
		expect(body.personalized).toBe(true)
		expect(body.ranking.name).toBe("for-you")
		expect(body.ranking.version).toMatch(/^for-you-1\.\d+$/)
		expect(body.excluded).toEqual([{entityId: dashless(C[1]), reason: "voted"}])
		// Four votes on AI safety are worth 6 * 4 / 6 = 4 days (3.456 units): c3 (0.8 behind c1) and c5 (3.2
		// behind) both pass c1; c4, untagged, stays below.
		expect(body.items.map((i: any) => i.entityId)).toEqual([C[2], C[4], C[0], C[3]].map(dashless))
		const c3 = body.items[0]
		expect(c3.reason).toMatchObject({topicName: "AI safety", kind: "vote", count: 4, text: "4 votes on AI safety"})
		expect(c3.score.boost).toBeCloseTo(4 * 0.864, 3)
		expect(c3.score.total).toBeCloseTo(c3.score.best + c3.score.boost, 9)
		expect(body.items.find((i: any) => i.entityId === dashless(C[0])).reason).toBeNull()
	})

	it("gives a user with no interests Best unchanged and unpersonalized", async () => {
		const body = (await (await post({userId: NEWCOMER, candidateIds: C})).json()) as any
		expect(body.personalized).toBe(false)
		expect(body.items).toEqual([])
		expect(body.ranking.version).toMatch(/^for-you-1\.\d+$/)
	})

	it("bumps the version when a tunable changes", async () => {
		const before = ((await (await post({userId: USER, candidateIds: C})).json()) as any).ranking.version
		await pool.query("UPDATE personalization.for_you_config SET exploration_share = exploration_share")
		const after = ((await (await post({userId: USER, candidateIds: C})).json()) as any).ranking.version
		expect(after).not.toBe(before)
	})

	it("reports the user's feed experiment", async () => {
		await pool.query("DELETE FROM personalization.feed_experiments WHERE active")
		await pool.query(
			"INSERT INTO personalization.feed_experiments (id, active, arm_a, arm_b, share) VALUES ('it-for-you', true, 'best', 'for-you', 1)",
		)
		const body = (await (await post({userId: USER, candidateIds: C})).json()) as any
		expect(body.experiment).toEqual({
			id: "it-for-you",
			arms: ["best", "for-you"],
			interleaved: true,
			assignment: "hash",
		})
	})

	it("rejects malformed bodies", async () => {
		expect((await post({userId: "nope", candidateIds: C})).status).toBe(400)
		expect((await post({userId: USER, candidateIds: ["nope"]})).status).toBe(400)
		expect((await post({userId: USER, candidateIds: Array.from({length: 501}, (_, i) => u(i + 100))})).status).toBe(
			400,
		)
	})
})
