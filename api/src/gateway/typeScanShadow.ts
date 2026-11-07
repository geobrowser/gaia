/**
 * The unordered page of the light `entities` shadow: `entities(spaceId: .., typeId: .., first: n, offset: m)`.
 *
 * PostGraphile's `entities` with no `orderBy` returns rows in primary-key (entity id) order. With `GEODB_TYPE_SCAN_SHADOW`
 * on, a page that has this shape is also read from geo-db as a type scan (`.args = .{ .types = .{..}, .first = N }`, geo-db
 * ISS-577-185), which lists the distinct entities of the types in entity-id order, and the page's ids are compared as a
 * sequence:
 * - no `orderBy`, one type claim (`typeId` or `typeIds: {in}`, at most 32 Types) and no other filter clause or system-type
 *   argument (`exactFilter`), no `id: {in}` filter, a `first` of at least 1, and a Space argument (`spaceId`, `spaceIds`);
 * - geo-db has no skip, so it is asked for `first = offset + first` and the first `offset` ids are dropped. A window past
 *   4096 (geo-db's one-row cut) is not asked: it counts `type_scan_window`;
 * - a serve that predates the argument refuses the document (`MalformedRequest`): counted `type_scan_refused`, never a
 *   failed request. A read that fails any other way is `failed`.
 *
 * Limits that stay: geo-db orders by the id it holds, which is the gaia id only for a conforming UUIDv4 (any other legacy id
 * is remapped, see `conformingId`), so a page holding such ids can mismatch on order alone.
 */

import {conformingId} from "../geodb/card"
import {GeoDbError, MAX_LINE_SPACES, MAX_TYPE_SCAN, MAX_TYPE_SCAN_TYPES} from "../geodb/client"
import {observeGeoDbRead} from "../geodb/metrics"
import type {EntityShadowDeps} from "./entityShadow"
import type {LightEntitiesPlan} from "./lightShadow"
import type {UncomparedReason, UnsupportedReason} from "./metrics"

export type TypeScanCheck =
	| {result: "skip"}
	| {result: "failed"}
	| {result: "unsupported"; reason: UnsupportedReason}
	| {result: "match" | "mismatch"; uncompared: UncomparedReason[]; detail: Record<string, unknown>}

/** What a type scan page asks of geo-db: the Spaces and Types (gaia ids), the page's `first` and `offset`. */
export type TypeScanShape = {spaceIds: string[]; typeIds: string[]; first: number; offset: number}

/** Whether the page's plan has the shape the type scan compares. `ordered` is true when the root named an `orderBy`. */
export function typeScanShape(plan: LightEntitiesPlan): TypeScanShape | null {
	const {first} = plan
	if (plan.ordered || first === null || first < 1) return null
	if (plan.offset !== null && plan.offset < 0) return null
	if (plan.spaceIds === null || plan.ids !== null || plan.everySpace) return null
	if (!plan.exactFilter) return null
	const claim = plan.claims[0]
	if (plan.claims.length !== 1 || claim?.kind !== "type") return null
	return {spaceIds: plan.spaceIds, typeIds: claim.typeIds, first, offset: plan.offset ?? 0}
}

/** How many entities geo-db is asked for: it has no skip, so the window's end (`offset + first`). */
export const typeScanAsk = (shape: Pick<TypeScanShape, "first" | "offset">): number => shape.offset + shape.first

/** The page's ids out of geo-db's scan: the first `offset` dropped, `first` kept. */
export const typeScanWindow = (ids: string[], shape: Pick<TypeScanShape, "first" | "offset">): string[] =>
	ids.slice(shape.offset, shape.offset + shape.first)

/** A serve that predates `.args.types` refuses the document; that refusal is the only one counted as unsupported. */
export const isTypeScanRefusal = (error: unknown): boolean =>
	error instanceof GeoDbError && error.reason === "client_failed" && /\bMalformedRequest\b/.test(error.message)

/**
 * Reads the page from geo-db and compares it with `pgIds` (PostGraphile's entity ids, in the order it returned them).
 * Never throws.
 */
export async function checkTypeScan(
	deps: EntityShadowDeps,
	plan: LightEntitiesPlan,
	pgIds: string[],
	label: string,
): Promise<TypeScanCheck> {
	const shape = typeScanShape(plan)
	const readTypeScan = deps.geoDb.readTypeScan?.bind(deps.geoDb)
	if (!shape || deps.typeScan !== true || !readTypeScan || pgIds.length === 0) return {result: "skip"}
	const ask = typeScanAsk(shape)
	if (ask > MAX_TYPE_SCAN) return {result: "unsupported", reason: "type_scan_window"}
	if (shape.typeIds.length > MAX_TYPE_SCAN_TYPES) return {result: "unsupported", reason: "args"}
	if (shape.spaceIds.length > MAX_LINE_SPACES) return {result: "unsupported", reason: "many_spaces"}
	if (shape.spaceIds.length > 1 && deps.multiSpace !== true) return {result: "unsupported", reason: "no_space"}
	const geoSpaceIds: string[] = []
	for (const space of shape.spaceIds) {
		const geo = await deps.geoSpaceOf(space)
		if (!geo) return {result: "unsupported", reason: "unmapped_space"}
		geoSpaceIds.push(geo)
	}
	if (deps.budget && !deps.budget.tryAcquire(1)) return {result: "unsupported", reason: "throttled"}
	const started = performance.now()
	let scanned: string[]
	try {
		scanned = await readTypeScan({
			geoSpaceIds: [...new Set(geoSpaceIds)],
			typeIds: [...new Set(shape.typeIds.map((id) => conformingId("entity", id)))],
			first: ask,
		})
		observeGeoDbRead((performance.now() - started) / 1000, "ok")
	} catch (error) {
		observeGeoDbRead((performance.now() - started) / 1000, "error")
		if (isTypeScanRefusal(error)) return {result: "unsupported", reason: "type_scan_refused"}
		deps.log.warn(`${label} type scan read failed`, {
			reason: error instanceof GeoDbError ? error.reason : "error",
			error: String(error),
		})
		return {result: "failed"}
	}
	const geoIds = typeScanWindow(scanned, shape)
	const wanted = pgIds.map((id) => conformingId("entity", id))
	const same = wanted.length === geoIds.length && wanted.every((id, i) => id === geoIds[i])
	return {
		result: same ? "match" : "mismatch",
		uncompared: [],
		detail: {
			typeScan: "sequence",
			first: shape.first,
			offset: shape.offset,
			postgres: pgIds.length,
			geoDb: geoIds.length,
			...(same ? {} : {at: firstDifference(wanted, geoIds)}),
		},
	}
}

function firstDifference(wanted: string[], got: string[]): Record<string, unknown> {
	const at = wanted.findIndex((id, i) => id !== got[i])
	return {index: at < 0 ? Math.min(wanted.length, got.length) : at, postgres: wanted[at], geoDb: got[at]}
}
