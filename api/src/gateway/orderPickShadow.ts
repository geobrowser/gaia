/**
 * The ordered page of the light `entities` shadow: `entities(orderBy: CREATED_AT_DESC, first: n, typeId: ..)`.
 *
 * The card checks (lightShadow.ts) judge each returned entity on its own and leave the page's order uncompared
 * (`order_completeness`). With `GEODB_ORDER_PICK_SHADOW` on, a page that has this shape is also read from geo-db and
 * compared with what PostGraphile returned:
 * - the order is one key, `CREATED_AT_*` or `UPDATED_AT_*`, alone (a list adds `ID_ASC`, whose order is gaia's
 *   identities, not geo-db's, so such a page stays uncompared);
 * - one type claim and no other filter clause or system-type argument (`exactFilter`), no `offset`, a `first`;
 * - no Space argument: PostGraphile orders by the entity's time across every Space, so geo-db reads every loaded
 *   Space (`readEveryPage`), the entity placed by its earliest createdAt (`.pick = .least`) or latest updatedAt
 *   (`.pick = .greatest`), whatever the direction. A page scoped to a Space stays uncompared: its entities'
 *   times may come from other Spaces, which that read would not see.
 *
 * `.ties = .entity` makes geo-db's order deterministic, not equal to Postgres's: geo-db orders equal keys by its own
 * identity (a conformed id, not the gaia id). So what is compared depends on the ties at the page:
 * - ties at the cut (the last entity of a full page shares its key with the next one): which of the tied entities
 *   fall inside the page is each store's own choice, so nothing of the page is compared. Counted `order_ties`, and
 *   `order_completeness` stays;
 * - ties inside the page, away from the cut: its members are settled (every tied entity is in), their order is not.
 *   The page is compared as a set, counted `order_ties`, and `order_completeness` is dropped (membership was compared);
 * - no ties: the page is compared as a sequence, and `order_completeness` is dropped.
 *
 * One more limit stays: geo-db's createdAt and updatedAt are the Space-by-Space times the feeder wrote and a subset of
 * Postgres's until the history is backfilled, so a mismatch can be a missing time, not a wrong order.
 */

import {CREATED_AT_PROPERTY, conformingId, instantNanos, UPDATED_AT_PROPERTY} from "../geodb/card"
import {GeoDbError} from "../geodb/client"
import {observeGeoDbRead} from "../geodb/metrics"
import {type EntityShadowDeps, MAX_EVERY_SPACES} from "./entityShadow"
import type {EntitiesOrder, LightEntitiesPlan} from "./lightShadow"
import type {UncomparedReason, UnsupportedReason} from "./metrics"

export type OrderPageCheck =
	| {result: "skip"}
	| {result: "failed"}
	| {result: "unsupported"; reason: UnsupportedReason}
	| {
			result: "match" | "mismatch"
			uncompared: UncomparedReason[]
			pageCompared: boolean
			detail: Record<string, unknown>
	  }

/** Whether the page's plan has the shape the ordered read compares. */
export function orderedPageShape(
	plan: LightEntitiesPlan,
): {order: EntitiesOrder; typeId: string; first: number} | null {
	const {order, first} = plan
	if (!order || first === null || first < 1 || first > 99) return null
	if (plan.offset !== null && plan.offset !== 0) return null
	if (plan.spaceIds !== null || plan.ids !== null) return null
	if (!plan.exactFilter) return null
	const claim = plan.claims[0]
	if (plan.claims.length !== 1 || claim?.kind !== "type" || claim.typeIds.length !== 1) return null
	return {order, typeId: claim.typeIds[0] as string, first}
}

/**
 * Reads the page from geo-db and compares it with `pgIds` (PostGraphile's entity ids, gaia's, in the order it
 * returned them). Never throws.
 */
