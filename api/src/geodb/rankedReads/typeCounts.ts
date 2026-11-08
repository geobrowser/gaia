/**
 * The plan for serving `public.topic_feed_type_counts` from geo-db's DerivedScan count mode (collection 1), and the way back
 * from the counts geo-db answers to the rows the SQL returns. Pure; the shadow (`../typeCountsShadow.ts`) runs it.
 *
 * Wire frame revisions: 13 is one topic and no Space filter; 14 adds several topics (`match_all` or a pooled union, page marker
 * 0xFFFF); 15 adds a graph block carrying the Space filter. The plan picks the form the call needs and `form` says which, so a
 * caller can refuse a form the serve is below (a serve answers a form it does not know with `InvalidWirePayload`).
 *
 * What the SQL does beyond the topics stays on this side:
 *  - the types are made distinct, and `debate_tagged_claims` walks Debate where Claim was asked, so two requested types can share
 *    one walked type: each requested type is given the count of the type it walks (the SQL groups by the requested type);
 *  - `entity_feed_blocklist` goes over as the blocked list (geo-db takes 256 at most; more is not served);
 *  - a type with no entities is absent from the SQL's answer, so a zero count is dropped here;
 *  - a Space filter rides as the graph block: the Spaces (geo-db ids, 1 to 64), the Name property, and the Tags rule. Name and
 *    Tags are independent per-Space tests (migration 0098): an entity needs a non-blank Name text in a listed Space, and a walked
 *    Debate also needs a Debate-tag relation in a listed Space. The SQL applies the tag test to whatever type it WALKS as Debate
 *    (not only under `debate_tagged_claims`, which is how Claim becomes Debate), so the rule is sent whenever Debate is walked.
 *    More than 64 Spaces is a fallback: Postgres stays the read.
 *
 * Counts are per row of the group, which equals the SQL's distinct entity count: a topic holds an entity once per type. With
 * several topics geo-db counts distinct entities itself (the pooled form) or the entities every topic holds (`match_all`).
 */

import {MAX_COUNT_BLOCKED, MAX_COUNT_SPACES, MAX_COUNT_TOPICS, MAX_COUNT_TYPES} from "../derived"
import {CLAIM_TYPE, DEBATE_TAG, DEBATE_WALK_TYPE} from "./feeds"

/** The Name property, whose non-blank text in a listed Space is the Space test (`0098`: `a126ca53-...`). */
export const NAME_PROPERTY = "a126ca53-0c8e-48d5-b888-82c734c38935"
/** The frame revision each form needs. */
export const REVISION_PLAIN = 13
export const REVISION_TOPICS = 14
export const REVISION_GRAPH = 15

export type TypeCountsCall = {
	topics: string[]
	types: string[]
	/** The gaia Space ids of the filter, or null for none. */
	spaces: string[] | null
	matchAll: boolean
	debateTaggedClaims: boolean
}

/** The graph block's content, every id 32 hex digits. */
export type CountGraph = {
	spaces: string[]
	nameProperty: string
	tagged: {kind: string; relation: string} | null
}

export type TypeCountsPlan = {
	/** The first topic, as 32 hex digits: the request's group. */
	group: string
	/** The other distinct topics, 32 hex digits (empty for one topic). */
	others: string[]
	/** Whether an entity must hold every topic. Only set with several topics, where it means something. */
	matchAll: boolean
	/** The distinct types to walk, as 32 hex digits, in first-asked order. */
	walked: string[]
	/** Each distinct requested type (lowercase uuid text) and the index into `walked` it is counted from. */
	requested: {type: string; walked: number}[]
	/** The Space filter as geo-db takes it, or null. */
	graph: CountGraph | null
	/** The wire frame revision the form needs: 13, 14 or 15. */
	form: number
}

/** A call geo-db could serve but not in one request: Postgres stays the read, and the shadow says so. */
export type TypeCountsFallback = {fallback: "too_many_spaces"}

export const isFallback = (plan: TypeCountsPlan | TypeCountsFallback | null): plan is TypeCountsFallback =>
	plan !== null && "fallback" in plan

const hex = (uuid: string) => uuid.replace(/-/g, "").toLowerCase()
const uuidOfHex = (h: string) =>
	`${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`

/**
 * The plan for a call, null when count mode cannot serve it (the SQL stays the read), or a fallback. `geoSpaces` are the call's
 * Spaces as geo-db knows them (gaia ids are mapped by the caller); a Space filter without them is not served.
 */
export function planTypeCounts(
	call: TypeCountsCall,
	blockedCount: number,
	geoSpaces: string[] | null = null,
): TypeCountsPlan | TypeCountsFallback | null {
	if (blockedCount > MAX_COUNT_BLOCKED) return null
	const topics = [...new Set(call.topics.map(hex))]
	if (topics.length < 1 || topics.length > MAX_COUNT_TOPICS) return null
	let spaces: string[] | null = null
	if (call.spaces !== null) {
		if (geoSpaces === null) return null
		spaces = [...new Set(geoSpaces.map(hex))]
		if (spaces.length > MAX_COUNT_SPACES || call.spaces.length > MAX_COUNT_SPACES)
			return {fallback: "too_many_spaces"}
		// An empty filter is a filter nothing passes, which the wire cannot say (1 to 64 Spaces): the SQL stays the read.
		if (spaces.length === 0) return null
	}
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
	const graph: CountGraph | null =
		spaces === null
			? null
			: {
					spaces,
					nameProperty: hex(NAME_PROPERTY),
					tagged: walked.includes(hex(DEBATE_WALK_TYPE))
						? {kind: hex(DEBATE_WALK_TYPE), relation: hex(DEBATE_TAG)}
						: null,
				}
	const [group, ...others] = topics as [string, ...string[]]
	return {
		group,
		others,
		matchAll: topics.length > 1 && call.matchAll,
		walked,
		requested,
		graph,
		form: graph ? REVISION_GRAPH : topics.length > 1 ? REVISION_TOPICS : REVISION_PLAIN,
	}
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
