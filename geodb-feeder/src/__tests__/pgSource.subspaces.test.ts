/**
 * readSubspaceRows against a real Postgres (GEODB_FEEDER_TEST_PG_URL, as pgSource.votes.test.ts). The enum cast
 * (`type::text`) and the UNION ALL of the two tables are what a mocked pool cannot check. Tables are gaia's
 * (api/drizzle 0002, 0045), without their foreign keys.
 */
import {randomUUID} from "node:crypto"
import pg from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {ReadOnlyPg, readSubspaceRows} from "../pgSource.js"
import {subspaceFromRow} from "../subspacesKeyed.js"

const url = process.env.GEODB_FEEDER_TEST_PG_URL

describe.skipIf(!url)("readSubspaceRows on a real Postgres", () => {
	const dbName = `subspaces_${randomUUID().replaceAll("-", "").slice(0, 12)}`
	const dbUrl = () => {
		const u = new URL(url as string)
		u.pathname = `/${dbName}`
		return u.toString()
	}
	const [a, b, c, t] = [randomUUID(), randomUUID(), randomUUID(), randomUUID()]
	let admin: pg.Client
	let ro: ReadOnlyPg

	beforeAll(async () => {
		admin = new pg.Client({connectionString: url})
		await admin.connect()
		await admin.query(`CREATE DATABASE ${dbName}`)
		const cl = new pg.Client({connectionString: dbUrl()})
		await cl.connect()
		await cl.query(`CREATE TYPE "subspaceType" AS ENUM('verified', 'related')`)
		await cl.query(`CREATE TABLE subspaces (parent_space_id uuid NOT NULL, child_space_id uuid NOT NULL,
			type "subspaceType" NOT NULL DEFAULT 'verified', PRIMARY KEY (parent_space_id, child_space_id, type))`)
		await cl.query(`CREATE TABLE subspace_topics (space_id uuid NOT NULL, topic_id uuid NOT NULL, PRIMARY KEY (space_id, topic_id))`)
		await cl.query(`INSERT INTO subspaces VALUES ($1,$2,'verified'), ($1,$2,'related'), ($2,$3,'verified')`, [a, b, c])
		await cl.query(`INSERT INTO subspace_topics VALUES ($1,$2)`, [a, t])
		await cl.end()
		ro = new ReadOnlyPg({connectionString: dbUrl()})
	})

	afterAll(async () => {
		await ro?.close()
		await admin?.query(`DROP DATABASE IF EXISTS ${dbName} WITH (FORCE)`)
		await admin?.end()
	})

	it("returns every subspace and topic row, typed, and each maps to an event", async () => {
		const rows = await readSubspaceRows(ro)
		expect(rows.map((r) => `${r.parent_space_id}|${r.target_id}|${r.type}`).sort()).toEqual(
			[`${a}|${b}|verified`, `${a}|${b}|related`, `${b}|${c}|verified`, `${a}|${t}|topic`].sort(),
		)
		expect(rows.map((r) => subspaceFromRow(r).type).sort()).toEqual(["related", "topic", "verified", "verified"])
	})
})
