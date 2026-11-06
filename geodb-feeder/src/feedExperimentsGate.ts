/**
 * The feed-experiments gate (RUNBOOK 2c-config-experiments): diff Postgres `personalization.feed_experiments` against the
 * keyed records in the config system Space. Read only; two sets of files, as interestConfigGate.ts.
 *
 *   - Postgres: `\copy (select id, active, arm_a, arm_b, share, salt, coalesce(note, '') from
 *     personalization.feed_experiments) to stdout`;
 *   - one TSV per record column from geo-db GQL, named `fe-<column>.tsv` for id, active, arm_a, arm_b, share, salt, note,
 *     each line `record<TAB>value<TAB>count` (a null note has no statement; it compares as the empty string).
 *
 * An experiment deleted from Postgres shows as geo-db-only (not a failure).
 */
import {readFileSync} from "node:fs"
import {feedExperimentRecordId} from "./feedExperimentsKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

const MUTABLE = ["active", "arm_a", "arm_b", "share", "salt", "note"]
export const PG_COLUMNS = ["id", ...MUTABLE] as const

const canon = (v: string) => (Number.isFinite(Number(v)) && v.trim() !== "" ? String(Number(v)) : v)

export function gateFeedExperiments(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const [id, active, armA, armB, share, salt, note] = line.split("\t") as [string, string, string, string, string, string, string?]
		pg.set(feedExperimentRecordId(id), {
			id,
			active: active === "t" || active === "true" ? "true" : "false",
			arm_a: armA,
			arm_b: armB,
			share: canon(share),
			salt,
			note: note ?? "",
		})
	}
	const names = ["id", ...MUTABLE]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/fe-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const rec of cols.id!.keys()) {
		const g: Record<string, string> = {}
		for (const n of names) {
			const v = cols[n]!.get(rec)?.value
			g[n] = n === "note" ? (v ?? "") : n === "share" ? canon(v ?? "<missing>") : (v ?? "<missing>")
		}
		geo.set(rec, g)
	}
	return compare("feed_experiments", pg, geo, ["id"], MUTABLE, cols, ["id"])
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun feedExperimentsGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateFeedExperiments(pgTsv, dir)
	console.log(
		JSON.stringify(
			{
				...r,
				counts: {
					geoOnly: r.geoOnly.length,
					pgOnly: r.pgOnly.length,
					valueDiffs: r.valueDiffs.length,
					refCountViolations: r.refCountViolations.length,
				},
			},
			null,
			2,
		),
	)
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 ? 0 : 1)
}
