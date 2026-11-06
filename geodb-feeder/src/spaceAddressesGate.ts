/**
 * The space-addresses gate (RUNBOOK 2c-duodecies): diff Postgres `spaces.address` against the keyed records in the
 * governance system Space. Read only; two sets of files, as votesGate.ts. Output and pass rule are votesGate.ts's
 * (compare): a pg-only key or a value difference is unexplained; geo-db-only is expected (an SPACE_ADDRESS_UPDATED for a
 * Space Postgres lacks is written anyway; spaceAddressesKeyed.ts). A create that overwrote an address an update had set
 * (Postgres' ON CONFLICT DO NOTHING) shows as a value difference.
 *
 *   - Postgres rows from `\copy (select replace(id::text,'-',''), address from spaces where address is not null) to
 *     stdout`, one TSV, columns `space`, `address` (PG_COLUMNS), the id dashless and the address exactly as stored;
 *   - one TSV per record column from geo-db GQL (`MATCH (r)-[:`<column>`]->(x) RETURN r, x`), named `sa-<name>.tsv`
 *     for `space` and `address`, each line `record<TAB>value<TAB>count`. The address is a text value, compared as is.
 */
import {readFileSync} from "node:fs"
import {cid} from "./ids.js"
import {spaceAddressRecordId} from "./spaceAddressesKeyed.js"
import {type Cols, compare, type GateResult, readCol, type Side} from "./votesGate.js"

export const PG_COLUMNS = ["space", "address"] as const

export function gateSpaceAddresses(pgTsv: string, dir: string): GateResult {
	const pg: Side = new Map()
	for (const line of readFileSync(pgTsv, "utf8").split("\n")) {
		if (!line) continue
		const [space, address] = line.split("\t") as [string, string]
		pg.set(spaceAddressRecordId(space), {space: cid("entity", space), address})
	}
	const names = ["space", "address"]
	const cols: Cols = Object.fromEntries(names.map((n) => [n, readCol(`${dir}/sa-${n}.tsv`)]))
	const geo: Side = new Map()
	for (const id of cols.space!.keys()) geo.set(id, {space: cols.space!.get(id)?.value ?? "<missing>", address: cols.address!.get(id)?.value ?? "<missing>"})
	return compare("space_addresses", pg, geo, ["space"], ["address"], cols, ["space", "address"])
}

if (import.meta.main) {
	const [pgTsv, dir] = process.argv.slice(2) as [string, string]
	if (!pgTsv || !dir) {
		console.error("usage: bun spaceAddressesGate.ts <postgres.tsv> <geodb-dump-dir>")
		process.exit(2)
	}
	const r = gateSpaceAddresses(pgTsv, dir)
	const head = (a: unknown[]) => a.slice(0, 20)
	console.log(JSON.stringify({...r, geoOnly: head(r.geoOnly), pgOnly: head(r.pgOnly), valueDiffs: head(r.valueDiffs), counts: {geoOnly: r.geoOnly.length, pgOnly: r.pgOnly.length, valueDiffs: r.valueDiffs.length, refCountViolations: r.refCountViolations.length}}, null, 2))
	process.exit(r.pgOnly.length === 0 && r.valueDiffs.length === 0 && r.refCountViolations.length === 0 ? 0 : 1)
}
