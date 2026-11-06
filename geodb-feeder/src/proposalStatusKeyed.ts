/**
 * The stored facts behind a proposal's status, as natural-key records (geo-db plan 10.12 K1-K5; plan 10.11 GAP-7, the
 * `governance` family), behind GEODB_PROPOSAL_STATUS, which takes effect only with GEODB_RECORD_KEYS
 * (proposalStatusKeyedEnabled). With either off nothing here runs. A flag of its own: the backfill must run first.
 *
 * WHAT THIS IS NOT. gaia stores no proposal status. `proposals` has no status column: the api computes it on every
 * read (api/src/proposals/status.ts computeProposalStatus; the SQL twins sqlIsProposed, sqlIsExecutable and
 * sqlIsRejected in api/src/proposals/queries.ts) from the votes, the settings snapshot, `executed_at`,
 * `unexecutable_at` and the CLOCK. "Proposed", "executable" and "rejected" change with no event at all (a voting window
 * closes, a grace period lapses), so a status mirrored as a column would be wrong the moment it was written. The
 * transitions are therefore NOT forced into the feeder. What the feeder can mirror are the two stored inputs, and that
 * is all this file does: a read of a proposal's status in geo-db is a function over these records, the votes
 * (proposalVotesKeyed.ts), the settings (spaceVotingSettingsKeyed.ts) and `now`; see RUNBOOK 2c-novies for where that
 * function belongs.
 *
 * The two stored inputs have different writers:
 * - `executed_at`: an indexer event. PROPOSAL_EXECUTED (HermesProposalExecuted, hermes-schema/proto/governance.proto) on
 *   `<env>.space.governance` becomes `UPDATE proposals SET executed_at = $1 WHERE id = $2` (kg-indexer
 *   handlers/governance.rs handle_proposal_executed, storage.rs update_proposal_executed), $1 the block time of the
 *   event (0 when metadata is missing). Never cleared; a second event overwrites. This consumer handles that event.
 * - `unexecutable_at`: NO event. It is written by an operator-run script, proposal-executor/src/backfill-unexecutable.ts
 *   (dry-run by default), after an on-chain `latestProposalVersion == 0` check. There is nothing to consume, so the
 *   consumer cannot carry it: only the backfill reads it (from Postgres), and a value written after the backfill reaches
 *   geo-db only by rerunning the backfill (RUNBOOK 2c-novies). The lasting fix is for whatever sets the column to
 *   publish an event, a change in proposal-executor, not here.
 *
 * One record per proposal, in the `governance` system Space (RECORD_KINDS.proposalStatus is pinned to that family):
 * - record entity: `recordEntityId(proposalStatus, [id(proposal)])`, the raw 16-byte gaia id, the table's key;
 * - key column (RECORD_COLUMNS.proposalStatus.proposal): `add` + `set_active(true)`, a reference to the proposal's entity
 *   (`cid("entity", id)`, as proposal_actions and proposal_votes reference it);
 * - mutable columns, only when the source has the value: `put` of executed_at and unexecutable_at (instants, whole
 *   seconds), each under `recordStatementId(proposalStatus, [record, property, int(value)])`. A put closes the cell's
 *   other active statements, so a second PROPOSAL_EXECUTED replaces the first, as the UPDATE does, and A -> B -> A ends
 *   on A. A column the source lacks is not written, so it is never cleared: nothing in gaia clears either column.
 *
 * Every event's intents are fixed by the event alone: no read before write, no journal. hermes keys the event by its
 * Space, so one proposal's events arrive in order on one partition. Values are `as i64` (the cast kg-indexer makes).
 *
 * Accepted and rejected where kg-indexer accepts and rejects: a `proposal_id` that is not 16 bytes is dead-lettered
 * (Uuid::from_slice errors); missing metadata is written as time 0 (`unwrap_or(0)`). A wire type protobuf would not
 * decode is a decode failure here, never read as a default.
 *
 * Not mirrored, in either direction: the UPDATE matches no row when the proposal is not in Postgres yet, and kg-indexer
 * drops a whole block on a permanent failure, so geo-db can hold an executed_at Postgres lacks (and a proposal created
 * in a dropped block has none of the rest). Treat any difference as expected and investigate it, never as a bug in
 * either (proposalStatusGate.ts reports geo-db-only keys, not as a failure).
 */
import {parseFields} from "./hermes.js"
import {cid} from "./ids.js"
import {
	applyKeyedRecord,
	backfillKeyedRows,
	deadLetterDecode,
	type KeyedBackfillResult,
	type KeyedOutcome,
	type KeyedRecordDeps,
	keyColumn,
} from "./keyedRecord.js"
import {idPart, intPart, recordEntityId, recordStatementId} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {putIntent} from "./translate.js"

const KIND = RECORD_KINDS.proposalStatus.id
const FAMILY = RECORD_KINDS.proposalStatus.family
const COL = RECORD_COLUMNS.proposalStatus

/** The `event-type` header value kg-indexer decodes as HermesProposalExecuted. */
export const PROPOSAL_EXECUTED = "PROPOSAL_EXECUTED"

export type ProposalStatusFacts = {
	/** 32 hex digits. */
	proposal: string
	/** Block time of the execution in whole seconds (i64), or null when the source has none. */
	executedAt: bigint | null
	/** Whole seconds (i64), or null. Only the backfill carries it. */
	unexecutableAt: bigint | null
}

const i64 = (v: bigint) => BigInt.asIntN(64, v)

