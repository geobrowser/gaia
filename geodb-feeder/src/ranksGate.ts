/**
 * The ranks gate (RUNBOOK 2c-decies): diff Postgres `ranks.ranking_blocks`, `ranks.rankings` and `ranks.ranking_items`
 * against the keyed records in the votes system Space. Read only; two sets of files per table, as votesGate.ts.
 * Output and pass rule are votesGate.ts's (compare): a pg-only key or a value difference is unexplained; geo-db-only
 * is expected (an item of a ranking's earlier generation, superseded; ranksKeyed.ts). Run once per table.
 *
 *   - Postgres rows from `\copy (...) to stdout`, one TSV per table, `\N` for null, the columns and order of
 *     PG_COLUMNS[table] below. Ids are dashless (`replace(x::text, '-', '')`); timestamps are whole unix seconds
 *     (`extract(epoch from x)::bigint`); `weight` is `weight::text`;
 *   - one TSV per record column from geo-db GQL (`MATCH (r)-[:`<column>`]->(x) RETURN r, x`), named
 *     `<prefix>-<name>.tsv` (prefix `rb`, `rk`, `ri` for blocks, rankings, items), for the names in the table's
 *     columns, each line `record<TAB>value<TAB>count`. A fact that is null in Postgres has no line.
 *   - `author_address` (rankings) and `generation` (items) are not compared: the first is carried by the backfill
 *     only, the second is the stream's own bookkeeping.
 */
import {readFileSync} from "node:fs"
import {cid} from "./ids.js"
import {rankingBlockRecordId, rankingItemRecordId, rankingRecordId} from "./ranksKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

type Kind = "instant" | "text" | "int" | "decimal" | "ref"
type Table = {prefix: string; family: string; keys: string[]; cols: Record<string, Kind>; id: (row: Record<string, string>) => string}

/** The columns of each table, in `\copy` order, with how each is normalised. Keys first. */
export const TABLES = {
	blocks: {
		prefix: "rb",
		family: "ranking_blocks",
		keys: ["block", "space"],
		cols: {block: "ref", space: "ref", name: "text", filter: "text", start_date: "instant", end_date: "instant", restriction: "ref", ranking_type: "ref", submission_frequency: "int"},
		id: (r) => rankingBlockRecordId(r.block as string, r.space as string),
	},
	rankings: {
		prefix: "rk",
		family: "rankings",
		keys: ["ranking", "space"],
		cols: {ranking: "ref", space: "ref", block: "ref", rank_type: "text", submitted_at: "instant", updated_at_block: "int", update_index: "int"},
		id: (r) => rankingRecordId(r.ranking as string, r.space as string),
	},
	items: {
		prefix: "ri",
		family: "ranking_items",
		keys: ["ranking", "entity", "space"],
		cols: {ranking: "ref", entity: "ref", space: "ref", position: "text", weight: "decimal"},
		id: (r) => rankingItemRecordId(r.ranking as string, r.entity as string, r.space as string),
	},
} as const satisfies Record<string, Table>

export type RanksTable = keyof typeof TABLES

/** The columns of each table's Postgres TSV, in order. */
export const PG_COLUMNS = {
	blocks: Object.keys(TABLES.blocks.cols),
	rankings: Object.keys(TABLES.rankings.cols),
	items: Object.keys(TABLES.items.cols),
} as const

const ABSENT = "<missing>"
const instantSeconds = (v: string) => /^instant:(\d+)\.\d+$/.exec(v)?.[1] ?? v
const decimal = (v: string) => (v === ABSENT ? v : String(Number(v.replace(/^decimal:/, ""))))

/** Postgres value to the compared form: `\N` is absent, references are `cid("entity", id)`. */
const pgValue = (kind: Kind, v: string): string => {
	if (v === "\\N") return ABSENT
	if (kind === "ref") return cid("entity", v)
	return kind === "decimal" ? decimal(v) : v
}
const geoValue = (kind: Kind, v: string): string => (kind === "instant" ? instantSeconds(v) : kind === "decimal" ? decimal(v) : v)

export function gateRanks(table: RanksTable, pgTsv: string, dir: string): GateResult {
	const t: Table = TABLES[table]
	const names = Object.keys(t.cols)
	const mutable = names.filter((n) => !t.keys.includes(n))
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const f = line.split("\t")
		const row = Object.fromEntries(names.map((c, i) => [c, f[i] as string])) as Record<string, string>
		pg.set(t.id(row), Object.fromEntries(names.map((c) => [c, pgValue(t.cols[c] as Kind, row[c] as string)])))
	}
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/${t.prefix}-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols[t.keys[0] as string]!.keys()) {
		geo.set(id, Object.fromEntries(names.map((n) => [n, geoValue(t.cols[n] as Kind, cols[n]!.get(id)?.value ?? ABSENT)])))
	}
	return compare(`ranks.${t.family}`, pg, geo, t.keys, mutable, cols, t.keys)
}

if (import.meta.main) {
	const [table, pgTsv, dir] = process.argv.slice(2) as [RanksTable, string, string]
	if (!(table in TABLES) || !pgTsv || !dir) {
		console.error("usage: bun ranksGate.ts <blocks|rankings|items> <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateRanks(table, pgTsv, dir)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(JSON.stringify({...r, geoOnly: head(r.geoOnly), pgOnly: head(r.pgOnly), valueDiffs: head(r.valueDiffs), counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length, valueDiffs: r.valueDiffs.length, refCountViolations: r.refCountViolations.length}}, null, 2))
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 ? 0 : 1)
}
