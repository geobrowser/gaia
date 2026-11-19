/**
 * Which part of a shadowed answer differed, read from a mismatch's detail object. Names only: the result is a
 * fixed set (`MISMATCH_PARTS`), never an id or a free-text value, so it is safe as a metric label.
 */

import {MISMATCH_PARTS, type MismatchPart} from "./metrics"

/** `derivedMismatches` entries (the derived-field checks of entityShadow's compareDerived) and the part each counts as. */
const DERIVED_PARTS: Readonly<Record<string, MismatchPart>> = {
	name: "name",
	description: "description",
	types: "types",
	typesNames: "types_names",
	spaceIds: "space_ids",
	updatedAt: "updated_at",
	createdAt: "created_at",
	allValuesList: "all_values_list",
	propertyNames: "property_names",
	typeNames: "type_names",
	toEntityNames: "to_entity_names",
	fromEntityNames: "from_entity_names",
}

const known = new Set<string>(MISMATCH_PARTS)
const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v)
/** A count, or a list of samples, that is not empty. */
const some = (v: unknown): boolean => (Array.isArray(v) ? v.length > 0 : typeof v === "number" && v > 0)

/**
 * The distinct parts a mismatch detail names, in a stable order; `["other"]` when none. A detail that carries its own
 * `parts` list (an operation judged as several checks) is read from that list; `first` (the first mismatching
 * entity of an operation) is read too.
 */
export function mismatchPartsOf(detail: Record<string, unknown> | undefined): MismatchPart[] {
	const out = new Set<MismatchPart>()
	const read = (d: Record<string, unknown>): void => {
		if (Array.isArray(d.parts)) {
			for (const part of d.parts) if (typeof part === "string" && known.has(part)) out.add(part as MismatchPart)
		} else {
			if (some(d.onlyPostgres) || some(d.missingInGeoDb)) out.add("statements_missing")
			if (some(d.onlyGeoDb) || some(d.geoOnlyRows)) out.add("statements_extra")
			if (Array.isArray(d.derivedMismatches))
				for (const name of d.derivedMismatches) {
					const part = typeof name === "string" ? DERIVED_PARTS[name] : undefined
					if (part) out.add(part)
				}
			if (isRecord(d.nestedMismatches) && Object.keys(d.nestedMismatches).length > 0) out.add("nested_entity")
			if (isRecord(d.propertyInfoMismatches) && Object.keys(d.propertyInfoMismatches).length > 0)
				out.add("property_info")
			if (d.relationsMembership !== undefined) out.add("relations_membership")
			if (some(d.relationFacets) || d.facets !== undefined) out.add("relation_facets")
			if (d.edge !== undefined) out.add("relation_edge")
			if (some(d.unmetFilter)) out.add("unmet_filter")
			if (d.notInSpace === true) out.add("not_in_space")
			if (d.absentInPostgres === true) out.add("absent_in_postgres")
		}
		if (d.filterBreach) out.add("filter_breach")
		if (d.orderedPage !== undefined) out.add("ordered_page")
		if (d.typeScan !== undefined) out.add("type_scan")
		if (d.propertyOrder !== undefined) out.add("property_order")
		if (isRecord(d.first)) read(d.first)
	}
	if (detail) read(detail)
	return out.size === 0 ? ["other"] : [...out]
}
