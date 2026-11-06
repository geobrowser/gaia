/**
 * The proposal-actions gate (RUNBOOK 2c-quinquies): diff Postgres `proposal_actions` against the keyed records in the
 * governance system Space. Read only; two sets of files, as votesGate.ts. Output and pass rule are votesGate.ts's
 * (compare): a pg-only key or a value difference is unexplained; geo-db-only is expected (proposalActionsKeyed.ts).
 *
 *   - Postgres rows from `\copy (...) to stdout`, one TSV, `\N` for null, the same columns and order as
 *     PG_COLUMNS below; `block` is `proposal_versions.version_created_at_block` joined on (proposal_id,
 *     proposal_version), and metadata / content_id are `encode(..., 'hex')`;
 *   - one TSV per record column from geo-db GQL (`MATCH (r)-[:`<column>`]->(x) RETURN r, x`), named `pa-<name>.tsv`
 *     for the names in COLUMNS, each line `record<TAB>value<TAB>count`. A column the action does not have has no line.
 */
import {readFileSync} from "node:fs"
import {cid} from "./ids.js"
import {proposalActionRecordId} from "./proposalActionsKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

/** proposal, index, block, action_type, target, content_uri, metadata, content_id, quorum, duration, partial, universal, flat, disable_fast_path, grace. */
export const PG_COLUMNS = [
	"proposal", "index", "block", "action_type", "target", "content_uri", "metadata", "content_id", "quorum", "duration",
	"partial", "universal", "flat", "disable_fast_path", "grace",
] as const

const MUTABLE = ["action_type", "target", "content_uri", "metadata", "content_id", "quorum", "duration", "partial", "universal", "flat", "disable_fast_path", "grace"]
const ABSENT = "\\N"

export function gateProposalActions(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const f = line.split("\t")
		const row = Object.fromEntries(PG_COLUMNS.map((c, i) => [c, f[i] as string])) as Record<string, string>
		const id = proposalActionRecordId({proposal: row.proposal as string, block: BigInt.asUintN(64, BigInt(row.block as string)), index: Number(row.index)})
		pg.set(id, {
			proposal: cid("entity", row.proposal as string),
			index: row.index as string,
			block: row.block as string,
			...Object.fromEntries(MUTABLE.map((c) => [c, c === "target" && row[c] !== ABSENT ? cid("entity", row[c] as string) : row[c]])),
		})
	}
	const names = ["proposal", "index", "block", ...MUTABLE]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/pa-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.proposal!.keys()) {
		const g: Record<string, string> = {}
		for (const n of names) g[n] = cols[n]!.get(id)?.value ?? (MUTABLE.includes(n) ? ABSENT : "<missing>")
		geo.set(id, g)
	}
	return compare("proposal_actions", pg, geo, ["proposal", "index", "block"], MUTABLE, cols, ["proposal"])
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun proposalActionsGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateProposalActions(pgTsv, dir)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(JSON.stringify({...r, geoOnly: head(r.geoOnly), pgOnly: head(r.pgOnly), valueDiffs: head(r.valueDiffs), counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length, valueDiffs: r.valueDiffs.length, refCountViolations: r.refCountViolations.length}}, null, 2))
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 ? 0 : 1)
}
