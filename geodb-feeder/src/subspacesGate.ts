/**
 * The subspaces gate (RUNBOOK 2c-sexies): diff Postgres `subspaces` and `subspace_topics` against the ACTIVE keyed
 * edges in the governance system Space. Read only; two sets of files, as votesGate.ts, and its output and pass rule
 * (compare): a pg-only edge is unexplained; geo-db-only is expected (subspacesKeyed.ts).
 *
 *   - Postgres rows from `\copy (...) to stdout`, one TSV, three columns: parent (or Space) id, child (or topic) id,
 *     type (`verified`, `related` or `topic`), ids dashless:
 *       SELECT replace(parent_space_id::text,'-',''), replace(child_space_id::text,'-',''), type::text FROM subspaces
 *       UNION ALL SELECT replace(space_id::text,'-',''), replace(topic_id::text,'-',''), 'topic' FROM subspace_topics
 *   - one TSV per type from geo-db GQL, `subspaces-<type>.tsv`, each line `subject<TAB>target` for every ACTIVE edge of
 *     that type's property (`MATCH (s)-[:`<property>`]->(t) RETURN s, t`), subject and target as the entity ids
 *     (`cid("entity", id)` of the Space or topic).
 */
import {readFileSync} from "node:fs"
import {cid} from "./ids.js"
import {type Cols, compare, type GateResult, type Side} from "./votesGate.js"

const TYPES = ["verified", "related", "topic"] as const

export function gateSubspaces(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const [parent, target, type] = line.split("\t") as [string, string, string]
		if (!TYPES.includes(type as (typeof TYPES)[number])) throw new Error(`unknown subspace type ${type}`)
		const key = `${type}|${cid("entity", parent)}|${cid("entity", target)}`
		pg.set(key, {key})
	}
	const geo: Side = new Map()
	for (const type of TYPES) {
		for (const line of readFileSync(`${dir}/subspaces-${type}.tsv`, "utf8").split("\n")) {
			if (!line) continue
			const [subject, target] = line.split("\t") as [string, string]
			const key = `${type}|${subject}|${target}`
			geo.set(key, {key})
		}
	}
	return compare("subspaces", pg, geo, ["key"], [], {} as Cols, [])
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun subspacesGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateSubspaces(pgTsv, dir)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(JSON.stringify({...r, geoOnly: head(r.geoOnly), pgOnly: head(r.pgOnly), counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length}}, null, 2))
	process.exit(r.pgOnly.length === 0 ? 0 : 1)
}
