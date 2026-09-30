import {drizzle} from "drizzle-orm/node-postgres"
import {Effect} from "effect"
import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {listProposalsInSpace} from "../../proposals/queries"
import {computeProposalStatus} from "../../proposals/status"
import {PROPOSAL_STATUSES, type ProposalStatus} from "../../proposals/types"

/**
 * Runs the SQL status fragments in `proposals/queries.ts` against a real
 * database and checks each row lands in exactly the status
 * `computeProposalStatus` gives it. The parity tests in
 * `proposals/__tests__/queries.test.ts` only exercise the TypeScript side.
 *
 * The cases centre on `execute_by` (GEO-2609): past the deadline the contract's
 * `canExecuteProposal` is false, so a passed-but-unexecuted proposal must read
 * REJECTED, not EXECUTABLE; a NULL or zero `execute_by` is no window at all.
 */

const SPACE = "00000000-cafe-4000-8000-00000000f609"
const EDITOR = "00000000-cafe-4000-8000-00000000e609"
const DAY = 86_400
const now = Math.floor(Date.now() / 1000)

interface Case {
	name: string
	expected: ProposalStatus
	mode: "Fast" | "Slow"
	endTime: number
	executeBy: number | null
	yes: number
	executedAt?: number
}

const CASES: Case[] = [
	// Slow, passed its late vote, never executed, deadline long gone: the shape of 995f9dda… on testnet.
	{
		name: "slow passed, window closed",
		expected: "REJECTED",
		mode: "Slow",
		endTime: now - 89 * DAY,
		executeBy: now - 82 * DAY,
		yes: 1,
	},
	{
		name: "slow passed, window open",
		expected: "EXECUTABLE",
		mode: "Slow",
		endTime: now - 10,
		executeBy: now + DAY,
		yes: 1,
	},
	{
		name: "fast passed, window closed",
		expected: "REJECTED",
		mode: "Fast",
		endTime: now + DAY,
		executeBy: now - 10,
		yes: 5,
	},
	{
		name: "fast passed, execute_by null",
		expected: "EXECUTABLE",
		mode: "Fast",
		endTime: now + DAY,
		executeBy: null,
		yes: 5,
	},
	{
		name: "fast passed, execute_by zero",
		expected: "EXECUTABLE",
		mode: "Fast",
		endTime: now + DAY,
		executeBy: 0,
		yes: 5,
	},
	{
		name: "fast passed, at the boundary",
		expected: "EXECUTABLE",
		mode: "Fast",
		endTime: now + DAY,
		executeBy: now + 60,
		yes: 5,
	},
	{name: "fast open, execute_by zero", expected: "PROPOSED", mode: "Fast", endTime: now + DAY, executeBy: 0, yes: 0},
	{
		name: "fast open, window closed",
		expected: "REJECTED",
		mode: "Fast",
		endTime: now + DAY,
		executeBy: now - 10,
		yes: 0,
	},
	{
		name: "executed, window closed",
		expected: "ACCEPTED",
		mode: "Fast",
		endTime: now - DAY,
		executeBy: now - 10,
		yes: 5,
		executedAt: now - 2 * DAY,
	},
]

const ids = CASES.map((_, i) => `00000000-cafe-4000-8000-0000000006${String(i).padStart(2, "0")}`)

describe("proposal status SQL matches computeProposalStatus (execute_by)", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await pool.query(`DELETE FROM proposal_versions WHERE proposal_id = ANY($1::uuid[])`, [ids])
		await pool.query(`DELETE FROM proposals WHERE id = ANY($1::uuid[])`, [ids])
		await pool.query(`INSERT INTO spaces (id, type, address) VALUES ($1, 'DAO', '0xtest') ON CONFLICT DO NOTHING`, [
			SPACE,
		])
		for (const [i, c] of CASES.entries()) {
			await pool.query(
				`INSERT INTO proposals (id, space_id, proposed_by, created_at, created_at_block, current_version, executed_at)
				 VALUES ($1, $2, $3, $4, $5, 1, $6)`,
				[ids[i], SPACE, EDITOR, now - 100 * DAY + i, 100 + i, c.executedAt ?? null],
			)
			// Fast: flat threshold 3, so 5 yes passes and 0 does not. Slow: 51% partial, quorum 1.
			await pool.query(
				`INSERT INTO proposal_versions (proposal_id, proposal_version, voting_mode, start_time, end_time,
				   quorum, threshold, partial_percentage_support_threshold, universal_percentage_support_threshold,
				   flat_support_threshold, execute_by, yes_count, no_count, abstain_count,
				   version_created_at, version_created_at_block)
				 VALUES ($1, 1, $2, $3, $4, 1, $5, $6, 0, $7, $8, $9, 0, 0, $10, 100)`,
				[
					ids[i],
					c.mode,
					c.endTime - DAY,
					c.endTime,
					c.mode === "Fast" ? 3 : 5_100_000,
					c.mode === "Slow" ? 5_100_000 : 0,
					c.mode === "Fast" ? 3 : 0,
					c.executeBy,
					c.yes,
					c.endTime - DAY,
				],
			)
		}
	})

	afterAll(async () => {
		await pool.query(`DELETE FROM proposal_versions WHERE proposal_id = ANY($1::uuid[])`, [ids])
		await pool.query(`DELETE FROM proposals WHERE id = ANY($1::uuid[])`, [ids])
		await pool.end()
	})

	async function idsWithStatus(status: ProposalStatus): Promise<Set<string>> {
		const db = drizzle(pool)
		const {proposals} = await Effect.runPromise(
			listProposalsInSpace(db, {spaceId: SPACE, limit: 100, status: [status]}),
		)
		return new Set(proposals.map((p) => p.id))
	}

	it("puts every case in exactly its expected status, in SQL and in TypeScript", async () => {
		const bySql = new Map<ProposalStatus, Set<string>>()
		for (const s of PROPOSAL_STATUSES) bySql.set(s, await idsWithStatus(s))

		const db = drizzle(pool)
		const {proposals} = await Effect.runPromise(listProposalsInSpace(db, {spaceId: SPACE, limit: 100}))
		const rows = new Map(proposals.map((p) => [p.id, p]))

		for (const [i, c] of CASES.entries()) {
			const id = ids[i] as string
			const sqlStatuses = PROPOSAL_STATUSES.filter((s) => bySql.get(s)?.has(id))
			expect(sqlStatuses, `${c.name}: SQL`).toEqual([c.expected])

			const row = rows.get(id)
			if (!row) throw new Error(`${c.name}: not listed`)
			expect(computeProposalStatus(row, BigInt(now)).status, `${c.name}: TypeScript`).toBe(c.expected)
		}
	})
})
