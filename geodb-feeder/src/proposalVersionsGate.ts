/**
 * The proposal-versions gate (RUNBOOK 2c-octies): diff Postgres `proposal_versions` against the keyed records in the
 * governance system Space. Read only; two sets of files, as votesGate.ts. Output and pass rule are votesGate.ts's
 * (compare): a pg-only key or a value difference is unexplained; geo-db-only is expected (proposalVersionsKeyed.ts).
 * Two additions, both from proposalVersionsKeyed.ts:
 *
 *   - `escalated`: a version geo-db holds as Fast and Postgres as Slow had its settings rewritten in place by a
 *     fast-to-slow escalation, an event the consumer does not read. Its differences in the settings columns
 *     (voting_mode, start_time, end_time, quorum, partial, universal, flat, execute_by) are listed there and are not
 *     failures. Any other difference of such a record still is.
 *   - `ordinalViolations`: the records do not store the ordinal; Postgres's `proposal_version` must equal the rank of the
 *     version's block among the proposal's rows (1 for the lowest block). A row that breaks that cannot be keyed by block.
 *
 *   - Postgres rows from `\copy (...) to stdout`, one TSV, `\N` for null, the same columns and order as PG_COLUMNS below
 *     (`proposal_id`, `proposal_version`, `version_created_at_block`, `voting_mode::text`, `start_time`, `end_time`,
 *     `quorum`, the three thresholds, `execute_by`, `name`, `version_created_at`; ids dashless);
 *   - one TSV per record column from geo-db GQL (`MATCH (r)-[:`<column>`]->(x) RETURN r, x`), named `pv-<name>.tsv` for
 *     the names in COLUMNS, each line `record<TAB>value<TAB>count`. A column the version does not have has no line.
 */
import {readFileSync} from "node:fs"
import {cid} from "./ids.js"
import {proposalVersionRecordId} from "./proposalVersionsKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

/** proposal, version, block, voting_mode, start_time, end_time, quorum, partial, universal, flat, execute_by, name, created_at. */
export const PG_COLUMNS = [
	"proposal", "version", "block", "voting_mode", "start_time", "end_time", "quorum", "partial", "universal", "flat", "execute_by", "name", "created_at",
] as const

const MUTABLE = ["voting_mode", "start_time", "end_time", "quorum", "partial", "universal", "flat", "execute_by", "name", "created_at"]
/** The columns a settings escalation rewrites. */
const SETTINGS = ["voting_mode", "start_time", "end_time", "quorum", "partial", "universal", "flat", "execute_by"]
const ABSENT = "\\N"

export type ProposalVersionsGateResult = GateResult & {
	escalated: {record: string; columns: string[]}[]
	ordinalViolations: string[]
}

export function gateProposalVersions(pgTsv: string, dir: string): ProposalVersionsGateResult {
	const pg: Side = new Map()
	const perProposal = new Map<string, {block: bigint; version: number}[]>()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const f = line.split("\t")
		const row = Object.fromEntries(PG_COLUMNS.map((c, i) => [c, f[i] as string])) as Record<string, string>
		const block = BigInt.asUintN(64, BigInt(row.block as string))
		const id = proposalVersionRecordId({proposal: row.proposal as string, block})
		pg.set(id, {
			proposal: cid("entity", row.proposal as string),
			block: row.block as string,
			...Object.fromEntries(MUTABLE.map((c) => [c, row[c] as string])),
		})
		const l = perProposal.get(row.proposal as string) ?? []
		l.push({block: BigInt(row.block as string), version: Number(row.version)})
		perProposal.set(row.proposal as string, l)
	}
	const ordinalViolations: string[] = []
	for (const [proposal, l] of perProposal) {
		l.sort((a, b) => (a.block < b.block ? -1 : a.block > b.block ? 1 : 0))
		l.forEach((v, i) => {
			if (v.version !== i + 1) ordinalViolations.push(`${proposal}:block ${v.block}:version ${v.version}:rank ${i + 1}`)
		})
	}
	const names = ["proposal", "block", ...MUTABLE]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/pv-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.proposal!.keys()) {
		const g: Record<string, string> = {}
		for (const n of names) g[n] = cols[n]!.get(id)?.value ?? (MUTABLE.includes(n) ? ABSENT : "<missing>")
		geo.set(id, g)
	}
	const r = compare("proposal_versions", pg, geo, ["proposal", "block"], MUTABLE, cols, ["proposal"])
	const escalatedRecords = new Set<string>()
	for (const [id, g] of geo) {
		const p = pg.get(id)
		if (p && g.voting_mode === "Fast" && p.voting_mode === "Slow") escalatedRecords.add(id)
	}
	const escalated: ProposalVersionsGateResult["escalated"] = []
	const valueDiffs: GateResult["valueDiffs"] = []
	for (const d of r.valueDiffs) {
		if (escalatedRecords.has(d.record) && SETTINGS.includes(d.column)) {
			let e = escalated.find((x) => x.record === d.record)
			if (!e) escalated.push((e = {record: d.record, columns: []}))
			e.columns.push(d.column)
		} else valueDiffs.push(d)
	}
	return {...r, valueDiffs, escalated, ordinalViolations}
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun proposalVersionsGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateProposalVersions(pgTsv, dir)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(
		JSON.stringify(
			{
				...r,
				geoOnly: head(r.geoOnly),
				pgOnly: head(r.pgOnly),
				valueDiffs: head(r.valueDiffs),
				escalated: head(r.escalated),
				ordinalViolations: head(r.ordinalViolations),
				counts: {
					geoOnly: r.geoOnly.length,
					pgOnly: r.pgOnly.length,
					valueDiffs: r.valueDiffs.length,
					refCountViolations: r.refCountViolations.length,
					escalated: r.escalated.length,
					ordinalViolations: r.ordinalViolations.length,
				},
			},
			null,
			2,
		),
	)
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 && r.ordinalViolations.length === 0 ? 0 : 1)
}
