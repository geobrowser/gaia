/**
 * The order of an `entitiesOrderedByProperty` page, held to geo-db's own values.
 *
 * The card checks (lightShadow.ts) judge each returned entity on its own and leave the page's order uncompared
 * (`order_completeness`). For a page that names its Spaces and its data type (and no `includeWithoutValue`), the first
 * entities of the page are read from geo-db in one batch (`readCards`, the Spaces named) and every neighbouring pair is
 * checked against the sort the SQL function `entities_ordered_by_property` applies:
 *
 * - an entity's key is its least (ascending) or greatest (descending) value of the property among its rows in those
 *   Spaces; with `typeIds`, only the rows of a Space where the entity has one of the Types count (the function's
 *   `type_pred`: the TYPES relation in the same Space as the value);
 * - entities are ordered by key in the direction asked, and entities with equal keys by entity id ascending
 *   (`ORDER BY sub.sort_value <dir>, e.id`). The id is gaia's uuid, compared as the normalized text (the same order as
 *   Postgres's byte order of a uuid), so a tie is broken exactly as the function breaks it.
 *
 * A pair is judged only when both keys are known and comparable. What cannot be placed is counted, not guessed:
 * - text keys: Postgres sorts `left(text, 1024)` under the database's collation, which geo-db does not know, so two text
 *   keys are compared only for equality (equal text ties exactly; different text says nothing about the collation's order);
 * - an entity geo-db holds no value of the property for (the feeder has not written it) or, for text, several different
 *   values (which one Postgres picks is the collation's choice).
 * A page in which every pair was judged leaves `order_completeness`; one with pairs left over is `order_partial`.
 * `datetime` and `date` are compared by instant and by calendar day: Postgres sorts the stored text, which agrees for the
 * canonical forms the indexer writes.
 *
 * It reads at most MAX_ORDER_ENTITIES entities of the page (a prefix: the pairs inside it are still a contiguous slice of
 * the order), under the shadow's `budget`, and never throws.
 */

import {conformingId, instantNanos} from "../geodb/card"
import {GeoDbError, MAX_CARD_IDS, MAX_LINE_SPACES} from "../geodb/client"
import {observeGeoDbRead} from "../geodb/metrics"
import type {EntityShadowDeps} from "./entityShadow"
import type {PropertyOrder} from "./lightShadow"
import type {UncomparedReason} from "./metrics"

/** Most entities of a page whose keys are read (one batch card read). */
export const MAX_ORDER_ENTITIES = MAX_CARD_IDS

/** The TYPES relation: links an entity to its type entity (`types_rel_id` in the SQL function). */
const TYPES_RELATION = conformingId("entity", "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1")

export type PropertyOrderCheck =
	| {result: "skip"}
	| {result: "failed"}
	| {
			result: "match" | "mismatch"
			/** Every neighbouring pair of the page was judged. */
			complete: boolean
			uncompared: UncomparedReason[]
			detail: Record<string, unknown>
	  }

/** A sort key of the property's type, as geo-db prints it; null when the type is not one this check orders. */
type Key =
	| {kind: "num"; raw: string}
	| {kind: "bool"; value: boolean}
	| {kind: "when"; nanos: bigint}
	| {kind: "day"; day: [number, number, number]}
	| {kind: "text"; text: string}

const DECIMAL = /^-?\d+(\.\d+)?$/
const DAY = /^date:(-?\d+)-(\d{2})-(\d{2})$/

/** The data types whose keys this check can order. */
export const ORDERED_DATA_TYPES = new Set(["integer", "float", "decimal", "boolean", "datetime", "date", "text"])

/** A geo-db value row of the property as a key of `dataType`, or null when it is not a usable value of that type (the function's `null_pred`). */
export function keyOfRow(dataType: string, raw: string): Key | null {
	switch (dataType) {
		case "integer":
		case "float":
		case "decimal":
			return DECIMAL.test(raw) || (/^[-+0-9.eE]+$/.test(raw) && Number.isFinite(Number(raw)))
				? {kind: "num", raw}
				: null
		case "boolean":
			return raw === "true" ? {kind: "bool", value: true} : raw === "false" ? {kind: "bool", value: false} : null
		case "datetime": {
			const nanos = instantNanos(raw)
			return nanos === null ? null : {kind: "when", nanos}
		}
		case "date": {
			const m = DAY.exec(raw)
			return m ? {kind: "day", day: [Number(m[1]), Number(m[2]), Number(m[3])]} : null
		}
		case "text":
			// `null_pred`: a text of blanks is no value. The sort key is `left(text, 1024)`.
			return raw.trim().length > 0 ? {kind: "text", text: [...raw].slice(0, 1024).join("")} : null
		default:
			return null
	}
}

