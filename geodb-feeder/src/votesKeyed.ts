/**
 * User votes as natural-key records (geo-db plan 10.12 K1-K5; plan 10.11 GAP-7, the `votes` family), behind
 * GEODB_VOTES, which takes effect only with GEODB_RECORD_KEYS (votesKeyedEnabled). With either off nothing here runs.
 *
 * Source: `<env>.curation.votes` (HermesVoteCast, hermes-schema/proto/voting.proto), the topic vote-indexer folds
 * into gaia's `user_votes`. One record per `user_votes` primary key, in the `votes` system Space (systemSpaces.ts):
 * - record entity: `recordEntityId(userVote, [id(user), id(object), int(object_type), id(space), int(kind)])`, the
 *   gaia ids as Postgres holds them (raw 16 bytes), so the record is a pure function of the row's key;
 * - key columns (RECORD_COLUMNS.userVote): `add` + `set_active(true)` under
 *   `recordStatementId(userVote, [record, property, target])`; references to `cid("entity", ...)` for the voter and
 *   the Space, to the object's geo-db id (`cid("entity", id)`, or `cid("statement", id)` for a relation, which geo-db
 *   holds as a statement), numbers for object type and kind. K5: no target needs to exist in this Space.
 * - mutable columns: `put` of `vote_type` (0 up, 1 down, 2 removed) and `voted_at` (instant), each under
 *   `recordStatementId(userVote, [record, property, int(value)])`. A put closes the cell's other active statements,
 *   so a changed vote replaces the old value, and including the value in the id makes A -> B -> A end on A and a
 *   replayed event a no-op.
 *
 * A removal (direction NONE) is written as vote_type 2 and keeps the record, as vote-indexer keeps the row. Every
 * event's intents are fixed by the event alone: no read before write, no journal, and a replay from offset zero, a
 * rerun backfill or a redelivered message converge on the last event per key. Kafka keys the topic by object id and
 * every `user_votes` key includes it, so one key's events arrive in order on one partition.
 *
 * Rejected exactly where vote-indexer rejects (dead-lettered, never written): undecodable payload, missing metadata,
 * an id that is not 16 bytes, an object type other than entity/relation, an unknown direction, Interested + Down.
 * An unknown kind keeps its raw value as its own key, as vote-indexer does. A negative raw kind has no u64 key part
 * and is dead-lettered (the producer has never sent one).
 */
import {type ApplyDeps, applyIntents} from "./apply.js"
import {parseFields} from "./hermes.js"
import {cid} from "./ids.js"
import {putIntent} from "./translate.js"
import type {KeyedOffsets} from "./membershipKeyed.js"
import {idPart, intPart, recordEntityId, recordStatementId} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {ensureSystemSpace, type SystemSpaceDeps} from "./systemSpaces.js"

const KIND = RECORD_KINDS.userVote.id
const FAMILY = RECORD_KINDS.userVote.family
const COL = RECORD_COLUMNS.userVote

export type VoteObjectType = 0 | 1
export type VoteDirection = 0 | 1 | 2
export const VOTE_KIND_INTERESTED = 3

/** One `user_votes` row as an event sets it. Ids are 32 lowercase hex, as gaia holds them. */
export type UserVote = {
	user: string
	object: string
	objectType: VoteObjectType
	space: string
	kind: number
	direction: VoteDirection
	votedAtSeconds: bigint
}

export type VoteOutcome = "applied" | "ignored" | "dead_letter"

const hex16 = (v: bigint | Uint8Array | undefined, what: string): string => {
	if (!(v instanceof Uint8Array) || v.length !== 16) throw new Error(`${what}: not 16 bytes`)
	return Buffer.from(v).toString("hex")
}

