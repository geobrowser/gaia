/**
 * The feed-experiment-members gate (RUNBOOK 2c-config-experiment-members): diff Postgres
 * `personalization.feed_experiment_members` against the keyed records in the config system Space. Read only.
 *
 *   - Postgres: `\copy (select experiment_id, user_id, interleaved from personalization.feed_experiment_members) to stdout`;
 *   - one TSV per record column from geo-db GQL, named `fem-<column>.tsv` for experiment_id, user_id, interleaved, each
 *     line `record<TAB>value<TAB>count`.
 *
 * A membership deleted from Postgres shows as geo-db-only (not a failure).
 */
import {readFileSync} from "node:fs"
import {feedExperimentMemberRecordId} from "./feedExperimentMembersKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

const KEYS = ["experiment_id", "user_id"]
const MUTABLE = ["interleaved"]

export function gateFeedExperimentMembers(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const [experiment, user, interleaved] = line.split("\t") as [string, string, string]
		pg.set(feedExperimentMemberRecordId(experiment, user), {
			experiment_id: experiment,
			user_id: user.toLowerCase(),
			interleaved: interleaved === "t" || interleaved === "true" ? "true" : "false",
		})
	}
	const names = [...KEYS, ...MUTABLE]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/fem-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const rec of cols.experiment_id!.keys()) {
		const g: Record<string, string> = {}
		for (const n of names) g[n] = cols[n]!.get(rec)?.value ?? "<missing>"
		geo.set(rec, g)
	}
	return compare("feed_experiment_members", pg, geo, KEYS, MUTABLE, cols, KEYS)
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun feedExperimentMembersGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateFeedExperimentMembers(pgTsv, dir)
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
