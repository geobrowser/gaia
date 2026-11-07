/**
 * The plan for serving `public.topic_feed_type_counts` from geo-db's DerivedScan count mode (wire frame revision 13, collection 1),
 * and the way back from the counts geo-db answers to the rows the SQL returns. Pure; the shadow (`../typeCountsShadow.ts`) runs it.
 *
 * Collection 1 is keyed by one topic, so count mode answers ONE topic over a list of types. What the SQL does beyond that stays on
 * this side or is not served:
 *  - one distinct topic, no Space filter: served. With a single topic `match_all` is the same walk either way;
 *  - several distinct topics (`match_all` needs the rarest topic walked and the others required; otherwise a union with a distinct
 *    entity count) or a Space filter (name values in those Spaces, and the debate tag): NOT served, those need the rows;
 *  - the types are made distinct, and `debate_tagged_claims` walks Debate where Claim was asked, so two requested types can share one
 *    walked type: each requested type is given the count of the type it walks (the SQL groups by the requested type);
 *  - `entity_feed_blocklist` goes over as the blocked list (geo-db takes 256 at most; more is not served);
 *  - a type with no entities is absent from the SQL's answer, so a zero count is dropped here.
 *
 * Counts are per row of the group, which equals the SQL's distinct entity count: a topic holds an entity once per type.
 */

import {MAX_COUNT_BLOCKED, MAX_COUNT_TYPES} from "../derived"
import {CLAIM_TYPE, DEBATE_WALK_TYPE} from "./feeds"

export type TypeCountsCall = {
	topics: string[]
	types: string[]
	spaces: string[] | null
	matchAll: boolean
	debateTaggedClaims: boolean
}

export type TypeCountsPlan = {
	/** The topic, as 32 hex digits. */
	group: string
	/** The distinct types to walk, as 32 hex digits, in first-asked order. */
	walked: string[]
	/** Each distinct requested type (lowercase uuid text) and the index into `walked` it is counted from. */
	requested: {type: string; walked: number}[]
}

const hex = (uuid: string) => uuid.replace(/-/g, "").toLowerCase()
const uuidOfHex = (h: string) =>
	`${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`

/** The plan for a call, or null when count mode cannot serve it (the SQL stays the read). */
export function planTypeCounts(call: TypeCountsCall, blockedCount: number): TypeCountsPlan | null {
	if (call.spaces !== null || blockedCount > MAX_COUNT_BLOCKED) return null
	const topics = [...new Set(call.topics.map(hex))]
	if (topics.length !== 1) return null
	const topic = topics[0] as string
	const walked: string[] = []
	const requested: TypeCountsPlan["requested"] = []
	const seen = new Set<string>()
	for (const raw of call.types) {
		const type = hex(raw)
		if (seen.has(type)) continue
		seen.add(type)
		const walk = call.debateTaggedClaims && type === hex(CLAIM_TYPE) ? hex(DEBATE_WALK_TYPE) : type
		let at = walked.indexOf(walk)
		if (at < 0) at = walked.push(walk) - 1
		requested.push({type: uuidOfHex(type), walked: at})
	}
	if (walked.length === 0 || walked.length > MAX_COUNT_TYPES) return null
	return {group: topic, walked, requested}
}

/** The SQL's rows from geo-db's counts: `type_id` (uuid text) to entity count, zero counts omitted. */
export function typeCountsFrom(plan: TypeCountsPlan, counts: number[]): Map<string, number> {
	const out = new Map<string, number>()
	for (const r of plan.requested) {
		const n = counts[r.walked] ?? 0
		if (n > 0) out.set(r.type, n)
	}
	return out
}
