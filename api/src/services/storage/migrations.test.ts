import {readdirSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"
import {describe, expect, it} from "vitest"

/**
 * Guards the drizzle migration journal.
 *
 * These are cheap file checks with no database, so they live outside
 * `__tests__/` and run in the unit workflow, which is the point: the failure
 * they catch is invisible to anything that needs a database, because a fresh
 * database hides it completely.
 *
 * On 2026-09-15, migration 0086 shipped with a hand-written `when` four seconds
 * EARLIER than 0085's. Drizzle applies an entry only when its `folderMillis`
 * exceeds the newest applied row's `created_at`:
 *
 *   if (!lastDbMigration || Number(lastDbMigration[2]) < migration.folderMillis)
 *
 * A fresh database has no rows, so `lastDbMigration` is undefined and every
 * migration applies in order regardless — it passed locally and in CI. On
 * testnet, which already held 0085, it was skipped: no error, no row written,
 * exit code 0, and the api's `migrate` init container reported "Completed".
 * Nothing anywhere reported that a migration had not run. It was found only
 * because a plugin that assumed the migration had run then crash-looped every
 * api pod. See GEO-2916.
 */

// `import.meta.dir` is a Bun global and is undefined under vitest, which is what
// runs this in CI — resolve from import.meta.url instead.
const DRIZZLE_DIR = join(dirname(fileURLToPath(import.meta.url)), "../../../drizzle")
const JOURNAL = join(DRIZZLE_DIR, "meta/_journal.json")

type JournalEntry = {idx: number; version: string; when: number; tag: string; breakpoints?: boolean}

const journal = (): JournalEntry[] => (JSON.parse(readFileSync(JOURNAL, "utf8")) as {entries: JournalEntry[]}).entries

describe("drizzle migration journal", () => {
	it("has strictly increasing `when` values", () => {
		const entries = journal()
		expect(entries.length).toBeGreaterThan(0)

		// Report every offender at once — a rebase that interleaves two branches'
		// migrations can produce several, and fixing them one CI run at a time is
		// miserable.
		const offenders = entries
			.slice(1)
			.map((entry, i) => ({prev: entries[i] as JournalEntry, entry}))
			.filter(({prev, entry}) => entry.when <= prev.when)
			.map(({prev, entry}) => `${entry.tag} (when=${entry.when}) is not after ${prev.tag} (when=${prev.when})`)

		expect(
			offenders,
			"Drizzle SKIPS a migration whose `when` is not greater than the previously applied one, " +
				"silently and with exit code 0, on every database that already has the previous migration. " +
				"It still applies cleanly on a fresh database, so this cannot be caught by running the " +
				"migrations locally. Set `when` to Date.now() at the time you add the entry.",
		).toEqual([])
	})

	it("has `idx` values that match position and order", () => {
		const entries = journal()
		expect(entries.map((e) => e.idx)).toEqual(entries.map((_, i) => i))
	})

	it("has a .sql file for every journal entry", () => {
		const files = new Set(readdirSync(DRIZZLE_DIR).filter((f) => f.endsWith(".sql")))
		const missing = journal()
			.map((e) => `${e.tag}.sql`)
			.filter((f) => !files.has(f))
		expect(missing, "a journal entry with no .sql file fails at migrate time").toEqual([])
	})

	it("has a journal entry for every .sql file", () => {
		const tagged = new Set(journal().map((e) => `${e.tag}.sql`))
		const orphans = readdirSync(DRIZZLE_DIR)
			.filter((f) => f.endsWith(".sql"))
			.filter((f) => !tagged.has(f))
		expect(
			orphans,
			"drizzle applies migrations by journal order, so a .sql file with no entry is never run — " +
				"which looks exactly like a migration that applied and did nothing",
		).toEqual([])
	})
})
