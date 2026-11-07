/**
 * geo-db twins of the two `user_votes` reads pair-fit and the entity filter make (GEODB_USER_VOTES_TWIN, off by default).
 *
 * `readSharedClaims` (pairFit/queries.ts) joins the user's stances with each candidate's; the `votedBy` argument of
 * `entities` (kg/entityVotedByFilterPlugin.ts) keeps the entities a user voted on. Both read `public.user_votes` and have no
 * geo-db path. The feeder writes each row as a keyed record in the `votes` system Space (geodb-feeder/src/votesKeyed.ts), a
 * record whose `user` column refers to the user's entity. So one user's votes are the Space's backlinks of that entity
 * (`readBacklinks`, the rows whose value refers to it) and one batch card read of the records they name (`readCards`).
 *
 * Both twins decode a vote's columns, then apply the SQL's rule:
 *  - shared claims: stances only (kind 1, a claim, Agree or Disagree; a removal, direction 2, is no position), each user's
 *    LATEST stance per claim across Spaces (the filter runs before the latest is picked, so a later removal does not hide an
 *    earlier stance), then the claims user and candidate both hold and whether they are on opposite sides.
 *  - votedBy: the objects (claims, `object_type` 0: a relation vote's object is no entity) the user holds a vote on, narrowed by
 *    kind and direction the way the filter's `votedByKinds` / `votedByTypes` are.
 *
 * Ids: geo-db holds the entity ref of a gaia id (`entityRef`: a v4 id as itself, any other remapped), so claims come out in
 * ref form and a caller comparing with Postgres maps its ids the same way.
 *
 * Not twinned: the claims' topics (`readSharedClaims` also returns each claim's Topics targets, read from `relations`); the
 * topic read needs a relation card read of every shared claim in every Space it lives in, which is a separate slice.
 */

import type {GeoCardRow} from "../geodb/card"
import {type GeoDbCardClient, GeoDbError, MAX_BACKLINKS} from "../geodb/client"
import {USER_VOTE_COLUMNS} from "../geodb/keyedRecordsShadow"
import {entityRef} from "../proposals/governance/recordKey"
import {AmbiguousRecordError, instantSeconds, wholeNumber} from "../proposals/governance/records"

/** One vote record, decoded. Refs are 32 hex. */
export type GeoVote = {
	user: string
	/** Null for a relation vote (`objectType` 1): geo-db holds a relation as a statement, not a ref. */
	object: string | null
	objectType: number
	space: string
	kind: number
	/** `user_votes.vote_type`: 0 agree/verify/up, 1 disagree/dispute/down, 2 removed. */
	direction: number
	votedAt: bigint
}

/** A vote record's card as a vote, or null when it is not one (a column missing). Throws AmbiguousRecordError on two values. */
export function decodeVoteRecord(rows: GeoCardRow[]): GeoVote | null {
	const C = USER_VOTE_COLUMNS
	const one = (prop: string): string | null => {
		const values = new Set(rows.filter((r) => r.prop === prop).map((r) => r.raw))
		if (values.size > 1) throw new AmbiguousRecordError(`column ${prop} holds ${values.size} values`)
		return [...values][0] ?? null
	}
	const user = one(C.user)
	const space = one(C.space)
	const objectType = one(C.objectType)
	const kind = one(C.kind)
	const direction = one(C.direction)
	const votedAt = one(C.votedAt)
	if (
		user === null ||
		space === null ||
		objectType === null ||
		kind === null ||
		direction === null ||
		votedAt === null
	)
		return null
	return {
		user,
		object: one(C.object),
		objectType: Number(wholeNumber(objectType)),
		space,
		kind: Number(wholeNumber(kind)),
		direction: Number(wholeNumber(direction)),
		votedAt: instantSeconds(votedAt),
	}
}

export type UserVotes = {
	votes: GeoVote[]
	/** The backlink read filled its limit, so the user may hold more votes than were read. A caller must not compare. */
	truncated: boolean
}

export type VotesReader = Pick<GeoDbCardClient, "readBacklinks" | "readCards">

/**
 * Every vote record the `votes` Space holds for `userId` (a gaia id, dashed or not). At most `limit` backlinks are read: when
 * that many come back the result is `truncated`. Throws `GeoDbError` if the store refuses and AmbiguousRecordError on a
 * record that holds two values for a column.
 */
export async function readUserVotes(
	geoDb: VotesReader,
	votesSpace: string,
	userId: string,
	limit: number = MAX_BACKLINKS,
): Promise<UserVotes> {
	if (!geoDb.readBacklinks || !geoDb.readCards)
		throw new GeoDbError("client_failed", "the geo-db client cannot read backlinks or batch cards")
	const backlinks = await geoDb.readBacklinks({geoSpaceIds: [votesSpace], entityId: entityRef(userId), limit})
	const records = [...new Set(backlinks.filter((b) => b.prop === USER_VOTE_COLUMNS.user).map((b) => b.from))]
	if (records.length === 0) return {votes: [], truncated: backlinks.length >= limit}
	const rows = await geoDb.readCards({geoSpaceIds: [votesSpace], entityIds: records})
	const byRecord = new Map<string, GeoCardRow[]>()
	for (const row of rows) {
		if (row.entity === undefined) continue
		const list = byRecord.get(row.entity) ?? []
		list.push(row)
		byRecord.set(row.entity, list)
	}
	const votes: GeoVote[] = []
	for (const id of records) {
		const vote = decodeVoteRecord(byRecord.get(id) ?? [])
		if (vote !== null) votes.push(vote)
	}
	return {votes, truncated: backlinks.length >= limit}
}