/** Decode a HermesProposalExecuted with kg-indexer's handle_proposal_executed rules. Throws where it errors. */
export function parseProposalExecuted(b: Uint8Array): ProposalStatusFacts {
	const f = new Map<number, bigint | Uint8Array>()
	for (const [n, v] of parseFields(b)) f.set(n, v)
	const space = f.get(1) ?? new Uint8Array(0)
	if (!(space instanceof Uint8Array) || space.length !== 16) throw new Error("proposal executed: space_id is not 16 bytes")
	const id = f.get(2) ?? new Uint8Array(0)
	if (!(id instanceof Uint8Array) || id.length !== 16) throw new Error("proposal executed: proposal_id is not 16 bytes")
	let createdAt = 0n
	const meta = f.get(3)
	if (meta !== undefined && !(meta instanceof Uint8Array)) throw new Error("proposal executed: invalid metadata wire type")
	if (meta !== undefined) {
		for (const [n, v] of parseFields(meta)) {
			if (n === 1) {
				if (typeof v !== "bigint") throw new Error("proposal executed: invalid metadata field 1 wire type")
				createdAt = v
			}
		}
	}
	return {proposal: Buffer.from(id).toString("hex"), executedAt: i64(createdAt), unexecutableAt: null}
}

/** The record entity of one proposal's status facts. */
export const proposalStatusRecordId = (proposal: string): string => recordEntityId(KIND, [idPart(proposal)])

// A signed value's statement-id key part is its u64 bit pattern (record keys hold unsigned ints).
const valueStatementId = (record: string, property: string, value: bigint) =>
	recordStatementId(KIND, [idPart(record), idPart(property), intPart(BigInt.asUintN(64, value))])

/** The intents for one proposal's facts. Pure, and depends on the facts alone. */
export function proposalStatusIntents(s: ProposalStatusFacts): string[] {
	const record = proposalStatusRecordId(s.proposal)
	const proposal = cid("entity", s.proposal)
	const out = [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(recordStatementId(KIND, [idPart(record), idPart(COL.proposal), idPart(proposal)]), record, COL.proposal, `.{ .reference = "${proposal}" }`),
	]
	const instant = (property: string, v: bigint | null) => {
		if (v !== null) out.push(putIntent({id: valueStatementId(record, property, v), entity: record, property, value: `.{ .instant = "${v * 1_000_000n}" }`}))
	}
	instant(COL.executedAt, s.executedAt)
	instant(COL.unexecutableAt, s.unexecutableAt)
	return out
}

export type KeyedProposalStatusDeps = KeyedRecordDeps

export async function applyProposalStatus(
	deps: KeyedProposalStatusDeps,
	s: ProposalStatusFacts,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> {
	// Every record lives in the one governance Space; a founding failure is thrown, never dead-lettered.
	return applyKeyedRecord(deps, FAMILY, proposalStatusIntents(s), {...meta, space: s.proposal})
}

const headerText = (h: unknown): string | undefined => {
	const v = Array.isArray(h) ? h[0] : h
	if (v === undefined || v === null) return undefined
	return Buffer.isBuffer(v) || v instanceof Uint8Array ? Buffer.from(v).toString("utf8") : String(v)
}

export async function handleProposalStatusMessage(
	deps: KeyedProposalStatusDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<KeyedOutcome> {
	if (headerText(message.headers?.["event-type"]) !== PROPOSAL_EXECUTED) return "ignored"
	if (!message.value) return "ignored"
	let s: ProposalStatusFacts
	try {
		s = parseProposalExecuted(message.value)
	} catch (e) {
		return deadLetterDecode(deps, "space.governance", partition, message.offset, e)
	}
	return applyProposalStatus(deps, s, {partition, offset: message.offset})
}

/** Whether the keyed status writer runs: GEODB_PROPOSAL_STATUS only takes effect with GEODB_RECORD_KEYS. */
export const proposalStatusKeyedEnabled = (c: {recordKeys: boolean; proposalStatus: {enabled: boolean}}): boolean =>
	c.proposalStatus.enabled && c.recordKeys

/** The consumer's offsets directory under DATA_DIR; the backfill gates on it (keyedBackfillGate). */
export const proposalStatusOffsetsDir = (dataDir: string) => `${dataDir}/feeder/proposal-status-keyed`

/** A `proposals` row with `executed_at` or `unexecutable_at` set, as the backfill reads it (pgSource.readProposalStatusRows). */
export type ProposalStatusRow = {
	id: string
	executed_at: string | null
	unexecutable_at: string | null
}

const I64 = /^-?\d+$/
const big = (k: string, v: string | null): bigint | null => {
	if (v === null) return null
	if (!I64.test(v) || BigInt.asIntN(64, BigInt(v)) !== BigInt(v)) throw new Error(`proposals: ${k} ${JSON.stringify(v)}`)
	return BigInt(v)
}

/** A Postgres row as the facts it holds. Throws on a value no writer could have written, or a row with neither. */
export function proposalStatusFromRow(r: ProposalStatusRow): ProposalStatusFacts {
	const executedAt = big("executed_at", r.executed_at)
	const unexecutableAt = big("unexecutable_at", r.unexecutable_at)
	if (executedAt === null && unexecutableAt === null) throw new Error("proposals: neither executed_at nor unexecutable_at is set")
	return {proposal: r.id.replaceAll("-", "").toLowerCase(), executedAt, unexecutableAt}
}

/**
 * Write a snapshot of the proposals that have a status fact. Rerunnable: a held row is written again to the same ids.
 * A row Postgres holds that no writer could have written is dead-lettered (counted, the Job fails at the end).
 */
export const backfillProposalStatus = (
	deps: KeyedProposalStatusDeps,
	rows: AsyncIterable<ProposalStatusRow> | Iterable<ProposalStatusRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{topic: "space.governance", fromRow: proposalStatusFromRow, apply: (d, s) => applyProposalStatus(d, s, {partition: -1, offset: "backfill"})},
		progress,
	)
