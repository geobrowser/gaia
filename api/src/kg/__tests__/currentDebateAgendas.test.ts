import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

/**
 * GEO-3503: `currentDebateAgendas(spaceIds, earlierDays)` through the real GraphQL server, so
 * PostGraphile's handling of the function's row order and the default page size are part of what
 * is tested. The rules themselves are covered by drizzle/tests/0115_current_debate_agendas.sql.
 */

type Row = {
	agendaId: string
	source: string
	sectionName: string | null
	entityId: string
	itemPosition: number
	isCurrent: boolean
	lastListedOn: string | null
}

async function query(source: string) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query: source}),
		}),
		{},
	)
	return response.json() as Promise<{errors?: Array<{message: string}>; data?: {currentDebateAgendas?: Row[]}}>
}

const undash = (id: string) => id.replaceAll("-", "")
const id = (n: number) => `35030000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`

const SPACE = id(1)
const AGENDA = id(2)
const BLOCK = id(3)
const PAGE = id(4)
// 120 claims, so a page larger than the default 100 is needed to read them all.
const CLAIMS = Array.from({length: 120}, (_, i) => id(100 + i))
const ALL = [AGENDA, BLOCK, PAGE, ...CLAIMS]

const TYPES = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"
const CLAIM_TYPE = "96f859ef-a1ca-4b22-9372-c86ad58b694b"
const AGENDA_TYPE = "79e3757b-2630-492c-9d3b-d73a78d45893"
const DATE = "52665f3e-fb7d-48d5-8b6b-abb21b0d36db"
const NAME = "a126ca53-0c8e-48d5-b888-82c734c38935"
const BLOCKS = "beaba5cb-a677-41a8-b353-77030613fc70"
const COLLECTION_ITEM = "a99f9ce1-2ffa-4dac-8c61-f6310d46064a"

async function cleanup(pool: Pool) {
	await pool.query("DELETE FROM relations WHERE from_entity_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM values WHERE entity_id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM entities WHERE id = ANY($1::uuid[])", [ALL])
	await pool.query("DELETE FROM spaces WHERE id = $1", [SPACE])
}

describe("currentDebateAgendas", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await cleanup(pool)
		await pool.query("INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', 'addr-geo-3503')", [SPACE])
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT x, '1790000000', '0', '1790000000', '0' FROM unnest($1::uuid[]) AS t(x)`,
			[ALL],
		)
		const rel = (from: string, type: string, to: string, position: string | null = null) =>
			pool.query(
				`INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system, position)
				 VALUES (gen_random_uuid(), gen_random_uuid(), $1, $2, $3, $4, false, $5)`,
				[type, from, to, SPACE, position],
			)
		await rel(AGENDA, TYPES, AGENDA_TYPE)
		await pool.query(
			`INSERT INTO values (id, entity_id, property_id, space_id, datetime)
			 VALUES (gen_random_uuid()::text, $1, $2, $3, to_char(now() AT TIME ZONE 'America/New_York', 'YYYY-MM-DD') || 'T00:00:00-04:00')`,
			[AGENDA, DATE, SPACE],
		)
		await pool.query(
			"INSERT INTO values (id, entity_id, property_id, space_id, text) VALUES (gen_random_uuid()::text, $1, $2, $3, 'Elections')",
			[BLOCK, NAME, SPACE],
		)
		await rel(AGENDA, BLOCKS, BLOCK, "a0")
		// Inserted in reverse, with positions that put them back in order: the response must follow
		// the positions, not the insert order.
		for (const [i, claim] of [...CLAIMS.entries()].reverse()) {
			await rel(claim, TYPES, CLAIM_TYPE)
			await rel(BLOCK, COLLECTION_ITEM, claim, `a${i.toString().padStart(3, "0")}`)
		}
		// Not a claim: never an agenda item.
		await rel(BLOCK, COLLECTION_ITEM, PAGE, "a0005")
	})

	afterAll(async () => {
		await cleanup(pool)
		await pool.end()
	})

	const fields = "agendaId source sectionName entityId itemPosition isCurrent lastListedOn"

	it("returns the agenda's claims in position order, with their section and listing", async () => {
		const result = await query(`{ currentDebateAgendas(spaceIds: ["${SPACE}"], first: 1000) { ${fields} } }`)
		expect(result.errors).toBeUndefined()
		const rows = result.data?.currentDebateAgendas ?? []
		expect(rows.map((r) => r.entityId)).toEqual(CLAIMS.map(undash))
		expect(rows.every((r) => r.agendaId === undash(AGENDA) && r.source === "block_item" && r.isCurrent)).toBe(true)
		expect(rows.every((r) => r.sectionName === "Elections")).toBe(true)
		expect(rows[0]?.itemPosition).toBe(1)
		// The listings table has not seen this agenda; the row itself is the listing.
		expect(rows.every((r) => r.lastListedOn === rows[0]?.lastListedOn && r.lastListedOn !== null)).toBe(true)
	})

	it("is capped at the API's default page size unless `first` asks for more", async () => {
		const result = await query(`{ currentDebateAgendas(spaceIds: ["${SPACE}"]) { entityId } }`)
		expect(result.errors).toBeUndefined()
		expect(result.data?.currentDebateAgendas?.map((r) => r.entityId)).toEqual(CLAIMS.slice(0, 100).map(undash))
	})

	// This API's connections carry edges and totalCount, not pageInfo.
	type Page = {totalCount: number; edges: Array<{cursor: string; node: {entityId: string}}>}
	const page = async (args: string) => {
		const result = (await query(
			`{ currentDebateAgendasConnection(spaceIds: ["${SPACE}"], ${args}) { totalCount edges { cursor node { entityId } } } }`,
		)) as {errors?: Array<{message: string}>; data?: {currentDebateAgendasConnection?: Page}}
		expect(result.errors).toBeUndefined()
		return result.data?.currentDebateAgendasConnection as Page
	}

	it("pages through the connection by cursor and by offset in the same order", async () => {
		const byCursor: string[] = []
		let after = ""
		for (let round = 0; round < 10; round++) {
			const p = await page(`first: 50${after ? `, after: "${after}"` : ""}`)
			byCursor.push(...p.edges.map((e) => e.node.entityId))
			if (byCursor.length >= p.totalCount || p.edges.length === 0) break
			after = p.edges[p.edges.length - 1]?.cursor ?? ""
		}
		expect(byCursor).toEqual(CLAIMS.map(undash))

		const byOffset = [...(await page("first: 70")).edges, ...(await page("first: 70, offset: 70")).edges]
		expect(byOffset.map((e) => e.node.entityId)).toEqual(CLAIMS.map(undash))
	})
})
