/**
 * The space-topics gate (RUNBOOK 2c-undecies): diff Postgres `spaces.topic_id` against the keyed records in the
 * governance system Space. Read only; two sets of files, as votesGate.ts. Output and pass rule are votesGate.ts's
 * (compare): a pg-only key or a value difference is unexplained; geo-db-only is expected (a Space whose topic was removed
 * keeps its record with an empty `topic` cell, and a topic event for a Space Postgres lacks is written anyway;
 * spaceTopicsKeyed.ts).
 *
 *   - Postgres rows from `\copy (select replace(id::text,'-',''), replace(topic_id::text,'-','') from spaces where
 *     topic_id is not null) to stdout`, one TSV, columns `space`, `topic` (PG_COLUMNS), both dashless;
 *   - one TSV per record column from geo-db GQL (`MATCH (r)-[:`<column>`]->(x) RETURN r, x`), named `st-<name>.tsv`
 *     for the names in COLUMNS (`space`, `topic`), each line `record<TAB>value<TAB>count`.
 */
import {readFileSync} from "node:fs"
import {cid} from "./ids.js"
import {spaceTopicRecordId} from "./spaceTopicsKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

export const PG_COLUMNS = ["space", "topic"] as const

export function gateSpaceTopics(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const [space, topic] = line.split("\t") as [string, string]
		pg.set(spaceTopicRecordId(space), {space: cid("entity", space), topic: cid("entity", topic)})
	}
	const names = ["space", "topic"]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/st-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.space!.keys()) geo.set(id, {space: cols.space!.get(id)?.value ?? "<missing>", topic: cols.topic!.get(id)?.value ?? "<missing>"})
	return compare("space_topics", pg, geo, ["space"], ["topic"], cols, ["space", "topic"])
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun spaceTopicsGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateSpaceTopics(pgTsv, dir)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(JSON.stringify({...r, geoOnly: head(r.geoOnly), pgOnly: head(r.pgOnly), valueDiffs: head(r.valueDiffs), counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length, valueDiffs: r.valueDiffs.length, refCountViolations: r.refCountViolations.length}}, null, 2))
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 ? 0 : 1)
}
