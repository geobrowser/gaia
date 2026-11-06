/**
 * The votes gate (RUNBOOK 2c-ter, 2c-quater): diff Postgres `proposal_votes` / `user_votes` against the keyed records
 * in the votes system Space. Read only. Input is two sets of files, so it needs no connection of its own:
 *   - Postgres rows from `\copy (...) to stdout` (columns documented at PG_COLUMNS below);
 *   - one TSV per record column from geo-db GQL (`MATCH (r)-[:`<column>`]->(x) RETURN r, x`, the column value as
 *     `r.`<column>``), each line `record<TAB>value<TAB>count`.
 * Output: row counts, a checksum over the compared fields, one-active-reference-per-record, and every difference
 * classified as geo-db-only (expected: kg-indexer drops whole blocks), pg-only, or value-differs. A pg-only key or a
 * value difference where Postgres is not older is unexplained, and the gate does not pass on it.
 */
import {createHash} from "node:crypto"
import {readFileSync} from "node:fs"
import {cid} from "./ids.js"
import {proposalVoteRecordId} from "./proposalVotesKeyed.js"
import {userVoteRecordId, voteObjectRef} from "./votesKeyed.js"

export type Cols = Record<string, Map<string, {value: string; count: number}>>

export type GateResult = {
	family: string
	pgRows: number
	geoRecords: number
	pgChecksum: string
	geoChecksum: string
	matched: number
	geoOnly: string[]
	pgOnly: string[]
	valueDiffs: {record: string; column: string; pg: string; geo: string}[]
	refCountViolations: string[]
}

/** key -> record, plus the canonical compared fields for the checksum. */
type Side = Map<string, Record<string, string>>

const sha = (lines: string[]) => createHash("sha256").update(lines.sort().join("\n")).digest("hex")

export function readCol(path: string): Map<string, {value: string; count: number}> {
	const m = new Map<string, {value: string; count: number}>()
	for (const line of readFileSync(path, "utf8").split("\n")) {
		if (!line) continue
		const [r, v, c] = line.split("\t")
		m.set(r as string, {value: v as string, count: Number(c)})
	}
	return m
}

const instantSeconds = (v: string) => {
	const m = /^instant:(\d+)\.\d+$/.exec(v)
	return m ? m[1] : v
}

function compare(family: string, pg: Side, geo: Side, keyFields: string[], mutable: string[], cols: Cols, refCols: string[]): GateResult {
	const geoOnly: string[] = []
	const pgOnly: string[] = []
	const valueDiffs: GateResult["valueDiffs"] = []
	let matched = 0
	for (const [id, g] of geo) {
		const p = pg.get(id)
		if (!p) {
			geoOnly.push(id)
			continue
		}
		matched++
		for (const f of [...keyFields, ...mutable]) if (p[f] !== g[f]) valueDiffs.push({record: id, column: f, pg: p[f] as string, geo: g[f] as string})
	}
	for (const id of pg.keys()) if (!geo.has(id)) pgOnly.push(id)
	const refCountViolations: string[] = []
	for (const c of refCols) for (const [id, v] of cols[c] ?? []) if (v.count !== 1) refCountViolations.push(`${id}:${c}:${v.count}`)
	const line = (s: Record<string, string>) => [...keyFields, ...mutable.slice(0, 1)].map((f) => s[f]).join("|")
	return {
		family,
		pgRows: pg.size,
		geoRecords: geo.size,
		pgChecksum: sha([...pg.values()].map(line)),
		geoChecksum: sha([...geo.values()].map(line)),
		matched,
		geoOnly,
		pgOnly,
		valueDiffs,
		refCountViolations,
	}
}

const VOTE = {Yes: "1", No: "2", Abstain: "3"} as Record<string, string>

/** PG_COLUMNS (proposal_votes): proposal, version, voter, space, vote text, created_at, created_at_block, all ids dashless. */
export function gateProposalVotes(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const [proposal, version, voter, space, vote, createdAt, block] = line.split("\t") as string[]
		const id = proposalVoteRecordId({proposal: proposal as string, version: Number(version), voter: voter as string})
		pg.set(id, {
			proposal: cid("entity", proposal as string),
			version: version as string,
			voter: cid("entity", voter as string),
			space: cid("entity", space as string),
			vote: VOTE[vote as string] ?? `?${vote}`,
			created_at: createdAt as string,
			block: block as string,
		})
	}
	const names = ["proposal", "version", "voter", "space", "vote", "created_at", "block"]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/pv-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.proposal!.keys()) {
		const g: Record<string, string> = {}
		for (const n of names) {
			const v = cols[n]!.get(id)?.value ?? "<missing>"
			g[n] = n === "created_at" ? (instantSeconds(v) as string) : v
		}
		geo.set(id, g)
	}
	return compare("proposal_votes", pg, geo, ["proposal", "version", "voter", "space"], ["vote", "created_at", "block"], cols, ["proposal", "voter", "space"])
}

/** PG_COLUMNS (user_votes): user, object, object_type, space, vote_kind, vote_type, voted_at seconds, ids dashless. */
export function gateUserVotes(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const [user, object, ot, space, kind, dir_, at] = line.split("\t") as string[]
		const objectType = Number(ot) as 0 | 1
		const id = userVoteRecordId({user: user as string, object: object as string, objectType, space: space as string, kind: Number(kind)})
		pg.set(id, {
			user: cid("entity", user as string),
			object: voteObjectRef(object as string, objectType),
			otype: ot as string,
			space: cid("entity", space as string),
			kind: kind as string,
			direction: dir_ as string,
			voted_at: at as string,
		})
	}
	const names = ["user", "object", "otype", "space", "kind", "direction", "voted_at"]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/uv-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.user!.keys()) {
		const g: Record<string, string> = {}
		for (const n of names) {
			const v = cols[n]!.get(id)?.value ?? "<missing>"
			g[n] = n === "voted_at" ? (instantSeconds(v) as string) : v
		}
		geo.set(id, g)
	}
	return compare("user_votes", pg, geo, ["user", "object", "otype", "space", "kind"], ["direction", "voted_at"], cols, ["user", "object", "space"])
}

if (import.meta.main) {
	const [family, pgTsv, dir] = process.argv.slice(2) as [string, string, string]
	if (!family || !pgTsv || !dir) {
		console.error("usage: bun votesGate.ts <proposal|user> <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = family === "proposal" ? gateProposalVotes(pgTsv, dir) : gateUserVotes(pgTsv, dir)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(JSON.stringify({...r, geoOnly: head(r.geoOnly), pgOnly: head(r.pgOnly), valueDiffs: head(r.valueDiffs), counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length, valueDiffs: r.valueDiffs.length, refCountViolations: r.refCountViolations.length}}, null, 2))
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 ? 0 : 1)
}
