/**
 * The interest-config gate (RUNBOOK 2c-config-interest): diff Postgres `personalization.interest_config` and
 * `personalization.interest_signal_weights` against the keyed records in the config system Space. Read only; two sets of
 * files per table, as entityRankingConfigGate.ts.
 *
 *   interest_config: one TSV line, `\copy (select 1, half_life_days, spread_fraction, spread_max_neighbours,
 *     cooccurrence_min_support, cooccurrence_max_topics_per_entity, extract(epoch from sweep_lookback)
 *     from personalization.interest_config) to stdout`; geo-db files `ic-<column>.tsv` (`ic-id`, then the names below).
 *   interest_signal_weights: `\copy (select kind, weight, decays, coalesce(note, '') from
 *     personalization.interest_signal_weights) to stdout`; geo-db files `isw-kind.tsv`, `isw-weight.tsv`, `isw-decays.tsv`,
 *     `isw-note.tsv` (a null note has no statement; it compares as the empty string).
 *
 * Each geo-db line is `record<TAB>value<TAB>count`. A kind removed from Postgres shows as geo-db-only (not a failure).
 */
import {readFileSync} from "node:fs"
import {
	INTEREST_CONFIG_COLUMNS,
	interestConfigRecordId,
	interestSignalWeightRecordId,
} from "./interestConfigKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

const CONFIG_MUTABLE: string[] = INTEREST_CONFIG_COLUMNS.map((c) => c.pg)
const WEIGHT_MUTABLE = ["weight", "decays", "note"]

/** Numbers compare as numbers: Postgres prints `2592000`, geo-db may print `2592000.0`. */
const canon = (v: string) => (Number.isFinite(Number(v)) && v.trim() !== "" ? String(Number(v)) : v)
const lines = (file: string) =>
	readFileSync(file, "utf8")
		.split("\n")
		.filter((l) => l)

export function gateInterestConfig(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of lines(pgTsv)) {
		const f = line.split("\t")
		pg.set(interestConfigRecordId(), {
			id: "1",
			...Object.fromEntries(CONFIG_MUTABLE.map((c, i) => [c, canon(f[i + 1] as string)])),
		})
	}
	const names = ["id", ...CONFIG_MUTABLE]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/ic-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.id!.keys())
		geo.set(id, Object.fromEntries(names.map((n) => [n, canon(cols[n]!.get(id)?.value ?? "<missing>")])))
	return compare("interest_config", pg, geo, ["id"], CONFIG_MUTABLE, cols, [])
}

export function gateInterestSignalWeights(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of lines(pgTsv)) {
		const [kind, weight, decays, note] = line.split("\t") as [string, string, string, string | undefined]
		pg.set(interestSignalWeightRecordId(kind), {
			kind,
			weight: canon(weight),
			decays: decays === "t" || decays === "true" ? "true" : "false",
			note: note ?? "",
		})
	}
	const names = ["kind", ...WEIGHT_MUTABLE]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/isw-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.kind!.keys()) {
		const g: Record<string, string> = {}
		for (const n of names) {
			const v = cols[n]!.get(id)?.value
			g[n] = n === "note" ? (v ?? "") : n === "weight" ? canon(v ?? "<missing>") : (v ?? "<missing>")
		}
		geo.set(id, g)
	}
	return compare("interest_signal_weights", pg, geo, ["kind"], WEIGHT_MUTABLE, cols, ["kind"])
}

if (import.meta.main) {
	const [which, pgTsv, dir] = process.argv.slice(2) as [string, string, string]
	if ((which !== "config" && which !== "weights") || !pgTsv || !dir) {
		console.error("usage: bun interestConfigGate.ts <config|weights> <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = which === "config" ? gateInterestConfig(pgTsv, dir) : gateInterestSignalWeights(pgTsv, dir)
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
