import {readdirSync, readFileSync} from "node:fs"
import path from "node:path"

import {Client} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"

/**
 * Runs the SQL-level migration suites in `api/drizzle/tests` in CI.
 *
 * Until now nothing ran them. `drizzle/tests/README.md` carried the TODO and named the
 * consequence outright: "nothing here runs automatically, which is how both staleness
 * cases above survived." A third case had accumulated unnoticed by the time this was
 * written — 0083 changed participation to count curation votes, which invalidated a
 * fixture in 0078 whose entity carried a curation downvote, so 0078 had been failing
 * against the current schema for days. That is the whole argument for this file.
 *
 * WHY A SCRATCH DATABASE, and not the integration database these tests run beside:
 *
 *  - The suites TRUNCATE the ranking tables and mutate `entity_ranking_config`, which is
 *    a single shared row. Run against the shared test database they would corrupt every
 *    other integration test, and vitest runs files in parallel workers, so the damage
 *    would be nondeterministic.
 *  - They are written against `0073_fixtures_schema.sql`, which recreates only the
 *    columns of `entities`, `values`, `relations` and `votes_count` that the scoring
 *    functions read. Against the REAL schema their TRUNCATEs fail outright:
 *    `spaces` and `subspace_topics` gained foreign keys to `entities`, and the closure
 *    pulls in `proposals`, `proposal_votes`, `subspaces` and `space_voting_settings`.
 *
 * The second point is a real limitation and worth stating plainly: the fixture schema is
 * hand-maintained, so it can drift from the migrations it stands in for, and these suites
 * cannot catch that. Converting all eight files to id-scoped cleanup (as 0089 and 0090
 * already do) is what would let them run against the real schema. Until then this at
 * least runs them.
 */

const DRIZZLE_DIR = path.resolve(__dirname, "../../../drizzle")
const SUITES_DIR = path.join(DRIZZLE_DIR, "tests")
const FIXTURES_SCHEMA = "0073_fixtures_schema.sql"

/**
 * Migrations the fixture schema supports, applied in this order before the suites run.
 *
 * Derived from the suite filenames, plus the ones below that have no suite of their own
 * but are still required — 0082 creates the typed feed function that 0085 replaces.
 * Applying every migration instead is not an option: the fixture schema has no
 * `proposals` table, so 0086 and 0087 would fail.
 */
const EXTRA_MIGRATIONS = ["0082_feed_typed_variant"]

function suiteNames(): string[] {
	return readdirSync(SUITES_DIR)
		.filter((f) => f.endsWith(".sql") && f !== FIXTURES_SCHEMA)
		.sort()
}

function migrationFor(suite: string): string {
	const stem = suite.replace(/\.sql$/, "")
	const file = readdirSync(DRIZZLE_DIR).find((f) => f === `${stem}.sql`)
	if (!file) throw new Error(`no migration ${stem}.sql for suite ${suite}`)
	return stem
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

async function createScratchDatabase(name: string): Promise<string> {
	const admin = new Client({connectionString: adminUrl()})
	await admin.connect()
	try {
		await admin.query(`DROP DATABASE IF EXISTS "${name}"`)
		await admin.query(`CREATE DATABASE "${name}"`)
	} finally {
		await admin.end()
	}
	const url = new URL(adminUrl())
	url.pathname = `/${name}`
	return url.toString()
}

async function dropScratchDatabase(name: string): Promise<void> {
	const admin = new Client({connectionString: adminUrl()})
	await admin.connect()
	try {
		await admin.query(`DROP DATABASE IF EXISTS "${name}" WITH (FORCE)`)
	} finally {
		await admin.end()
	}
}

async function applySchemaAndMigrations(client: Client): Promise<void> {
	const fixtures = readFileSync(path.join(SUITES_DIR, FIXTURES_SCHEMA), "utf8")
	await client.query(stripPsqlMeta(fixtures))

	const ordered = [...new Set([...suiteNames().map(migrationFor), ...EXTRA_MIGRATIONS])].sort()
	for (const stem of ordered) {
		const sql = readFileSync(path.join(DRIZZLE_DIR, `${stem}.sql`), "utf8")
		// drizzle's statement separator is a comment to Postgres, but blank it anyway so a
		// failure message never points at a marker line.
		await client.query(stripPsqlMeta(sql.replace(/^--> statement-breakpoint$/gm, "")))
	}
}

/**
 * The suites are stateful and share one config row, so they run in one describe against
 * one scratch database, in order, exactly as they are run by hand.
 */
function runSuitesInOrder(label: string, order: (names: string[]) => string[]) {
	describe(`ranking SQL suites (${label})`, () => {
		const dbName = `ranking_sql_${label.replace(/\W/g, "_")}_${process.pid}`
		let client: Client
		const notices: string[] = []

		beforeAll(async () => {
			const url = await createScratchDatabase(dbName)
			client = new Client({connectionString: url})
			client.on("notice", (n) => {
				if (n.message) notices.push(n.message)
			})
			await client.connect()
			await applySchemaAndMigrations(client)
		}, 120_000)

		afterAll(async () => {
			await client?.end()
			await dropScratchDatabase(dbName)
		}, 60_000)

		for (const suite of order(suiteNames())) {
			it(suite, async () => {
				const sql = stripPsqlMeta(readFileSync(path.join(SUITES_DIR, suite), "utf8"))
				const before = notices.length
				// One query for the whole file: node-postgres throws on the first error, and
				// every suite raises `FAIL: <label>` from its own assert(), so the thrown
				// message names the assertion that broke.
				await client.query(sql)

				// A suite that ran clean but asserted NOTHING is the failure mode
				// `passing tests that never ran` describes — 8 green tests that were 8 skips.
				// Only the floor is checked: assert() is also called once in each file's own
				// function definition, and a couple of files mention it in prose, so an exact
				// count against `assert(` occurrences is brittle rather than stricter.
				const passes = notices.slice(before).filter((m) => m.startsWith("pass:"))
				expect(passes.length, `${suite} asserted nothing`).toBeGreaterThan(0)
			}, 120_000)
		}
	})
}

// Forward is how they are applied. Reverse is the property the README claims and which a
// missing TRUNCATE once broke: "they truncate their own fixtures, so they pass in any
// order — verified by running the set forwards and backwards."
runSuitesInOrder("forward", (n) => n)
runSuitesInOrder("reverse", (n) => [...n].reverse())

describe("ranking SQL suite coverage", () => {
	it("every suite has a migration of the same name", () => {
		for (const suite of suiteNames()) expect(() => migrationFor(suite)).not.toThrow()
	})
})
