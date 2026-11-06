/**
 * readProposalVersionRows against a real Postgres (GEODB_FEEDER_TEST_PG_URL, as pgSource.proposalActions.test.ts): the
 * keyset paging over (proposal_id, proposal_version) must page every row exactly once, and the columns must arrive
 * as the backfill expects. Table as gaia's (api/drizzle 0067), without the foreign key and reduced to the columns the
 * reader selects plus the ones it must not (threshold, the counts).
 */
import {randomUUID} from "node:crypto"
import pg from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {ReadOnlyPg, readProposalVersionRows} from "../pgSource.js"
import {proposalVersionFromRow} from "../proposalVersionsKeyed.js"

const url = process.env.GEODB_FEEDER_TEST_PG_URL

describe.skipIf(!url)("readProposalVersionRows on a real Postgres", () => {
	const dbName = `pversions_order_${randomUUID().replaceAll("-", "").slice(0, 12)}`
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
		await admin.query(`CREATE DATABASE ${dbName} TEMPLATE template0 LOCALE_PROVIDER icu ICU_LOCALE 'und-u-kn-true' LOCALE 'C'`)
		const c = new pg.Client({connectionString: dbUrl()})
		await c.connect()
		await c.query(`CREATE TYPE "votingMode" AS ENUM('Slow', 'Fast')`)
		await c.query(`CREATE TABLE proposal_versions (
			proposal_id uuid NOT NULL, proposal_version integer NOT NULL, voting_mode "votingMode" NOT NULL,
			start_time bigint NOT NULL, end_time bigint NOT NULL, quorum bigint NOT NULL, threshold bigint NOT NULL,
			partial_percentage_support_threshold bigint NOT NULL, universal_percentage_support_threshold bigint NOT NULL,
			flat_support_threshold bigint NOT NULL, execute_by bigint, name text,
			yes_count bigint DEFAULT 0 NOT NULL, no_count bigint DEFAULT 0 NOT NULL, abstain_count bigint DEFAULT 0 NOT NULL,
			version_created_at text NOT NULL, version_created_at_block text NOT NULL,
			PRIMARY KEY (proposal_id, proposal_version), UNIQUE (proposal_id, version_created_at_block))`)
		for (let p = 0; p < 10; p++) {
			const proposal = randomUUID()
			for (const version of [1, 2, 3, 4]) {
				await c.query(
					`INSERT INTO proposal_versions (proposal_id, proposal_version, voting_mode, start_time, end_time, quorum, threshold,
					   partial_percentage_support_threshold, universal_percentage_support_threshold, flat_support_threshold,
					   execute_by, name, version_created_at, version_created_at_block)
					 VALUES ($1, $2, $3, 100, 200, 7, 5, 500000, 750000, 5, $4, $5, '1700000000', $6)`,
					[proposal, version, version % 2 ? "Slow" : "Fast", version === 3 ? 300 : null, version === 2 ? null : "Add Member", String(1000 * p + version)],
				)
				inserted.push(`${proposal}|${version}`)
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

	it("pages every row exactly once, at small page sizes", async () => {
		for (const pageSize of [1, 5, 13, 1000]) {
			const seen: string[] = []
			for await (const r of readProposalVersionRows(ro, pageSize)) seen.push(`${r.proposal_id}|${r.proposal_version}`)
			expect(seen.length).toBe(inserted.length)
			expect(new Set(seen)).toEqual(new Set(inserted))
		}
	})

	it("carries the columns as the backfill expects, and every row converts", async () => {
		const rows = []
		for await (const r of readProposalVersionRows(ro, 50)) rows.push(r)
		const v3 = rows.find((x) => x.proposal_version === 3)
		expect(v3).toMatchObject({voting_mode: "Slow", execute_by: "300", name: "Add Member", quorum: "7", start_time: "100", end_time: "200", version_created_at: "1700000000"})
		expect(v3?.block).toMatch(/^\d*3$/)
		expect(rows.find((x) => x.proposal_version === 2)).toMatchObject({voting_mode: "Fast", execute_by: null, name: null})
		for (const x of rows) expect(() => proposalVersionFromRow(x)).not.toThrow()
	})
})
