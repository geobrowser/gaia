import {readdirSync, readFileSync, statSync} from "node:fs"
import {join, relative} from "node:path"
import {describe, expect, it} from "vitest"

/**
 * GEO-3703. In Postgres an unqualified ORDER BY name binds to an OUTPUT column before an input one, so
 *
 *   SELECT entity_id::text FROM t WHERE entity_id >= $1 ORDER BY entity_id LIMIT 20
 *
 * sorts by the text cast. The primary key can no longer supply the order, and the planner sorts every
 * matching row: the background shadows' keyset sweeps and score threshold read 55M-row tables per pass
 * (over 2 minutes each, a dozen at a time across the api pods) and starved the explore feed. Qualifying
 * the column (`ORDER BY t.entity_id`) binds it to the table column and restores the index plan.
 *
 * This guard flags any SQL string in api/src that selects `<col>::text` without an alias, or aliased
 * back to its own name, and then orders by the bare `<col>`.
 */

const SRC = join(__dirname, "..", "..")

function sourceFiles(dir: string): string[] {
	return readdirSync(dir).flatMap((name) => {
		const path = join(dir, name)
		if (statSync(path).isDirectory())
			return name === "__tests__" || name === "node_modules" ? [] : sourceFiles(path)
		return path.endsWith(".ts") ? [path] : []
	})
}

/** The `SELECT ... FROM` lists and ORDER BY lists of each statement-ish chunk of a line. */
function offenders(line: string): string[] {
	const found: string[] = []
	for (const m of line.matchAll(
		/SELECT\s+(.+?)\s+FROM\b(.*?)\bORDER BY\s+([^)`]+?)(?:\s+(?:LIMIT|OFFSET)\b|\)|`|$)/gi,
	)) {
		const selectList = m[1] ?? ""
		const orderList = (m[3] ?? "").split(",").map((t) => t.trim().split(/\s+/)[0] ?? "")
		const textOutputs = new Set<string>()
		for (const col of selectList.split(",")) {
			const c = col.trim().match(/^(?:[a-z_]+\.)?([a-z_]+)::text(?:\s+AS\s+([a-z_]+))?$/i)
			if (!c) continue
			const out = c[2] ?? c[1]
			if (out === c[1]) textOutputs.add(out as string)
		}
		for (const o of orderList) if (textOutputs.has(o)) found.push(o)
	}
	return found
}

describe("text-cast ORDER BY guard (GEO-3703)", () => {
	it("flags the pattern it exists to catch", () => {
		expect(
			offenders("SELECT entity_id::text FROM t WHERE entity_id >= $1::uuid ORDER BY entity_id LIMIT $2"),
		).toEqual(["entity_id"])
		expect(
			offenders(
				"(SELECT ranking_score::text FROM t ORDER BY ranking_score DESC, entity_id DESC OFFSET 199 LIMIT 1)",
			),
		).toEqual(["ranking_score"])
		expect(offenders("SELECT s.entity_id::text AS entity_id FROM t s ORDER BY s.entity_id LIMIT 1")).toEqual([])
		expect(offenders("SELECT user_id::text AS id FROM t ORDER BY user_id LIMIT 1")).toEqual([])
	})

	it("finds no unqualified ORDER BY on a ::text output column in api/src", () => {
		const hits = sourceFiles(SRC).flatMap((file) =>
			readFileSync(file, "utf8")
				.split("\n")
				.flatMap((line, i) => offenders(line).map((col) => `${relative(SRC, file)}:${i + 1} ORDER BY ${col}`)),
		)
		expect(hits).toEqual([])
	})
})
