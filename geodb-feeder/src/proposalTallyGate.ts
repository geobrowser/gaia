/**
 * The proposal tally gate (RUNBOOK 2c-quater): diff Postgres's proposal tallies (`proposal_versions` yes, no and
 * abstain counts) against the tally geo-db keeps itself. SHADOW only: read only, nothing reads the geo-db tally yet.
 *
 * geo-db folds every proposal vote record the feeder writes (`proposalVotesKeyed.ts`) into its derived class on the
 * write path: one row per (proposal, version) in collection 9 (`proposal_vote_counts`), key = the proposal's entity id
 * (16 bytes) then the version (u32, big-endian), value = yes, no and abstain as three little-endian u64, group = the
 * owning Space's entity id. The feeder writes the same records as before and needs no change to feed it. It needs a
 * serve with the view sink (geo-db GAP-9) opened with `derived_rows_max` above 0; without either the collection is
 * empty or the read is refused.
 *
 * Input is two files, so it needs no connection of its own:
 *   - Postgres rows from `\copy (...) to stdout`, one per proposal version: proposal, version, yes, no, abstain
 *     (ids dashless, as in votesGate's PG_COLUMNS);
 *   - the derived rows, `geodb-client derived scan 9 0x<space entity hex> --first 512 --pages 100000` for each Space
 *     concatenated, each line `<score><TAB><key hex><TAB><value hex>`; the trailing `more` line is ignored.
 * Output: row counts, and every difference classified. A version with no vote in Postgres is skipped (geo-db has no
 * record for it), geo-db-only is a vote Postgres dropped (kg-indexer drops whole blocks, see proposalVotesKeyed.ts),
 * and a count difference is reported with its direction. Any difference is a finding to investigate, not a bug in
 * either side by itself; the gate exits non-zero on one so nothing is cut over on it.
 */
import {readFileSync} from "node:fs"
import {cid} from "./ids.js"

export type Tally = {yes: number; no: number; abstain: number}

export type TallyGateResult = {
	pgVersions: number
	geoVersions: number
	matched: number
	/** The tally geo-db holds for a version Postgres holds none for (every count zero, or no row). */
	geoOnly: string[]
	/** A version Postgres counts votes for that geo-db has no row for. */
	pgOnly: string[]
	countDiffs: {proposal: string; version: number; pg: Tally; geo: Tally}[]
}

const OPTIONS = ["yes", "no", "abstain"] as const

const sum = (t: Tally) => t.yes + t.no + t.abstain

/** The key `proposal_vote_counts` holds a (proposal, version) under, as hex. */
export const tallyKey = (proposal: string, version: number): string => {
	const v = Buffer.alloc(4)
	v.writeUInt32BE(version)
	return `${cid("entity", proposal)}${v.toString("hex")}`
}

/** One derived row's key and value as hex; null for a line that is not a row (`more`, a blank). */
export function parseTallyRow(line: string): {key: string; tally: Tally} | null {
	const parts = line.split("\t")
	if (parts.length !== 3) return null
	const [, key, value] = parts as [string, string, string]
	if (!/^[0-9a-f]{40}$/.test(key) || !/^[0-9a-f]{48}$/.test(value)) return null
	const bytes = Buffer.from(value, "hex")
	const [yes, no, abstain] = OPTIONS.map((_, i) => Number(bytes.readBigUInt64LE(i * 8)))
	return {key, tally: {yes: yes as number, no: no as number, abstain: abstain as number}}
}

export function gateProposalTallies(pgTsv: string, derivedTsv: string): TallyGateResult {
	const pg = new Map<string, {proposal: string; version: number; tally: Tally}>()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const [proposal, version, yes, no, abstain] = line.split("\t") as string[]
		const tally = {yes: Number(yes), no: Number(no), abstain: Number(abstain)}
		if (sum(tally) === 0) continue
		pg.set(tallyKey(proposal as string, Number(version)), {proposal: proposal as string, version: Number(version), tally})
	}
	const geo = new Map<string, Tally>()
	for (const line of readFileSync(derivedTsv, "utf8").split("\n")) {
		const row = parseTallyRow(line)
		if (row) geo.set(row.key, row.tally)
	}
	const result: TallyGateResult = {pgVersions: pg.size, geoVersions: geo.size, matched: 0, geoOnly: [], pgOnly: [], countDiffs: []}
	for (const [key, g] of geo) {
		const p = pg.get(key)
		if (!p) {
			if (sum(g) > 0) result.geoOnly.push(key)
			continue
		}
		if (OPTIONS.every((o) => p.tally[o] === g[o])) result.matched++
		else result.countDiffs.push({proposal: p.proposal, version: p.version, pg: p.tally, geo: g})
	}
	for (const [key, p] of pg) if (!geo.has(key)) result.pgOnly.push(`${p.proposal}:${p.version}`)
	return result
}

if (import.meta.main) {
	const [pgTsv, derivedTsv] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !derivedTsv) {
		console.error("usage: bun proposalTallyGate.ts <postgres.tsv> <geodb-derived-scan.tsv>")
		process.exit(2)
	}
	const r = gateProposalTallies(pgTsv, derivedTsv)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(
		JSON.stringify(
			{...r, geoOnly: head(r.geoOnly), pgOnly: head(r.pgOnly), countDiffs: head(r.countDiffs), counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length, countDiffs: r.countDiffs.length}},
			null,
			2,
		),
	)
	process.exit(r.pgOnly.length === 0 && r.countDiffs.length === 0 ? 0 : 1)
}
