/**
 * The proposal-versions gate (RUNBOOK 2c-octies): diff Postgres `proposal_versions` against the keyed records in the
 * governance system Space. Read only; two sets of files, as votesGate.ts. Output and pass rule are votesGate.ts's
 * (compare): a pg-only key or a value difference is unexplained; geo-db-only is expected (proposalVersionsKeyed.ts).
 * Two additions:
 *
 *   - escalations: before the compare, each version's settings (voting_mode, start_time, end_time, quorum, partial,
 *     universal, flat, execute_by) are overlaid with the latest escalation record that hit it, resolved as the reader does
 *     (proposalEscalationKeyed.ts resolveEscalations); Postgres holds the escalated values. An escalation that hits no
 *     version is reported as `orphanEscalations` (kg-indexer did nothing for it), not as a failure.
 *   - `ordinalViolations`: the records do not store the ordinal; Postgres's `proposal_version` must equal the rank of the
 *     version's block among the proposal's rows (1 for the lowest block). A row that breaks that cannot be keyed by block.
 *
 *   - Postgres rows from `\copy (...) to stdout`, one TSV, `\N` for null, the same columns and order as PG_COLUMNS below
 *     (`proposal_id`, `proposal_version`, `version_created_at_block`, `voting_mode::text`, `start_time`, `end_time`,
 *     `quorum`, the three thresholds, `execute_by`, `name`, `version_created_at`; ids dashless);
 *   - one TSV per record column from geo-db GQL (`MATCH (r)-[:`<column>`]->(x) RETURN r, x`), named `pv-<name>.tsv` for
 *     the names in COLUMNS, each line `record<TAB>value<TAB>count`. A column the version does not have has no line;
 *   - the same for the escalation records, named `pe-<name>.tsv` for proposal, block and the SETTINGS names (an empty or
 *     missing file means no escalations).
 */
import {existsSync, readFileSync} from "node:fs"
import {cid} from "./ids.js"
import {resolveEscalations} from "./proposalEscalationKeyed.js"
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
	orphanEscalations: string[]
	ordinalViolations: string[]
}

/** Overlay each version in `geo` with the escalation that hit it (pe-*.tsv dumps in `dir`); returns the orphan escalation records. */
function overlayEscalations(geo: Side, dir: string): string[] {
	const names = ["proposal", "block", ...SETTINGS]
	if (!existsSync(`${dir}/pe-proposal.tsv`)) return []
	const cols: Cols = Object.fromEntries(names.map((n) => [n, existsSync(`${dir}/pe-${n}.tsv`) ? readCol(`${dir}/pe-${n}.tsv`) : new Map()]))
	type Esc = {id: string; block: bigint; settings: Record<string, string>}
	type Ver = {id: string; block: bigint}
	const escByProposal = new Map<string, Esc[]>()
	for (const id of cols.proposal!.keys()) {
		const proposal = cols.proposal!.get(id)!.value
		const block = cols.block!.get(id)?.value
		if (block === undefined) continue
		const settings = Object.fromEntries(SETTINGS.map((n) => [n, cols[n]!.get(id)?.value ?? ABSENT]))
		const l = escByProposal.get(proposal) ?? []
		l.push({id, block: BigInt.asUintN(64, BigInt(block)), settings})
		escByProposal.set(proposal, l)
	}
	const verByProposal = new Map<string, Ver[]>()
	for (const [id, g] of geo) {
		const l = verByProposal.get(g.proposal as string) ?? []
		l.push({id, block: BigInt.asUintN(64, BigInt(g.block as string))})
		verByProposal.set(g.proposal as string, l)
	}
	const orphans: string[] = []
	for (const [proposal, escs] of escByProposal) {
		const {applied, orphans: o} = resolveEscalations(verByProposal.get(proposal) ?? [], escs)
		for (const [v, e] of applied) Object.assign(geo.get(v.id)!, e.settings)
		orphans.push(...o.map((e) => e.id))
	}
	return orphans
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
	const orphanEscalations = overlayEscalations(geo, dir)
	const r = compare("proposal_versions", pg, geo, ["proposal", "block"], MUTABLE, cols, ["proposal"])
	return {...r, orphanEscalations, ordinalViolations}
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
				orphanEscalations: head(r.orphanEscalations),
				ordinalViolations: head(r.ordinalViolations),
				counts: {
					geoOnly: r.geoOnly.length,
					pgOnly: r.pgOnly.length,
					valueDiffs: r.valueDiffs.length,
					refCountViolations: r.refCountViolations.length,
					orphanEscalations: r.orphanEscalations.length,
					ordinalViolations: r.ordinalViolations.length,
				},
			},
			null,
			2,
		),
	)
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 && r.ordinalViolations.length === 0 ? 0 : 1)
}