/** The user's held stances: claim ref to direction (0 or 1), the latest stance per claim. Ties on time keep the higher direction. Pure. */
export function heldStances(votes: GeoVote[]): Map<string, 0 | 1> {
	const latest = new Map<string, {at: bigint; direction: 0 | 1}>()
	for (const v of votes) {
		if (v.objectType !== 0 || v.kind !== 1 || (v.direction !== 0 && v.direction !== 1) || v.object === null)
			continue
		const held = latest.get(v.object)
		if (!held || v.votedAt > held.at || (v.votedAt === held.at && v.direction > held.direction))
			latest.set(v.object, {at: v.votedAt, direction: v.direction})
	}
	return new Map([...latest].map(([claim, s]) => [claim, s.direction]))
}

export type SharedClaimRef = {claimRef: string; opposite: boolean}

/** The claims `user` and `candidate` both hold a stance on, ascending by claim ref. Pure. */
export function sharedClaimsOf(user: Map<string, 0 | 1>, candidate: Map<string, 0 | 1>): SharedClaimRef[] {
	const shared: SharedClaimRef[] = []
	for (const [claimRef, direction] of user) {
		const theirs = candidate.get(claimRef)
		if (theirs !== undefined) shared.push({claimRef, opposite: theirs !== direction})
	}
	return shared.sort((a, b) => (a.claimRef < b.claimRef ? -1 : a.claimRef > b.claimRef ? 1 : 0))
}

/**
 * The twin of `readSharedClaims`, without topics: for each candidate, the claims both hold and whether they disagree, keyed by
 * the candidate id as passed. A candidate with no shared claim has no entry, as in the SQL. Reads the user's votes once and
 * each candidate's once. Throws what `readUserVotes` throws; `truncated` is true when any user's read hit the limit.
 */
export async function readSharedClaimsGeoDb(
	geoDb: VotesReader,
	votesSpace: string,
	userId: string,
	candidateIds: string[],
	limit: number = MAX_BACKLINKS,
): Promise<{byCandidate: Map<string, SharedClaimRef[]>; truncated: boolean}> {
	const mine = await readUserVotes(geoDb, votesSpace, userId, limit)
	const user = heldStances(mine.votes)
	let truncated = mine.truncated
	const byCandidate = new Map<string, SharedClaimRef[]>()
	for (const candidateId of candidateIds) {
		const theirs = await readUserVotes(geoDb, votesSpace, candidateId, limit)
		if (theirs.truncated) truncated = true
		const shared = sharedClaimsOf(user, heldStances(theirs.votes))
		if (shared.length > 0) byCandidate.set(candidateId, shared)
	}
	return {byCandidate, truncated}
}

/** The `votedBy` narrowing: empty or absent arrays mean any, as in the filter. */
export type VotedBySelection = {kinds?: number[] | null; types?: number[] | null}

/** The entities (claim refs, ascending) of the user's votes that pass the narrowing. Relation votes are not entities. Pure. */
export function votedByRefs(votes: GeoVote[], {kinds, types}: VotedBySelection): string[] {
	const out = new Set<string>()
	for (const v of votes) {
		if (v.objectType !== 0 || v.object === null) continue
		if (kinds && kinds.length > 0 && !kinds.includes(v.kind)) continue
		if (types && types.length > 0 && !types.includes(v.direction)) continue
		out.add(v.object)
	}
	return [...out].sort()
}

/** The twin of the `votedBy` argument: the ids the entities connection is restricted to. Throws like `readUserVotes`. */
export async function readEntityVotedByGeoDb(
	geoDb: VotesReader,
	votesSpace: string,
	userId: string,
	selection: VotedBySelection,
	limit: number = MAX_BACKLINKS,
): Promise<{refs: string[]; truncated: boolean}> {
	const {votes, truncated} = await readUserVotes(geoDb, votesSpace, userId, limit)
	return {refs: votedByRefs(votes, selection), truncated}
}

/** What the SQL served for one candidate against the twin: the same claims with the same sides. Pure. */
export function sameSharedClaims(served: {claimId: string; opposite: boolean}[], twin: SharedClaimRef[]): boolean {
	if (served.length !== twin.length) return false
	const key = (ref: string, opposite: boolean) => `${ref}:${opposite ? 1 : 0}`
	const held = new Set(twin.map((t) => key(t.claimRef, t.opposite)))
	return served.every((s) => held.has(key(entityRef(s.claimId), s.opposite)))
}

/** The ids the SQL served against the twin's refs: the same set. Pure. */
export function sameVotedBy(servedIds: string[], twinRefs: string[]): boolean {
	const served = [...new Set(servedIds.map(entityRef))].sort()
	return served.length === twinRefs.length && served.every((id, i) => id === twinRefs[i])
}
