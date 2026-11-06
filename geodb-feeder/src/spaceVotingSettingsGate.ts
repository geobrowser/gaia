/**
 * The space-voting-settings gate (RUNBOOK 2c-septies): diff Postgres `space_voting_settings` against the keyed records in
 * the governance system Space. Read only; two sets of files, as votesGate.ts. Output and pass rule are votesGate.ts's
 * (compare): a pg-only key or a value difference is unexplained; geo-db-only is expected (spaceVotingSettingsKeyed.ts).
 *
 *   - Postgres rows from `\copy (...) to stdout`, one TSV, `\N` for null, the columns and order of PG_COLUMNS below,
 *     `space_id` dashless (`replace(space_id::text, '-', '')`);
 *   - one TSV per record column from geo-db GQL (`MATCH (r)-[:`<column>`]->(x) RETURN r, x`), named `svs-<name>.tsv`
 *     for the names in COLUMNS, each line `record<TAB>value<TAB>count`.
 */
import {readFileSync} from "node:fs"
import {cid} from "./ids.js"
import {spaceVotingSettingsRecordId} from "./spaceVotingSettingsKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

/** space, partial, universal, flat, quorum, duration, disable_fast_path (t/f), grace, updated_at, updated_at_block. */
export const PG_COLUMNS = ["space", "partial", "universal", "flat", "quorum", "duration", "disable_fast_path", "grace", "updated_at", "updated_at_block"] as const

const MUTABLE = PG_COLUMNS.filter((c) => c !== "space")

const instantSeconds = (v: string) => /^instant:(\d+)\.\d+$/.exec(v)?.[1] ?? v

export function gateSpaceVotingSettings(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const f = line.split("\t")
		const row = Object.fromEntries(PG_COLUMNS.map((c, i) => [c, f[i] as string])) as Record<string, string>
		pg.set(spaceVotingSettingsRecordId(row.space as string), {
			space: cid("entity", row.space as string),
			...Object.fromEntries(MUTABLE.map((c) => [c, c === "disable_fast_path" ? (row[c] === "t" || row[c] === "true" ? "true" : "false") : (row[c] as string)])),
		})
	}
	const names = ["space", ...MUTABLE]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/svs-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.space!.keys()) {
		const g: Record<string, string> = {}
		for (const n of names) {
			const v = cols[n]!.get(id)?.value ?? "<missing>"
			g[n] = n === "updated_at" ? instantSeconds(v) : v
		}
		geo.set(id, g)
	}
	return compare("space_voting_settings", pg, geo, ["space"], [...MUTABLE], cols, ["space"])
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun spaceVotingSettingsGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateSpaceVotingSettings(pgTsv, dir)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(JSON.stringify({...r, geoOnly: head(r.geoOnly), pgOnly: head(r.pgOnly), valueDiffs: head(r.valueDiffs), counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length, valueDiffs: r.valueDiffs.length, refCountViolations: r.refCountViolations.length}}, null, 2))
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 ? 0 : 1)
}
