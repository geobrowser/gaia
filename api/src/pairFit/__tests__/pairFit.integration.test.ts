/**
 * `/internal/pair-fit` end to end against a real, migrated Postgres (DATABASE_URL), so the SQL in
 * queries.ts runs against the real user_votes, account weights (0101) and interest read (0100)
 * rather than a mock. Fixtures use their own ids and are removed afterwards.
 */

import {drizzle} from "drizzle-orm/node-postgres"
import {Hono} from "hono"
import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {INTERNAL_TOKEN_HEADER, mountInternalRoutes} from "../../forYou/router"

const TOKEN = "t".repeat(40)
const NAME = "a126ca53-0c8e-48d5-b888-82c734c38935"
const TOPICS = "806d52bc-27e9-4c91-93c0-57978b093351"

const u = (n: number) => `0f0f3224-0000-4000-8000-${n.toString(16).padStart(12, "0")}`
const dashless = (id: string) => id.replaceAll("-", "")
const USER = u(1)
const RIVAL = u(2) // disagrees with USER on the AI claim, cares about AI
const FELLOW = u(3) // shares USER's interest in Energy, no shared votes
const JUNK = u(4) // on the exclusion list, and disagrees on everything
const STRANGER = u(5) // no data at all
const ALLY = u(6) // agrees with USER on the AI claim
const SPACE = u(0x10)
const SPACE_2 = u(0x11)
const T_AI = u(0xa1)
const T_ENERGY = u(0xa2)
const C_AI = u(0xc1)
const C_ENERGY = u(0xc2)
const PEOPLE = [USER, RIVAL, FELLOW, JUNK, STRANGER, ALLY]
const ALL = [...PEOPLE, SPACE, SPACE_2, T_AI, T_ENERGY, C_AI, C_ENERGY]

let pool: Pool
let app: Hono

async function cleanup() {
	await pool.query("DELETE FROM relations WHERE from_entity_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM values WHERE entity_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM user_votes WHERE user_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM personalization.user_topic_signals WHERE user_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM account_weights WHERE user_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM account_exclusions WHERE user_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM entities WHERE id = ANY($1::uuid[])", [ALL])
}

const post = (body: unknown, token: string | null = TOKEN, on: Hono = app) =>
	on.request("/internal/pair-fit", {
		method: "POST",
		headers: {"content-type": "application/json", ...(token ? {[INTERNAL_TOKEN_HEADER]: token} : {})},
		body: JSON.stringify(body),
	})

// vote_type 0 = Agree, 1 = Disagree, 2 = removed; vote_kind 1 = stance.
async function stance(user: string, claim: string, voteType: 0 | 1 | 2, space = SPACE, at = "now()") {
	await pool.query(
		`INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
		 VALUES ($1, $2, 0, $3, $4, 1, ${at})`,
		[user, claim, space, voteType],
	)
}

async function interest(user: string, topic: string, count: number) {
	await pool.query(
		`INSERT INTO personalization.user_topic_signals
		   (user_id, topic_id, kind, decayed_count, event_count, last_event_at, top_object_id, computed_at)
		 VALUES ($1, $2, 'vote', $3, $4, now(), $2, now())`,
		[user, topic, count, count],
	)
}

