/**
 * readProposalActionRows against a real Postgres (GEODB_FEEDER_TEST_PG_URL, as pgSource.proposalVotes.test.ts): the
 * keyset paging over (proposal_id, proposal_version, index) must page every row exactly once, and the join to
 * `proposal_versions` must carry each row's version block. Tables as gaia's (api/drizzle 0067 and kg-indexer
 * storage.rs), without foreign keys and reduced to the columns the reader selects.
 */
import {randomUUID} from "node:crypto"
import pg from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {ReadOnlyPg, readProposalActionRows} from "../pgSource.js"
import {proposalActionFromRow} from "../proposalActionsKeyed.js"

const url = process.env.GEODB_FEEDER_TEST_PG_URL

describe.skipIf(!url)("readProposalActionRows on a real Postgres", () => {
	const dbName = `pactions_order_${randomUUID().replaceAll("-", "").slice(0, 12)}`
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
		await c.query(`CREATE TYPE "proposalActionType" AS ENUM('AddMember', 'Publish', 'Flag', 'UpdateVotingSettings', 'Unknown')`)
		await c.query(`CREATE TABLE proposal_versions (proposal_id uuid NOT NULL, proposal_version integer NOT NULL,
			version_created_at_block text NOT NULL, PRIMARY KEY (proposal_id, proposal_version))`)
		await c.query(`CREATE TABLE proposal_actions (
			proposal_id uuid NOT NULL, proposal_version integer NOT NULL, "index" integer NOT NULL,
			action_type "proposalActionType" NOT NULL, target_id uuid, content_uri text, metadata bytea, content_id bytea,
			quorum bigint, duration bigint, partial_percentage_support_threshold bigint, universal_percentage_support_threshold bigint,
			flat_support_threshold bigint, disable_fast_path_access_for_new_members boolean, execution_grace_period bigint,
			PRIMARY KEY (proposal_id, proposal_version, "index"))`)
		for (let p = 0; p < 10; p++) {
			const proposal = randomUUID()
			for (const version of [1, 2, 3]) {
				await c.query("INSERT INTO proposal_versions VALUES ($1, $2, $3)", [proposal, version, String(1000 * p + version)])
				for (let i = 0; i < 4; i++) {
					await c.query(
						`INSERT INTO proposal_actions (proposal_id, proposal_version, "index", action_type, content_uri, metadata, quorum)
						 VALUES ($1, $2, $3, $4, $5, $6, $7)`,
						i === 1 ? [proposal, version, i, "Publish", "ipfs://x", Buffer.from([1, 255]), null] : [proposal, version, i, "Unknown", null, null, i === 3 ? 7 : null],
					)
					inserted.push(`${proposal}|${version}|${i}`)
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

	it("pages every row exactly once, at small page sizes", async () => {
		for (const pageSize of [1, 5, 13, 1000]) {
			const seen: string[] = []
			for await (const r of readProposalActionRows(ro, pageSize)) seen.push(`${r.proposal_id}|${r.proposal_version}|${r.index}`)
			expect(seen.length).toBe(inserted.length)
			expect(new Set(seen)).toEqual(new Set(inserted))
		}
	})

	it("carries the version block and the columns as the backfill expects, and every row converts", async () => {
		const rows = []
		for await (const r of readProposalActionRows(ro, 50)) rows.push(r)
		const pub = rows.find((x) => x.action_type === "Publish")
		expect(pub?.metadata).toBe("01ff")
		expect(pub?.content_uri).toBe("ipfs://x")
		expect(rows.find((x) => x.proposal_version === 2)?.block).toMatch(/^\d*002$/)
		expect(rows.find((x) => x.index === 3)?.quorum).toBe("7")
		for (const x of rows) expect(() => proposalActionFromRow(x)).not.toThrow()
	})
})