/** Compares two decimal strings (or, failing that, the numbers). */
function compareNumbers(a: string, b: string): number {
	if (DECIMAL.test(a) && DECIMAL.test(b)) {
		const negA = a.startsWith("-")
		const negB = b.startsWith("-")
		const [intA = "", fracA = ""] = (negA ? a.slice(1) : a).split(".")
		const [intB = "", fracB = ""] = (negB ? b.slice(1) : b).split(".")
		const zeroA = /^0*$/.test(intA) && /^0*$/.test(fracA)
		const zeroB = /^0*$/.test(intB) && /^0*$/.test(fracB)
		const signA = zeroA ? 0 : negA ? -1 : 1
		const signB = zeroB ? 0 : negB ? -1 : 1
		if (signA !== signB) return signA < signB ? -1 : 1
		if (signA === 0) return 0
		const iA = intA.replace(/^0+/, "")
		const iB = intB.replace(/^0+/, "")
		const width = Math.max(fracA.length, fracB.length)
		let magnitude = iA.length !== iB.length ? (iA.length < iB.length ? -1 : 1) : iA < iB ? -1 : iA > iB ? 1 : 0
		if (magnitude === 0) {
			const fA = fracA.padEnd(width, "0")
			const fB = fracB.padEnd(width, "0")
			magnitude = fA < fB ? -1 : fA > fB ? 1 : 0
		}
		return magnitude * signA
	}
	const x = Number(a)
	const y = Number(b)
	return x < y ? -1 : x > y ? 1 : 0
}

/** -1, 0 or 1 by key order; null when the two cannot be ordered here (different text). */
export function compareKeys(a: Key, b: Key): -1 | 0 | 1 | null {
	if (a.kind === "num" && b.kind === "num") return Math.sign(compareNumbers(a.raw, b.raw)) as -1 | 0 | 1
	if (a.kind === "bool" && b.kind === "bool") return a.value === b.value ? 0 : a.value ? 1 : -1
	if (a.kind === "when" && b.kind === "when") return a.nanos === b.nanos ? 0 : a.nanos < b.nanos ? -1 : 1
	if (a.kind === "day" && b.kind === "day") {
		for (let i = 0; i < 3; i++) {
			const x = a.day[i] as number
			const y = b.day[i] as number
			if (x !== y) return x < y ? -1 : 1
		}
		return 0
	}
	if (a.kind === "text" && b.kind === "text") return a.text === b.text ? 0 : null
	return null
}

/** The entity's sort key among its rows of the property: the least (asc) or greatest (desc), null when none or (text) not decidable. */
function pickKey(keys: Key[], direction: "asc" | "desc"): Key | null {
	const first = keys[0]
	if (!first) return null
	if (first.kind === "text") return keys.every((k) => k.kind === "text" && k.text === first.text) ? first : null
	let best: Key = first
	for (const key of keys.slice(1)) {
		const c = compareKeys(key, best)
		if (c !== null && (direction === "asc" ? c < 0 : c > 0)) best = key
	}
	return best
}

export type PairVerdict = {
	checked: number
	undecided: number
	firstBreak: {index: number; reason: "key" | "tie"} | null
}

/**
 * Judges a page's neighbouring pairs. `ids` are the entities in the order PostGraphile returned them (normalized
 * uuids), `keys` their keys (null: not known). A pair is broken when the earlier entity sorts after the later by key in
 * `direction`, or when the keys are equal and the earlier id is not smaller.
 */
export function judgePairs(ids: string[], keys: (Key | null)[], direction: "asc" | "desc"): PairVerdict {
	let checked = 0
	let undecided = 0
	let firstBreak: PairVerdict["firstBreak"] = null
	for (let i = 0; i + 1 < ids.length; i++) {
		const a = keys[i] ?? null
		const b = keys[i + 1] ?? null
		const c = a && b ? compareKeys(a, b) : null
		if (c === null) {
			undecided++
			continue
		}
		checked++
		const idA = ids[i] as string
		const idB = ids[i + 1] as string
		const inOrder = c === 0 ? idA < idB : direction === "asc" ? c < 0 : c > 0
		if (!inOrder && !firstBreak) firstBreak = {index: i, reason: c === 0 ? "tie" : "key"}
	}
	return {checked, undecided, firstBreak}
}