describe.skipIf(!process.env.DATABASE_URL)("/internal/pair-fit", () => {
	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		app = new Hono()
		mountInternalRoutes(app, drizzle(pool), TOKEN)
		await cleanup()
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT id, '1790000000', '0', '1790000000', '0' FROM unnest($1::uuid[]) id`,
			[ALL],
		)
		await pool.query(
			`INSERT INTO values (id, property_id, entity_id, space_id, text) VALUES
			 ('it-pf-1', $1, $2, $6, 'AI safety'), ('it-pf-2', $1, $3, $6, 'Energy'),
			 ('it-pf-3', $1, $4, $6, 'Frontier AI needs a licence'), ('it-pf-4', $1, $5, $6, 'Nuclear is green')`,
			[NAME, T_AI, T_ENERGY, C_AI, C_ENERGY, SPACE],
		)
		await pool.query(
			`INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
			 SELECT gen_random_uuid(), gen_random_uuid(), $1, f, t, $2, false
			 FROM (VALUES ($3::uuid, $4::uuid), ($5::uuid, $6::uuid)) v(f, t)`,
			[TOPICS, SPACE, C_AI, T_AI, C_ENERGY, T_ENERGY],
		)
		await interest(USER, T_AI, 4)
		await interest(USER, T_ENERGY, 2)
		await interest(RIVAL, T_AI, 3)
		await interest(FELLOW, T_ENERGY, 5)
		await interest(JUNK, T_AI, 3)
		await interest(ALLY, T_AI, 3)

		// USER agrees on the AI claim. RIVAL agreed in one space and later disagreed in another: the
		// later stance is the one that counts. USER's removed Energy stance is no position, so
		// nobody shares it.
		await stance(USER, C_AI, 0)
		await stance(RIVAL, C_AI, 0, SPACE, "now() - interval '2 days'")
		await stance(RIVAL, C_AI, 1, SPACE_2, "now() - interval '1 day'")
		await stance(ALLY, C_AI, 0)
		await stance(JUNK, C_AI, 1)
		await stance(USER, C_ENERGY, 2)
		await stance(FELLOW, C_ENERGY, 1)

		await pool.query(
			`INSERT INTO account_weights (user_id, weight, reasons, vote_count, stance_vote_count, computed_at)
			 SELECT id, 1, '[]', 1, 1, now() FROM unnest($1::uuid[]) id`,
			[[USER, RIVAL, FELLOW, ALLY]],
		)
		await pool.query(
			`INSERT INTO account_weights (user_id, weight, reasons, vote_count, stance_vote_count, computed_at)
			 VALUES ($1, 0, '[{"code":"excluded"}]', 1, 1, now())`,
			[JUNK],
		)
		await pool.query(`INSERT INTO account_exclusions (user_id, reason, source) VALUES ($1, 'test account', 'it')`, [
			JUNK,
		])
	})

	afterAll(async () => {
		if (!pool) return
		await cleanup()
		await pool.end()
	})

	it("does not exist without the token configured (404), and refuses a wrong or missing one (401)", async () => {
		for (const token of [undefined, "", "short"]) {
			const bare = new Hono()
			expect(mountInternalRoutes(bare, drizzle(pool), token)).not.toBe("enabled")
			expect((await post({userId: USER, candidateIds: [RIVAL]}, TOKEN, bare)).status).toBe(404)
		}
		expect((await post({userId: USER, candidateIds: [RIVAL]}, null)).status).toBe(401)
		expect((await post({userId: USER, candidateIds: [RIVAL]}, "x".repeat(40))).status).toBe(401)
	})

	it("ranks the disagreeing pair first, explains each, and leaves out the excluded account", async () => {
		const res = await post({
			userId: dashless(USER),
			candidateIds: [STRANGER, ALLY, FELLOW, JUNK, RIVAL].map(dashless),
		})
		expect(res.status).toBe(200)
		expect(res.headers.get("cache-control")).toBe("private, no-store")
		const body = (await res.json()) as any
		expect(body.ranking).toEqual({name: "pair-fit", version: "pair-fit-1"})
		expect(body.excluded).toEqual([{userId: dashless(JUNK), reason: "excluded_account"}])
		expect(body.items.map((i: any) => i.userId)).toEqual([RIVAL, ALLY, FELLOW, STRANGER].map(dashless))

		const [rival, ally, fellow, stranger] = body.items
		expect(rival.disagreeing).toBe(true)
		expect(rival.parts).toMatchObject({sharedClaims: 1, opposed: 1, agreed: 0, accountWeight: 1})
		expect(rival.reason).toEqual({
			kind: "disagree",
			claimId: dashless(C_AI),
			name: "Frontier AI needs a licence",
			text: "You two disagree on Frontier AI needs a licence",
		})

		expect(ally.disagreeing).toBe(false)
		expect(ally.parts).toMatchObject({sharedClaims: 1, opposed: 0, agreed: 1, disagreement: 0})
		expect(ally.reason).toMatchObject({kind: "shared_topic", name: "AI safety"})

		// A removed stance is no position: FELLOW shares interest only, so is never disagreeing.
		expect(fellow.parts).toMatchObject({sharedClaims: 0, disagreement: 0})
		expect(fellow.disagreeing).toBe(false)
		expect(fellow.parts.interest).toBeGreaterThan(0)
		expect(fellow.reason).toMatchObject({kind: "shared_topic", text: "You both care about Energy"})

		expect(stranger).toMatchObject({score: 0, disagreeing: false, reason: null})
	})

	it("never answers with anyone's side", async () => {
		const text = await (await post({userId: USER, candidateIds: [RIVAL, ALLY]})).text()
		expect(text).not.toMatch(/vote_type|voteType|"agree"|"disagree"\s*:|position|stance/i)
	})

	it("answers an empty candidate list with nothing, and rejects malformed bodies", async () => {
		const body = (await (await post({userId: USER, candidateIds: []})).json()) as any
		expect(body.items).toEqual([])
		expect((await post({userId: "nope", candidateIds: []})).status).toBe(400)
		expect((await post({userId: USER, candidateIds: ["nope"]})).status).toBe(400)
		expect(
			(await post({userId: USER, candidateIds: Array.from({length: 301}, (_, i) => u(i + 0x100))})).status,
		).toBe(400)
	})
})
