/**
 * The for_you_config gate (RUNBOOK 2c-config): diff the Postgres `personalization.for_you_config` row against the keyed
 * record in the config system Space. Read only; two sets of files, as votesGate.ts (compare, pass rule).
 *
 *   - Postgres: one TSV line, `\copy (select 1, max_boost_days, interest_half_saturation, exploration_share, max_topics
 *     from personalization.for_you_config) to stdout` (PG_COLUMNS order, `\N` for null);
 *   - one TSV per record column from geo-db GQL, named `fyc-<name>.tsv` for the names in COLUMNS, each line
 *     `record<TAB>value<TAB>count`.
 */
import {readFileSync} from "node:fs"
import {forYouConfigRecordId} from "./forYouConfigKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

export const PG_COLUMNS = [
	"id",
	"max_boost_days",
	"interest_half_saturation",
	"exploration_share",
	"max_topics",
] as const
const MUTABLE = PG_COLUMNS.filter((c) => c !== "id")

/** Numbers compare as numbers: Postgres prints `0.1`, geo-db may print `0.1` or `0.10`. */
const canon = (v: string) => (Number.isFinite(Number(v)) && v.trim() !== "" ? String(Number(v)) : v)

export function gateForYouConfig(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const f = line.split("\t")
		const row = Object.fromEntries(PG_COLUMNS.map((c, i) => [c, canon(f[i] as string)])) as Record<string, string>
		pg.set(forYouConfigRecordId(), {id: "1", ...Object.fromEntries(MUTABLE.map((c) => [c, row[c] as string]))})
	}
	const names = ["id", ...MUTABLE]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/fyc-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.id!.keys())
		geo.set(id, Object.fromEntries(names.map((n) => [n, canon(cols[n]!.get(id)?.value ?? "<missing>")])))
	return compare("for_you_config", pg, geo, ["id"], [...MUTABLE], cols, [])
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun forYouConfigGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateForYouConfig(pgTsv, dir)
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