/** Decode a HermesVoteCast and apply vote-indexer's validation. Throws on what vote-indexer rejects. */
export function parseVoteCast(b: Uint8Array): UserVote {
	const f = new Map<number, bigint | Uint8Array>()
	for (const [n, v] of parseFields(b)) f.set(n, v)
	const metaBytes = f.get(8)
	if (!(metaBytes instanceof Uint8Array)) throw new Error("vote: missing meta")
	let createdAt = 0n
	for (const [n, v] of parseFields(metaBytes)) if (n === 1 && typeof v === "bigint") createdAt = v
	const ot = f.get(2)
	if (!(ot instanceof Uint8Array) || ot.length < 4) throw new Error("vote: invalid object type")
	const otWord = Buffer.from(ot.subarray(0, 4)).readUInt32BE(0)
	if (otWord !== 0 && otWord !== 1) throw new Error(`vote: invalid object type ${Buffer.from(ot).toString("hex")}`)
	const dirRaw = f.get(4) ?? 0n
	if (typeof dirRaw !== "bigint" || dirRaw > 2n) throw new Error(`vote: invalid direction ${String(dirRaw)}`)
	// proto3 int32 enum: a negative value arrives as a 64-bit varint; vote-indexer narrows `vote.kind as i16`.
	const kindRaw = f.get(9) ?? 0n
	if (typeof kindRaw !== "bigint") throw new Error("vote: invalid kind")
	const kind = Number(BigInt.asIntN(16, BigInt.asIntN(32, kindRaw)))
	if (kind < 0) throw new Error(`vote: negative kind ${kind} has no record key`)
	const direction = Number(dirRaw) as VoteDirection
	if (kind === VOTE_KIND_INTERESTED && direction === 1) throw new Error("vote: Interested has no Down")
	return {
		user: hex16(f.get(1), "vote: voter_id"),
		object: hex16(f.get(3), "vote: object_id"),
		objectType: otWord as VoteObjectType,
		space: hex16(f.get(7), "vote: space_pov"),
		kind,
		direction,
		votedAtSeconds: createdAt,
	}
}

/** The record entity of one `user_votes` key. */
export function userVoteRecordId(v: Pick<UserVote, "user" | "object" | "objectType" | "space" | "kind">): string {
	return recordEntityId(KIND, [idPart(v.user), idPart(v.object), intPart(v.objectType), idPart(v.space), intPart(v.kind)])
}

/** The geo-db id the object column references: entities are entities, relations are statements in geo-db. */
export const voteObjectRef = (object: string, objectType: VoteObjectType): string =>
	cid(objectType === 1 ? "statement" : "entity", object)

const valueStatementId = (record: string, property: string, value: bigint | number) =>
	recordStatementId(KIND, [idPart(record), idPart(property), intPart(value)])
const refStatementId = (record: string, property: string, target: string) =>
	recordStatementId(KIND, [idPart(record), idPart(property), idPart(target)])

const keyColumn = (id: string, record: string, property: string, value: string): string[] => [
	`.{ .add = .{ .id = "${id}", .entity = "${record}", .property = "${property}", .value = ${value} } }`,
	`.{ .set_active = .{ .statement = "${id}", .active = true } }`,
]

/** The intents for one vote. Pure, and depends on the vote alone. */
export function userVoteIntents(v: UserVote): string[] {
	const record = userVoteRecordId(v)
	const refs: [string, string][] = [
		[COL.user, cid("entity", v.user)],
		[COL.object, voteObjectRef(v.object, v.objectType)],
		[COL.space, cid("entity", v.space)],
	]
	const nums: [string, number][] = [
		[COL.objectType, v.objectType],
		[COL.kind, v.kind],
	]
	return [
		`.{ .create_entity = "${record}" }`,
		...refs.flatMap(([p, t]) => keyColumn(refStatementId(record, p, t), record, p, `.{ .reference = "${t}" }`)),
		...nums.flatMap(([p, n]) => keyColumn(valueStatementId(record, p, n), record, p, `.{ .number = ${n} }`)),
		putIntent({
			id: valueStatementId(record, COL.direction, v.direction),
			entity: record,
			property: COL.direction,
			value: `.{ .number = ${v.direction} }`,
		}),
		putIntent({
			id: valueStatementId(record, COL.votedAt, v.votedAtSeconds),
			entity: record,
			property: COL.votedAt,
			value: `.{ .instant = "${v.votedAtSeconds * 1_000_000n}" }`,
		}),
	]
}

export type KeyedVotesDeps = ApplyDeps & SystemSpaceDeps & {offsets: KeyedOffsets}

export async function applyUserVote(
	deps: KeyedVotesDeps,
	v: UserVote,
	meta: {partition: number; offset: string},
): Promise<VoteOutcome> {
	// Every vote lives in the one votes Space: a founding failure, ResourceExhausted included (SystemSpaceCapError),
	// is thrown, never dead-lettered. The consumer retries the message without marking its offset; the backfill Job
	// fails. Skipping would drop the whole family, not one vote.
	const space = await ensureSystemSpace(deps, FAMILY)
	const intents = userVoteIntents(v)
	const applyDeps: ApplyDeps = {...deps, saveSpace: () => deps.systemSpaces.save()}
	const r = await applyIntents(applyDeps, space, {...meta, space: v.space}, intents, intents.length, 0, () => {})
	return r.deadLettered > 0 ? "dead_letter" : "applied"
}

