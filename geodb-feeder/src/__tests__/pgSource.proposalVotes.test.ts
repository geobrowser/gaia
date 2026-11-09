/**
 * readProposalVoteRows against a real Postgres (GEODB_FEEDER_TEST_PG_URL, as pgSource.votes.test.ts). The keyset
 * WHERE compares the uuid/int primary key, so the ORDER BY must sort by the same columns, not the `::text` output
 * aliases; under ICU numeric collation (`und-u-kn-true`) text order and uuid order disagree, so a wrong ORDER BY
 * makes paging skip or repeat rows. The table is gaia's (api/drizzle 0011 + 0067), without its foreign keys.
 */
import {randomUUID} from "node:crypto"
import pg from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {ReadOnlyPg, readProposalVoteRows} from "../pgSource.js"
import {type ProposalVoteRow, proposalVoteFromRow} from "../proposalVotesKeyed.js"

const url = process.env.GEODB_FEEDER_TEST_PG_URL

describe.skipIf(!url)("readProposalVoteRows on a real Postgres", () => {
	const dbName = `pvotes_order_${randomUUID().replaceAll("-", "").slice(0, 12)}`
	const dbUrl = () => {
		const u = new URL(url as string)
		u.pathname = `/${dbName}`
		return u.toString()
	}
	const inserted: string[] = []
	let admin: pg.Client
	let ro: ReadOnlyPg

	beforeAll(async () => {
		admin = new pg.Client({connectionString: url})
		await admin.connect()
		await admin.query(
			`CREATE DATABASE ${dbName} TEMPLATE template0 LOCALE_PROVIDER icu ICU_LOCALE 'und-u-kn-true' LOCALE 'C'`,
		)
		const c = new pg.Client({connectionString: dbUrl()})
		await c.connect()
		await c.query(`CREATE TYPE "voteOption" AS ENUM('Yes', 'No', 'Abstain')`)
		await c.query(`CREATE TABLE proposal_votes (
			proposal_id uuid NOT NULL, voter_id uuid NOT NULL, space_id uuid NOT NULL, vote "voteOption" NOT NULL,
			created_at text NOT NULL, created_at_block text NOT NULL, proposal_version integer NOT NULL,
			PRIMARY KEY (proposal_id, proposal_version, voter_id))`)
		// A few proposals with several versions and voters each, so pages break inside one proposal and version.
		const options = ["Yes", "No", "Abstain"]
		// The first two ids are fixed: numeric collation sorts 9a... before 10... (9 < 10) while uuid byte order sorts 0x10 before 0x9a,
		// so the text and uuid orders differ on every run. The rest are random.
		const fixedProposals = ["9a000000-0000-4000-8000-000000000000", "10000000-0000-4000-8000-000000000000"]
		for (let p = 0; p < 10; p++) {
			const proposal = fixedProposals[p] ?? randomUUID()
			const space = randomUUID()
			for (const version of [1, 2, 3, -2147483648]) {
				for (let v = 0; v < 4; v++) {
					const voter = randomUUID()
					await c.query("INSERT INTO proposal_votes VALUES ($1, $2, $3, $4, $5, $6, $7)", [
						proposal,
						voter,
						space,
						options[v % 3],
						String(1_700_000_000 + v),
						String(100 + v),
						version,
					])
					inserted.push(`${proposal}|${version}|${voter}`)
				}
			}
		}
		await c.end()
		ro = new ReadOnlyPg({connectionString: dbUrl()})
	}, 60_000)

	afterAll(async () => {
		await ro?.close()
		await admin?.query(`DROP DATABASE IF EXISTS ${dbName} WITH (FORCE)`)
		await admin?.end()
	})

	it("the fixture's text order really differs from uuid order", async () => {
		const [r] = await ro.read<{same: boolean}>(
			`SELECT array_agg(proposal_id::text ORDER BY proposal_id::text) = array_agg(proposal_id::text ORDER BY proposal_id) AS same
			 FROM (SELECT DISTINCT proposal_id FROM proposal_votes) p`,
		)
		expect(r?.same).toBe(false)
	})

	it("pages every row exactly once, at small page sizes", async () => {
		for (const pageSize of [1, 7, 16, 1000]) {
			const seen: string[] = []
			for await (const r of readProposalVoteRows(ro, pageSize)) seen.push(`${r.proposal_id}|${r.proposal_version}|${r.voter_id}`)
			expect(seen.length).toBe(inserted.length)
			expect(new Set(seen)).toEqual(new Set(inserted))
		}
	})

	it("reads each column as the backfill expects, and every row converts", async () => {
		const rows: ProposalVoteRow[] = []
		for await (const r of readProposalVoteRows(ro, 50)) rows.push(r)
		const r = rows.find((x) => x.proposal_version < 0) as ProposalVoteRow
		expect(r.proposal_id).toMatch(/^[0-9a-f-]{36}$/)
		expect(r.proposal_version).toBe(-2147483648)
		expect(["Yes", "No", "Abstain"]).toContain(r.vote)
		expect(r.created_at).toMatch(/^17000000\d\d$/)
		for (const x of rows) expect(() => proposalVoteFromRow(x)).not.toThrow()
	})
})
