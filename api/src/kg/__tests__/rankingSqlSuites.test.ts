import {execFile} from "node:child_process"
import {readdirSync, readFileSync} from "node:fs"
import path from "node:path"
import {promisify} from "node:util"

import {Client} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"

/**
 * Runs the SQL-level migration suites in `api/drizzle/tests` in CI.
 *
 * Until #950 nothing ran them, which `drizzle/tests/README.md` had already named as the
 * cause of two staleness cases; turning them on immediately exposed a third (0078 had
 * been failing for days). This is the follow-up that file asked for: they now run against
 * the REAL schema, built by the real migrator, instead of a hand-maintained stand-in.
 *
 * WHY THAT MATTERED. The suites used to run against `0073_fixtures_schema.sql`, which
 * recreated a few columns of four tables and carried the comment "verified against the
 * live schema". It had drifted, and in the direction that hides bugs: it declared every
 * `entities` column except `id` nullable, where production has `created_at_block`,
 * `updated_at` and `updated_at_block` NOT NULL. Every suite inserted rows the real
 * database would have rejected. Nothing could have caught that except doing this.
 *
 * ISOLATION IS A ROLLBACK, NOT A TRUNCATE. Each suite runs inside a transaction that is
 * rolled back, so a suite cannot leave anything behind for the next one, and forward and
 * reverse order share one database. The suites keep their own TRUNCATE/DELETE cleanup for
 * when someone runs them by hand through psql, where there is no such wrapper.
 *
 * The scratch database is still per-run and dropped afterwards: the suites TRUNCATE
 * ranking tables and mutate `entity_ranking_config`, a single shared row, so they can
 * never share the integration database — vitest parallelises across files.
 */

const execFileAsync = promisify(execFile)

const API_ROOT = path.resolve(__dirname, "../../..")
const SUITES_DIR = path.join(API_ROOT, "drizzle", "tests")

function suiteNames(): string[] {
	return readdirSync(SUITES_DIR)
		.filter((f) => f.endsWith(".sql"))
		.sort()
}

/**
 * psql meta-commands (`\set`, `\echo`) are not SQL and node-postgres cannot parse them.
 * Blanked rather than dropped so any error message still points at the right line.
 */
function stripPsqlMeta(sql: string): string {
	return sql
		.split("\n")
		.map((line) => (/^\s*\\/.test(line) ? "" : line))
		.join("\n")
}

function adminUrl(): string {
	const url = process.env.DATABASE_URL
	if (!url) throw new Error("DATABASE_URL is required — these suites need a real Postgres")
	return url
}

function urlForDatabase(name: string): string {
	const url = new URL(adminUrl())
	url.pathname = `/${name}`
	return url.toString()
}

async function withAdmin(fn: (c: Client) => Promise<void>): Promise<void> {
	const admin = new Client({connectionString: adminUrl()})
	await admin.connect()
	try {
		await fn(admin)
	} finally {
		await admin.end()
	}
}

const DB_NAME = `ranking_sql_suites_${process.pid}`

let client: Client
const notices: string[] = []

beforeAll(async () => {
	await withAdmin(async (admin) => {
		await admin.query(`DROP DATABASE IF EXISTS "${DB_NAME}" WITH (FORCE)`)
		await admin.query(`CREATE DATABASE "${DB_NAME}"`)
	})

	// The real migrator, not a reimplementation of it. Six migrations use CREATE INDEX
	// CONCURRENTLY, which cannot run inside a transaction, so applying the files by hand
	// would mean reproducing drizzle's statement-breakpoint semantics — and a stand-in for
	// the migrations is the very thing this change exists to delete.
	//
	// DATABASE_URL_DIRECT because drizzle.config.ts prefers it over DATABASE_URL.
	await execFileAsync("bun", ["drizzle-kit", "migrate"], {
		cwd: API_ROOT,
		env: {...process.env, DATABASE_URL_DIRECT: urlForDatabase(DB_NAME)},
	})

	client = new Client({connectionString: urlForDatabase(DB_NAME)})
	client.on("notice", (n) => {
		if (n.message) notices.push(n.message)
	})
	await client.connect()
}, 300_000)

afterAll(async () => {
	await client?.end()
	await withAdmin(async (admin) => {
		await admin.query(`DROP DATABASE IF EXISTS "${DB_NAME}" WITH (FORCE)`)
	})
}, 60_000)

async function runSuite(suite: string): Promise<void> {
	const sql = stripPsqlMeta(readFileSync(path.join(SUITES_DIR, suite), "utf8"))
	const before = notices.length

	await client.query("BEGIN")
	try {
		// One query for the whole file: node-postgres throws on the first error, and every
		// suite raises `FAIL: <label>` from its own assert(), so the thrown message names
		// the assertion that broke.
		await client.query(sql)
	} finally {
		await client.query("ROLLBACK")
	}

	// A suite that ran clean but asserted NOTHING is the failure mode
	// `passing tests that never ran` describes — 8 green tests that were 8 skips. Only the
	// floor is checked: assert() is also called once in each file's own function
	// definition, so an exact count against `assert(` occurrences is brittle, not stricter.
	const passes = notices.slice(before).filter((m) => m.startsWith("pass:"))
	expect(passes.length, `${suite} asserted nothing`).toBeGreaterThan(0)
}

// Forward is how the migrations are applied. Reverse is the property the README claims and
// which a missing TRUNCATE once broke. They share a database because the rollback, not the
// suites' own cleanup, is what isolates them.
describe("ranking SQL suites (forward)", () => {
	for (const suite of suiteNames()) {
		it(suite, () => runSuite(suite), 120_000)
	}
})

describe("ranking SQL suites (reverse)", () => {
	for (const suite of [...suiteNames()].reverse()) {
		it(suite, () => runSuite(suite), 120_000)
	}
})