/** Reads the first entities of the page from geo-db and holds their order to the function's. Never throws. */
export async function checkPropertyOrder(
	deps: EntityShadowDeps,
	order: PropertyOrder,
	pageIds: string[],
	label: string,
): Promise<PropertyOrderCheck> {
	const readCards = deps.geoDb.readCards?.bind(deps.geoDb)
	const ids = pageIds.slice(0, MAX_ORDER_ENTITIES)
	if (!readCards || ids.length < 2 || !ORDERED_DATA_TYPES.has(order.dataType)) return {result: "skip"}
	if (order.spaceIds.length > MAX_LINE_SPACES) return {result: "skip"}
	if (deps.budget && !deps.budget.tryAcquire(1)) return {result: "skip"}
	const started = performance.now()
	let rows: Awaited<ReturnType<typeof readCards>>
	try {
		const geoSpaceIds: string[] = []
		for (const space of order.spaceIds) {
			const geo = await deps.geoSpaceOf(space)
			// A Space geo-db has not loaded holds none of the page's rows: the order cannot be placed.
			if (!geo) return {result: "skip"}
			geoSpaceIds.push(geo)
		}
		rows = await readCards({geoSpaceIds, entityIds: ids.map((id) => conformingId("entity", id))})
		observeGeoDbRead((performance.now() - started) / 1000, "ok")
		// One Space's read carries no per-row Space; with several, a row without one cannot be placed.
		if (geoSpaceIds.length > 1 && rows.some((row) => !row.space)) return {result: "skip"}
		return judgePage(order, ids, rows, geoSpaceIds.length === 1 ? (geoSpaceIds[0] as string) : null)
	} catch (error) {
		observeGeoDbRead((performance.now() - started) / 1000, "error")
		try {
			deps.log.warn(`${label} property order read failed`, {
				reason: error instanceof GeoDbError ? error.reason : "error",
				error: String(error),
			})
		} catch {}
		return {result: "failed"}
	}
}

/** The rows of a batch card read as the page's keys, judged. Pure. */
export function judgePage(
	order: PropertyOrder,
	ids: string[],
	rows: {entity?: string; prop: string; raw: string; space?: string}[],
	onlySpace: string | null,
): PropertyOrderCheck {
	const property = conformingId("entity", order.propertyId)
	const types = order.typeIds ? new Set(order.typeIds.map((id) => conformingId("entity", id))) : null
	// With `typeIds`, a value counts only in a Space where the entity has one of the Types.
	const typed = new Set<string>()
	if (types)
		for (const row of rows)
			if (row.prop === TYPES_RELATION && row.entity && types.has(row.raw))
				typed.add(`${row.entity}\t${row.space ?? onlySpace ?? ""}`)
	const keysOf = new Map<string, Key[]>()
	for (const row of rows) {
		if (row.prop !== property || !row.entity) continue
		if (types && !typed.has(`${row.entity}\t${row.space ?? onlySpace ?? ""}`)) continue
		const key = keyOfRow(order.dataType, row.raw)
		if (!key) continue
		const held = keysOf.get(row.entity)
		if (held) held.push(key)
		else keysOf.set(row.entity, [key])
	}
	const keys = ids.map((id) => {
		const held = keysOf.get(conformingId("entity", id))
		return held ? pickKey(held, order.direction) : null
	})
	const verdict = judgePairs(ids, keys, order.direction)
	// Nothing could be placed: the page says nothing either way.
	if (verdict.checked === 0) return {result: "skip"}
	const complete = verdict.undecided === 0
	const detail = {
		entities: ids.length,
		pairsChecked: verdict.checked,
		pairsUndecided: verdict.undecided,
		dataType: order.dataType,
		...(verdict.firstBreak ? {firstBreak: verdict.firstBreak} : {}),
	}
	return {
		result: verdict.firstBreak ? "mismatch" : "match",
		complete,
		uncompared: complete ? [] : ["order_partial"],
		detail,
	}
}
