/**
 * The proposal-status gate (RUNBOOK 2c-novies): diff the status facts of Postgres `proposals` (executed_at,
 * unexecutable_at) against the keyed records in the governance system Space. Read only; two sets of files, as
 * votesGate.ts. Output and pass rule are votesGate.ts's (compare): a pg-only key or a value difference is unexplained;
 * geo-db-only is expected (proposalStatusKeyed.ts). It checks the stored facts only: the computed status is not stored
 * on either side and is not gated here.
 *
 *   - Postgres rows from `\copy (...) to stdout`, one TSV, `\N` for null, the columns and order of PG_COLUMNS below,
 *     `id` dashless (`replace(id::text, '-', '')`), only rows with executed_at or unexecutable_at not null;
 *   - one TSV per record column from geo-db GQL (`MATCH (r)-[:`<column>`]->(x) RETURN r, x`), named `ps-<name>.tsv`
 *     for the names in COLUMNS, each line `record<TAB>value<TAB>count`. A fact that is null in Postgres has no line.
 */
import {readFileSync} from "node:fs"
import {cid} from "./ids.js"
import {proposalStatusRecordId} from "./proposalStatusKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

/** proposal, executed_at, unexecutable_at (whole seconds, or \N). */
export const PG_COLUMNS = ["proposal", "executed_at", "unexecutable_at"] as const

const MUTABLE = ["executed_at", "unexecutable_at"] as const
const ABSENT = "<missing>"

const instantSeconds = (v: string) => /^instant:(\d+)\.\d+$/.exec(v)?.[1] ?? v

export function gateProposalStatus(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const f = line.split("\t")
		const row = Object.fromEntries(PG_COLUMNS.map((c, i) => [c, f[i] as string])) as Record<string, string>
		pg.set(proposalStatusRecordId(row.proposal as string), {
			proposal: cid("entity", row.proposal as string),
			...Object.fromEntries(MUTABLE.map((c) => [c, row[c] === "\\N" ? ABSENT : (row[c] as string)])),
		})
	}
	const names = ["proposal", ...MUTABLE]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/ps-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.proposal!.keys()) {
		const g: Record<string, string> = {}
		for (const n of names) {
			const v = cols[n]!.get(id)?.value ?? ABSENT
			g[n] = n === "proposal" ? v : instantSeconds(v)
		}
		geo.set(id, g)
	}
	return compare("proposals (status facts)", pg, geo, ["proposal"], [...MUTABLE], cols, ["proposal"])
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun proposalStatusGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateProposalStatus(pgTsv, dir)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(JSON.stringify({...r, geoOnly: head(r.geoOnly), pgOnly: head(r.pgOnly), valueDiffs: head(r.valueDiffs), counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length, valueDiffs: r.valueDiffs.length, refCountViolations: r.refCountViolations.length}}, null, 2))
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 ? 0 : 1)
}
