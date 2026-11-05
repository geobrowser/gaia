/**
 * readUserVoteRows against a real Postgres (set GEODB_FEEDER_TEST_PG_URL to a superuser URL; CI runs a postgres:17
 * service). The keyset WHERE compares the uuid/smallint primary key, so the ORDER BY must sort by the same columns:
 * if it sorted by the `::text` output aliases instead, any collation where text order differs from uuid byte order
 * would make paging skip or repeat rows. The database here uses ICU numeric collation (`und-u-kn-true`), under which
 * "9f..." sorts before "10...", so text order and uuid order disagree on most random ids.
 */
import {randomUUID} from "node:crypto"
import pg from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {ReadOnlyPg, readUserVoteRows} from "../pgSource.js"
import type {UserVoteRow} from "../votesKeyed.js"

const url = process.env.GEODB_FEEDER_TEST_PG_URL

describe.skipIf(!url)("readUserVoteRows on a real Postgres", () => {
	const dbName = `votes_order_${randomUUID().replaceAll("-", "").slice(0, 12)}`
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
		await c.query(`CREATE TABLE user_votes (
			user_id uuid NOT NULL, object_id uuid NOT NULL, object_type smallint NOT NULL, space_id uuid NOT NULL,
			vote_kind smallint NOT NULL, vote_type smallint NOT NULL, voted_at timestamptz NOT NULL,
			PRIMARY KEY (user_id, object_id, object_type, space_id, vote_kind))`)
		// A few users with many votes each, so pages break inside one user_id and the later key columns matter.
		const users = Array.from({length: 12}, () => randomUUID())
		for (const u of users) {
			for (let i = 0; i < 9; i++) {
				const o = randomUUID()
				const s = randomUUID()
				for (const kind of [-1, 0, 3]) {
					await c.query(
						"INSERT INTO user_votes VALUES ($1, $2, $3, $4, $5, $6, to_timestamp($7))",
						[u, o, i % 2, s, kind, i % 3, 1_700_000_000 + i],
					)
					inserted.push(`${u}|${o}|${i % 2}|${s}|${kind}`)
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
			`SELECT array_agg(user_id::text ORDER BY user_id::text) = array_agg(user_id::text ORDER BY user_id) AS same
			 FROM (SELECT DISTINCT user_id FROM user_votes) u`,
		)
		expect(r?.same).toBe(false)
	})

	it("pages every row exactly once, in primary-key order, at small page sizes", async () => {
		for (const pageSize of [1, 7, 26, 1000]) {
			const seen: string[] = []
			for await (const r of readUserVoteRows(ro, pageSize)) {
				seen.push(`${r.user_id}|${r.object_id}|${r.object_type}|${r.space_id}|${r.vote_kind}`)
			}
			expect(seen.length).toBe(inserted.length)
			expect(new Set(seen).size).toBe(inserted.length)
			expect(new Set(seen)).toEqual(new Set(inserted))
		}
	})

	it("reads each column as the backfill expects", async () => {
		const rows: UserVoteRow[] = []
		for await (const r of readUserVoteRows(ro, 50)) rows.push(r)
		const r = rows.find((x) => x.vote_kind === -1) as UserVoteRow
		expect(r.user_id).toMatch(/^[0-9a-f-]{36}$/)
		expect(typeof r.object_type).toBe("number")
		expect(r.vote_kind).toBe(-1)
		expect(r.voted_at_seconds).toMatch(/^17000000\d\d$/)
	})
})
