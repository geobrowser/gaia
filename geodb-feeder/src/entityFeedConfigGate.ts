/**
 * The entity-feed-config gate (RUNBOOK 2c-config-entity-feed): diff Postgres `entity_type_weights`,
 * `entity_type_exclusions` and `entity_feed_blocklist` against the keyed records in the config system Space. Read only.
 *
 *   - Postgres, one TSV per table (`\copy (...) to stdout`; a null is `\N`):
 *       weights:    select type_id, weight, note from entity_type_weights
 *       exclusions: select type_id, note from entity_type_exclusions
 *       blocklist:  select entity_id, reason from entity_feed_blocklist
 *   - one TSV per record column from geo-db GQL, `record<TAB>value<TAB>count` per line, named `etw-<column>.tsv`
 *     (type_id, weight, note), `etx-<column>.tsv` (type_id, note) and `efb-<column>.tsv` (entity_id, reason).
 *
 * A row deleted from Postgres shows as geo-db-only (not a failure); a null note or reason compares as empty.
 */
import {readFileSync} from "node:fs"
import {entityFeedBlockRecordId, entityTypeExclusionRecordId, entityTypeWeightRecordId} from "./entityFeedConfigKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

const canon = (v: string) => (Number.isFinite(Number(v)) && v.trim() !== "" ? String(Number(v)) : v)
const nullable = (v: string | undefined) => (v === undefined || v === "\\N" ? "" : v)

type Spec = {family: string; prefix: string; key: string; mutable: string[]; recordId: (k: string) => string; numeric?: string}

function gate(spec: Spec, pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const f = line.split("\t")
		const row: Record<string, string> = {[spec.key]: (f[0] as string).toLowerCase()}
		spec.mutable.forEach((m, i) => {
			const v = nullable(f[i + 1])
			row[m] = m === spec.numeric ? canon(v) : v
		})
		pg.set(spec.recordId(f[0] as string), row)
	}
	const names = [spec.key, ...spec.mutable]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/${spec.prefix}-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const rec of cols[spec.key]!.keys()) {
		const g: Record<string, string> = {}
		for (const n of names) {
			const v = cols[n]!.get(rec)?.value
			g[n] = n === spec.numeric ? canon(v ?? "<missing>") : n === spec.key ? (v ?? "<missing>") : (v ?? "")
		}
		geo.set(rec, g)
	}
	return compare(spec.family, pg, geo, [spec.key], spec.mutable, cols, [spec.key])
}

export const gateEntityTypeWeights = (pgTsv: string, dir: string) =>
	gate({family: "entity_type_weights", prefix: "etw", key: "type_id", mutable: ["weight", "note"], recordId: entityTypeWeightRecordId, numeric: "weight"}, pgTsv, dir)
export const gateEntityTypeExclusions = (pgTsv: string, dir: string) =>
	gate({family: "entity_type_exclusions", prefix: "etx", key: "type_id", mutable: ["note"], recordId: entityTypeExclusionRecordId}, pgTsv, dir)
export const gateEntityFeedBlocks = (pgTsv: string, dir: string) =>
	gate({family: "entity_feed_blocklist", prefix: "efb", key: "entity_id", mutable: ["reason"], recordId: entityFeedBlockRecordId}, pgTsv, dir)

if (import.meta.main) {
	const [which, pgTsv, dir] = process.argv.slice(2) as [string, string, string]
	const gates = {weights: gateEntityTypeWeights, exclusions: gateEntityTypeExclusions, blocklist: gateEntityFeedBlocks} as const
	const g = gates[which as keyof typeof gates]
	if (!g || !pgTsv || !dir) {
		console.error("usage: bun entityFeedConfigGate.ts <weights|exclusions|blocklist> <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = g(pgTsv, dir)
	console.log(
		JSON.stringify(
			{...r, counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length, valueDiffs: r.valueDiffs.length, refCountViolations: r.refCountViolations.length}},
			null,
			2,
		),
	)
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 ? 0 : 1)
}
