/**
 * The entity_ranking_config gate (RUNBOOK 2c-config-ranking): diff the Postgres `entity_ranking_config` row against the
 * keyed record in the config system Space. Read only; two sets of files, as forYouConfigGate.ts.
 *
 *   - Postgres: one TSV line, `\copy (select 1, <PG_COLUMNS minus id, in order> from entity_ranking_config) to stdout`
 *     (`\N` for null);
 *   - one TSV per record column from geo-db GQL, named `erc-<pg column>.tsv` (and `erc-id.tsv`), each line
 *     `record<TAB>value<TAB>count`.
 */
import {readFileSync} from "node:fs"
import {ENTITY_RANKING_COLUMNS, entityRankingConfigRecordId} from "./entityRankingConfigKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

const MUTABLE: string[] = ENTITY_RANKING_COLUMNS.map((c) => c.pg)
export const PG_COLUMNS = ["id", ...MUTABLE] as const

/** Numbers compare as numbers: Postgres prints `100000`, geo-db may print `100000` or `100000.0`. */
const canon = (v: string) => (Number.isFinite(Number(v)) && v.trim() !== "" ? String(Number(v)) : v)

export function gateEntityRankingConfig(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const f = line.split("\t")
		const row = Object.fromEntries(PG_COLUMNS.map((c, i) => [c, canon(f[i] as string)])) as Record<string, string>
		pg.set(entityRankingConfigRecordId(), {id: "1", ...Object.fromEntries(MUTABLE.map((c) => [c, row[c] as string]))})
	}
	const names = ["id", ...MUTABLE]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/erc-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.id!.keys())
		geo.set(id, Object.fromEntries(names.map((n) => [n, canon(cols[n]!.get(id)?.value ?? "<missing>")])))
	return compare("entity_ranking_config", pg, geo, ["id"], MUTABLE, cols, [])
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun entityRankingConfigGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateEntityRankingConfig(pgTsv, dir)
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