export async function handleVoteMessage(
	deps: KeyedVotesDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<VoteOutcome> {
	if (!message.value) return "ignored"
	const header = message.headers?.["event-type"]
	const eventType = header == null ? undefined : Buffer.from(header as Uint8Array | string).toString("utf8")
	if (eventType !== undefined && eventType !== "VOTE_CAST") return "ignored"
	let v: UserVote
	try {
		v = parseVoteCast(message.value)
	} catch (e) {
		deps.store.deadLetter({partition, offset: message.offset, topic: "votes", decode: String(e instanceof Error ? e.message : e)})
		deps.metrics.deadLetters.inc({kind: "message"})
		return "dead_letter"
	}
	return applyUserVote(deps, v, {partition, offset: message.offset})
}

/** Whether the keyed votes writer runs: GEODB_VOTES only takes effect with GEODB_RECORD_KEYS (serve can take `put`). */
export const votesKeyedEnabled = (c: {recordKeys: boolean; votes: {enabled: boolean}}): boolean => c.votes.enabled && c.recordKeys

/**
 * The votes consumer's offsets directory under DATA_DIR; its `offsets.json` exists once the consumer has run. The
 * backfill gates on it with membershipKeyed's keyedBackfillGate: a snapshot row is the key's state when it was read,
 * so once the consumer has applied events a snapshot could put back a vote the stream has since changed. With no
 * offsets the consumer replays the retained topic from earliest, in order, after the backfill; keys older than the
 * window exist only in the snapshot (7 days on testnet), which is why the backfill runs before GEODB_VOTES goes on.
 */
export const votesOffsetsDir = (dataDir: string) => `${dataDir}/feeder/votes-keyed`

/** A `user_votes` row as the backfill reads it (pgSource.readUserVoteRows). */
export type UserVoteRow = {
	user_id: string
	object_id: string
	object_type: number
	space_id: string
	vote_kind: number
	vote_type: number
	voted_at_seconds: string
}

const compact = (id: string) => id.replaceAll("-", "").toLowerCase()

/** A Postgres row as the event that set it. Throws on a value no event could have written. */
export function userVoteFromRow(r: UserVoteRow): UserVote {
	if (r.object_type !== 0 && r.object_type !== 1) throw new Error(`user_votes: object_type ${r.object_type}`)
	if (r.vote_type !== 0 && r.vote_type !== 1 && r.vote_type !== 2) throw new Error(`user_votes: vote_type ${r.vote_type}`)
	if (!Number.isInteger(r.vote_kind) || r.vote_kind < 0) throw new Error(`user_votes: vote_kind ${r.vote_kind}`)
	return {
		user: compact(r.user_id),
		object: compact(r.object_id),
		objectType: r.object_type,
		space: compact(r.space_id),
		kind: r.vote_kind,
		direction: r.vote_type,
		votedAtSeconds: BigInt(r.voted_at_seconds),
	}
}

export type VotesBackfillResult = {rows: number; applied: number; deadLetter: number}

/**
 * Write a snapshot of `user_votes`. Rerunnable: a held row is written again to the same ids. A row Postgres holds
 * that no event could have written is dead-lettered (counted, the Job fails at the end) rather than invented.
 */
export async function backfillUserVotes(
	deps: KeyedVotesDeps,
	rows: AsyncIterable<UserVoteRow> | Iterable<UserVoteRow>,
	progress: (done: number) => void = () => {},
): Promise<VotesBackfillResult> {
	const out: VotesBackfillResult = {rows: 0, applied: 0, deadLetter: 0}
	for await (const r of rows) {
		out.rows++
		let v: UserVote
		try {
			v = userVoteFromRow(r)
		} catch (e) {
			deps.store.deadLetter({partition: -1, offset: "backfill", topic: "votes", decode: String(e instanceof Error ? e.message : e)})
			deps.metrics.deadLetters.inc({kind: "message"})
			out.deadLetter++
			continue
		}
		const o = await applyUserVote(deps, v, {partition: -1, offset: "backfill"})
		if (o === "dead_letter") out.deadLetter++
		else out.applied++
		progress(out.rows)
		if (deps.signal?.aborted) throw new Error("votes backfill aborted")
	}
	return out
}