export async function checkOrderedPage(
	deps: EntityShadowDeps,
	plan: LightEntitiesPlan,
	pgIds: string[],
	label: string,
): Promise<OrderPageCheck> {
	const shape = orderedPageShape(plan)
	const readEntitiesPage = deps.geoDb.readEntitiesPage?.bind(deps.geoDb)
	if (!shape || deps.orderPick !== true || !readEntitiesPage || pgIds.length === 0) return {result: "skip"}
	if (deps.multiSpace !== true || !deps.allGeoSpaces) return {result: "unsupported", reason: "no_space"}
	let spaces: string[]
	try {
		spaces = await deps.allGeoSpaces()
	} catch (error) {
		deps.log.warn(`${label} space map unreadable`, {message: String(error)})
		return {result: "failed"}
	}
	if (spaces.length === 0) return {result: "unsupported", reason: "no_space"}
	if (spaces.length > MAX_EVERY_SPACES) return {result: "unsupported", reason: "many_spaces"}
	if (deps.everySpaceBudget && !deps.everySpaceBudget.tryAcquire(1))
		return {result: "unsupported", reason: "throttled"}
	const {order, first} = shape
	const field = order.property === "createdAt" ? CREATED_AT_PROPERTY : UPDATED_AT_PROPERTY
	const pick = order.property === "createdAt" ? "least" : "greatest"
	const started = performance.now()
	let rows: Awaited<ReturnType<typeof readEntitiesPage>>
	try {
		// One more than the page, so the entity just past the cut shows whether the cut splits a tie.
		rows = await readEntitiesPage({
			geoSpaceIds: spaces,
			typeId: conformingId("entity", shape.typeId),
			field,
			direction: order.direction,
			pick,
			first: first + 1,
		})
		observeGeoDbRead((performance.now() - started) / 1000, "ok")
	} catch (error) {
		observeGeoDbRead((performance.now() - started) / 1000, "error")
		deps.log.warn(`${label} ordered page read failed`, {
			reason: error instanceof GeoDbError ? error.reason : "error",
			message: String(error),
		})
		return {result: "failed"}
	}
	// The page's entities in order, each with its key: the pick's extreme of its rows of the order's property.
	const keys = new Map<string, bigint | null>()
	for (const row of rows) {
		if (!keys.has(row.entity)) keys.set(row.entity, null)
		if (row.prop !== field) continue
		const nanos = instantNanos(row.raw)
		if (nanos === null) continue
		const held = keys.get(row.entity) ?? null
		keys.set(row.entity, held === null || (pick === "least" ? nanos < held : nanos > held) ? nanos : held)
	}
	const entries = [...keys]
	const geoPage = entries.slice(0, first)
	const geoIds = geoPage.map(([id]) => id)
	const wanted = pgIds.map((id) => conformingId("entity", id))

	const next = entries[first]
	const last = geoPage[first - 1]
	if (pgIds.length === first && last && next && last[1] === next[1])
		return {
			result: "match",
			uncompared: ["order_ties"],
			pageCompared: false,
			detail: {orderedPage: "tie_at_cut"},
		}
	// Equal keys, or entities with no key at all (which sort last, in identity order), inside the page.
	const tied = geoPage.some(([, key], i) => i > 0 && key === (geoPage[i - 1] as [string, bigint | null])[1])
	const asSet = tied
	const same = asSet
		? wanted.length === geoIds.length &&
			new Set(wanted).size === wanted.length &&
			wanted.every((id) => geoIds.includes(id))
		: wanted.length === geoIds.length && wanted.every((id, i) => id === geoIds[i])
	return {
		result: same ? "match" : "mismatch",
		uncompared: asSet ? ["order_ties"] : [],
		pageCompared: true,
		detail: {
			orderedPage: asSet ? "set" : "sequence",
			postgres: pgIds.length,
			geoDb: geoIds.length,
			...(same ? {} : {first: firstDifference(wanted, geoIds, asSet)}),
		},
	}
}

function firstDifference(wanted: string[], got: string[], asSet: boolean): Record<string, unknown> {
	if (asSet) {
		const have = new Set(got)
		const want = new Set(wanted)
		return {
			onlyPostgres: wanted.filter((id) => !have.has(id)).slice(0, 3),
			onlyGeoDb: got.filter((id) => !want.has(id)).slice(0, 3),
		}
	}
	const at = wanted.findIndex((id, i) => id !== got[i])
	return {index: at < 0 ? Math.min(wanted.length, got.length) : at, postgres: wanted[at], geoDb: got[at]}
}
