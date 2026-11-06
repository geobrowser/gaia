/**
 * Postgres rows (the kg-indexer's `values` and `relations`) -> the GRC-20 ops an edit would have carried, so
 * the backfill goes through the feeder's own `translateOps` and writes exactly the rows the feeder writes.
 *
 * The kg-indexer keeps one `values` row per (entity, property, space), with the value in the column of its
 * type (handlers/edits.rs `value_to_value_op`). Each column maps back to the GRC value it came from; the
 * types the feeder does not carry (rect, time, schedule, bytes, embedding) are not selected, so a row holding
 * only those has nothing to map and is skipped, as the feeder skips them.
 */
import {cid} from "./ids.js"
import type {GrcOp, GrcPropertyValue, GrcValue} from "./translate.js"

export type PgValueRow = {
	entity_id: string
	property_id: string
	text: string | null
	language: string | null
	integer: string | null
	float: string | null
	decimal: string | null
	boolean: boolean | null
	datetime: string | null
	date: string | null
	point: string | null
}

export type PgRelationRow = {id: string; from_entity_id: string; to_entity_id: string; type_id: string; position?: string | null}

const I64_MIN = -(2n ** 63n)
const I64_MAX = 2n ** 63n - 1n

export const compactId = (id: string) => id.replaceAll("-", "").toLowerCase()

/** A numeric's text (`-12.3400`) as the mantissa/exponent pair GRC-20 carries; NaN and infinities have none. */
export function decimalValue(text: string): GrcValue | null {
	const m = /^(-?)(\d+)(?:\.(\d+))?$/.exec(text.trim())
	if (!m) return null
	const frac = m[3] ?? ""
	const mantissa = BigInt(`${m[1]}${m[2]}${frac}`)
	const fits = mantissa >= I64_MIN && mantissa <= I64_MAX
	return {type: "decimal", mantissa: {type: fits ? "i64" : "big", value: fits ? mantissa : text}, exponent: frac.length === 0 ? 0 : -frac.length}
}

/** `lat,lon[,alt]`, the kg-indexer's text form of a point. */
export function pointValue(text: string): GrcValue | null {
	const parts = text.split(",").map((p) => Number(p.trim()))
	if (parts.length < 2 || parts.length > 3 || parts.some((n) => !Number.isFinite(n))) return null
	return {type: "point", lat: parts[0], lon: parts[1], alt: parts[2] ?? null}
}

/** One `values` row as the GRC value it was written from, or null when it holds nothing the feeder carries. */
export function grcValue(row: PgValueRow): GrcValue | null {
	if (row.text != null) return {type: "text", value: row.text, language: row.language}
	if (row.integer != null) return {type: "integer", value: BigInt(row.integer)}
	if (row.float != null) return {type: "float", value: Number(row.float)}
	if (row.decimal != null) return decimalValue(row.decimal)
	if (row.boolean != null) return {type: "boolean", value: row.boolean}
	if (row.datetime != null) return {type: "datetime", value: row.datetime}
	if (row.date != null) return {type: "date", value: row.date}
	if (row.point != null) return pointValue(row.point)
	return null
}

/**
 * What geo-db already holds for an entity the replica knows: the properties it has a row for and the
 * statement ids it holds. Null for an entity the replica has never seen, which gets everything.
 */
export type Held = {props: Set<string>; statements: Set<string>}

/**
 * What geo-db holds for an entity, from its card rows. A card row's property is a value's property or a relation's
 * type, and a property can be both (a value `""` and relations of the same type on one entity): a relation row, known
 * by its statement being one of this entity's Postgres relations, says nothing about the value, so it does not mark
 * the property held. Left in, it made the backfill skip the value for good (a Postgres text "" with no geo-db row).
 */
export function heldOf(geo: {prop: string; statement: string}[], relations: PgRelationRow[]): Held {
	const rel = new Set(relations.map((r) => cid("statement", compactId(r.id))))
	return {
		props: new Set(geo.filter((r) => !rel.has(r.statement)).map((r) => r.prop)),
		statements: new Set(geo.map((r) => r.statement)),
	}
}

export type EntityOps = {ops: GrcOp[]; values: number; relations: number; skipped: number; excluded: number}

/**
 * The ops that bring one entity in one space up to the Postgres rows given, never touching a property or
 * relation geo-db already holds:
 *
 * - entity unknown to the replica (`held` null): `createEntity` with every value, then every relation.
 * - entity known: `updateEntity` setting only properties geo-db has no row for, and only relations whose
 *   statement id geo-db does not hold. What geo-db holds came from the live stream, which owns it.
 */
export function entityOps(
	entityId: string,
	values: PgValueRow[],
	relations: PgRelationRow[],
	held: Held | null,
	exclude: ReadonlySet<string>,
): EntityOps {
	const out: EntityOps = {ops: [], values: 0, relations: 0, skipped: 0, excluded: 0}
	const set: GrcPropertyValue[] = []
	for (const row of values) {
		const prop = compactId(row.property_id)
		if (exclude.has(prop)) {
			out.excluded++
			continue
		}
		if (held?.props.has(cid("entity", prop))) continue
		const value = grcValue(row)
		if (!value) {
			out.skipped++
			continue
		}
		set.push({property: prop, value})
	}
	const id = compactId(entityId)
	if (!held) out.ops.push({type: "createEntity", id, values: set})
	else if (set.length > 0) out.ops.push({type: "updateEntity", id, set})
	out.values = set.length
	for (const r of relations) {
		if (exclude.has(compactId(r.type_id))) {
			out.excluded++
			continue
		}
		if (held?.statements.has(cid("statement", compactId(r.id)))) continue
		out.ops.push({
			type: "createRelation",
			id: compactId(r.id),
			from: compactId(r.from_entity_id),
			to: compactId(r.to_entity_id),
			relationType: compactId(r.type_id),
			...(r.position != null ? {position: r.position} : {}),
		})
		out.relations++
	}
	return out
}

/**
 * Written by other indexers straight into Postgres, never carried by an edit (so the feeder never sees
 * them, and a backfilled copy would never be updated): vote-indexer's Score, the ranking-indexer's rank
 * position value and relation type, its Aggregated rankings relation type, and the vote ordinal and weighted values.
 */
export const DERIVED_PROPERTIES = [
	"85a4668a-42fa-4f48-8969-c0a9de0c294b", // vote-indexer Score
	"e1ffac9f-beb4-4654-8a50-00e1d6c662fb", // ranking-indexer rank position (value)
	"890deffb-3843-49fa-8269-74000e3dcef6", // ranking-indexer rank position (relation type)
	"67651da6-11a9-4246-9ff4-039aafbe9e43", // ranking-indexer Aggregated rankings (relation type)
	"49ee1b89-1820-4e75-a1ae-38a2dcaad4a5", // vote ordinal
	"103701dd-cabe-4a8e-835b-10345327b647", // vote weighted
].map(compactId)
