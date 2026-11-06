/**
 * The type ranking gate: diff Postgres's `entity_type_ranking` (the table behind a type-scoped Explore feed, the most
 * read of the score tables) against the rows gaia's vote-indexer mirrors into geo-db's derived class, collection 0
 * (`entity_type_ranking`). SHADOW only: read only, nothing reads the geo-db rows yet.
 *
 * The mirror is `scoring-service/vote-indexer/src/derived_scores.rs` behind `GEODB_DERIVED_MIRROR`. One row per
 * (type, entity): key = the type's uuid then the entity's uuid (32 raw bytes, no content-id transform), group = the type's
 * uuid, score = `ranking_score`, no value. A serve needs the view sink opened with `derived_rows_max` above 0.
 *
 * Input is two files, so it needs no connection of its own:
 *   - Postgres rows from `\copy (select type_id, entity_id, ranking_score::float8 from entity_type_ranking where
 *     type_id = any(...)) to stdout`: type, entity, score (ids dashless or dashed);
 *   - the derived rows, `geodb-client derived scan 0 0x<type uuid hex> --first 512 --pages 100000` for each type
 *     concatenated, each line `<score><TAB><key hex><TAB><value hex>`; the trailing `more` line is ignored.
 * Gate the types you scanned and no others: a type with no scan shows as pgOnly. Output: row counts and every
 * difference classified (geo-db only, Postgres only, score difference). The mirror is write-through from the
 * vote-indexer, so rows for entities not refreshed since the flag went on are pgOnly until their next refresh or a
 * backfill (`refresh_ranking_scores_for` over the entity set); the gate exits non-zero on any difference so nothing is
 * cut over on it.
 */
import {readFileSync} from "node:fs"

export type TypeRankingGateResult = {
	pgRows: number
	geoRows: number
	matched: number
	geoOnly: string[]
	pgOnly: string[]
	scoreDiffs: {type: string; entity: string; pg: number; geo: number}[]
}

const hex = (id: string): string => id.replaceAll("-", "").toLowerCase()

/** The key a (type, entity) row is held under, as hex: 32 raw bytes. */
export const typeRankingKey = (type: string, entity: string): string => `${hex(type)}${hex(entity)}`

/** Scores are the same f64 on both sides; the scan prints them in decimal, so allow a rounding ulp or two. */
const same = (a: number, b: number): boolean => a === b || Math.abs(a - b) <= 1e-9 * Math.max(Math.abs(a), Math.abs(b))

/** One derived scan row; null for a line that is not a row (`more`, a blank, another collection's key). */
export function parseTypeRankingRow(line: string): {key: string; score: number} | null {
	const parts = line.split("\t")
	if (parts.length !== 3) return null
	const [score, key] = parts as [string, string, string]
	if (!/^[0-9a-f]{64}$/.test(key)) return null
	const n = Number(score)
	return Number.isFinite(n) ? {key, score: n} : null
}

export function gateTypeRanking(pgTsv: string, derivedTsv: string): TypeRankingGateResult {
	const pg = new Map<string, {type: string; entity: string; score: number}>()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const [type, entity, score] = line.split("\t") as string[]
		pg.set(typeRankingKey(type as string, entity as string), {type: hex(type as string), entity: hex(entity as string), score: Number(score)})
	}
	const geo = new Map<string, number>()
	for (const line of readFileSync(derivedTsv, "utf8").split("\n")) {
		const row = parseTypeRankingRow(line)
		if (row) geo.set(row.key, row.score)
	}
	const r: TypeRankingGateResult = {pgRows: pg.size, geoRows: geo.size, matched: 0, geoOnly: [], pgOnly: [], scoreDiffs: []}
	for (const [key, g] of geo) {
		const p = pg.get(key)
		if (!p) r.geoOnly.push(`${key.slice(0, 32)}:${key.slice(32)}`)
		else if (same(p.score, g)) r.matched++
		else r.scoreDiffs.push({type: p.type, entity: p.entity, pg: p.score, geo: g})
	}
	for (const [key, p] of pg) if (!geo.has(key)) r.pgOnly.push(`${p.type}:${p.entity}`)
	return r
}

if (import.meta.main) {
	const [pgTsv, derivedTsv] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !derivedTsv) {
		console.error("usage: bun typeRankingGate.ts <postgres.tsv> <geodb-derived-scan.tsv>")
		process.exit(2)
	}
	const r = gateTypeRanking(pgTsv, derivedTsv)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(
		JSON.stringify(
			{...r, geoOnly: head(r.geoOnly), pgOnly: head(r.pgOnly), scoreDiffs: head(r.scoreDiffs), counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length, scoreDiffs: r.scoreDiffs.length}},
			null,
			2,
		),
	)
	process.exit(r.geoOnly.length === 0 && r.pgOnly.length === 0 && r.scoreDiffs.length === 0 ? 0 : 1)
}
