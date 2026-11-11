/**
 * statementBudget against a real Postgres (DATABASE_URL): the budget is in force inside each
 * statement, nothing is left on the session afterwards (the property PgBouncer transaction
 * mode needs), a statement over budget is cancelled by the server, writes commit, a caller's
 * own transaction keeps working and can override the budget, and the client stays usable
 * after a failure.
 */

import {sql} from "drizzle-orm"
import {drizzle} from "drizzle-orm/node-postgres"
import {Client, Pool, type PoolClient} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {applyStatementBudget, useStatementBudget} from "../statementBudget"

const BUDGET_MS = 250
const TABLE = `statement_budget_probe_${process.pid}`

let client: Client
// The client's own query, captured before wrapping: sees the session exactly as the server does.
let raw: (text: string) => Promise<{rows: Record<string, unknown>[]}>
let pool: Pool

beforeAll(async () => {
	client = new Client({connectionString: process.env.DATABASE_URL})
	await client.connect()
	raw = client.query.bind(client) as typeof raw
	applyStatementBudget(client as unknown as PoolClient, BUDGET_MS)
	await raw(`CREATE TABLE IF NOT EXISTS ${TABLE} (id int PRIMARY KEY)`)

	pool = new Pool({connectionString: process.env.DATABASE_URL, max: 2})
	useStatementBudget(pool, BUDGET_MS)
})

afterAll(async () => {
	await raw(`DROP TABLE IF EXISTS ${TABLE}`)
	await client.end()
	await pool.end()
})

describe("statementBudget on a real server", () => {
	it("puts the budget in force inside the statement", async () => {
		const res = await client.query("SELECT current_setting('statement_timeout') AS t")
		expect(res.rows[0]?.t).toBe(`${BUDGET_MS}ms`)
	})

	it("leaves nothing on the session afterwards", async () => {
		await client.query("SELECT 1")
		const res = await raw("SELECT current_setting('statement_timeout') AS t")
		expect(res.rows[0]?.t).toBe("0")
		// And no transaction is left open.
		const status = await raw("SELECT now() = statement_timestamp() AS autocommit")
		expect(status.rows[0]?.autocommit).toBe(true)
	})

	it("has the server cancel a statement that runs over budget", async () => {
		await expect(client.query("SELECT pg_sleep(2)")).rejects.toMatchObject({code: "57014"})
	})

	it("keeps the client usable after a failed statement", async () => {
		await expect(client.query("SELECT 1/0")).rejects.toMatchObject({code: "22012"})
		const res = await client.query("SELECT 41 + 1 AS n")
		expect(res.rows[0]?.n).toBe(42)
	})

	it("commits writes, visible to another session", async () => {
		await client.query(`INSERT INTO ${TABLE} (id) VALUES ($1) ON CONFLICT DO NOTHING`, [7])
		const other = new Client({connectionString: process.env.DATABASE_URL})
		await other.connect()
		try {
			const res = await other.query(`SELECT count(*)::int AS n FROM ${TABLE} WHERE id = 7`)
			expect(res.rows[0]?.n).toBe(1)
		} finally {
			await other.end()
		}
	})

	it("runs concurrent statements on one client each under the budget, with the right results", async () => {
		const results = await Promise.all(
			[1, 2, 3, 4, 5].map((i) =>
				client.query("SELECT $1::int AS i, current_setting('statement_timeout') AS t", [i]),
			),
		)
		expect(results.map((r) => r.rows[0])).toEqual([1, 2, 3, 4, 5].map((i) => ({i, t: `${BUDGET_MS}ms`})))
	})

	it("works through pool.query and through drizzle", async () => {
		const viaPool = await pool.query("SELECT current_setting('statement_timeout') AS t")
		expect(viaPool.rows[0]?.t).toBe(`${BUDGET_MS}ms`)

		const db = drizzle({client: pool})
		const viaDrizzle = await db.execute(sql`SELECT current_setting('statement_timeout') AS t`)
		expect(viaDrizzle.rows[0]?.t).toBe(`${BUDGET_MS}ms`)
	})

	it("gives a drizzle transaction the pool budget, and lets it set its own", async () => {
		const db = drizzle({client: pool})
		const seen = await db.transaction(
			async (tx) => {
				const before = await tx.execute(sql`SELECT current_setting('statement_timeout') AS t`)
				await tx.execute(sql`SET LOCAL statement_timeout = 2000`)
				const after = await tx.execute(sql`SELECT current_setting('statement_timeout') AS t`)
				return [before.rows[0]?.t, after.rows[0]?.t]
			},
			{accessMode: "read only"},
		)
		expect(seen).toEqual([`${BUDGET_MS}ms`, "2s"])

		// The pool is back to per-statement budgets once the transaction ends.
		const res = await pool.query("SELECT current_setting('statement_timeout') AS t")
		expect(res.rows[0]?.t).toBe(`${BUDGET_MS}ms`)
	})

	it("rolls back a failed drizzle transaction and keeps the pool usable", async () => {
		const db = drizzle({client: pool})
		await expect(
			db.transaction(async (tx) => {
				await tx.execute(sql.raw(`INSERT INTO ${TABLE} (id) VALUES (8)`))
				await tx.execute(sql`SELECT pg_sleep(2)`)
			}),
		).rejects.toThrow()
		const res = await pool.query(`SELECT count(*)::int AS n FROM ${TABLE} WHERE id = 8`)
		expect(res.rows[0]?.n).toBe(0)
	})
})
