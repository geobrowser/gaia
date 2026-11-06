/**
 * Proposal settings escalations as natural-key records (geo-db plan 10.12; the `governance` family), carried by the
 * proposal-versions consumer (proposalVersionsKeyed.ts, GEODB_PROPOSAL_VERSIONS) as its `PROPOSAL_SETTINGS_UPDATED` branch.
 *
 * Source: HermesProposalSettingsUpdated (hermes-schema/proto/governance.proto: space_id 1, proposal_id 2, settings 3, meta 4).
 * kg-indexer's update_proposal_settings rewrites voting_mode, start_time, end_time, quorum, the three thresholds and
 * execute_by (NULL when the event's value is 0) of the proposal's CURRENT version in place, and does nothing when the
 * proposal has no version. The event does not say which version, and finding out is a read.
 *
 * So the consumer does not rewrite a version. Each event is one IMMUTABLE record, keyed (proposal, block of the event)
 * (RECORD_KINDS.proposalEscalation), holding the settings the event set; its intents are fixed by the event alone, so a
 * replay is a no-op and the backfill (which reads Postgres, where the escalated values already sit in the version rows)
 * can never be undone by one. The version an escalation hit is resolved at READ time by `resolveEscalations`: the proposal's
 * version with the greatest block at or before the escalation's block, which is the current version when the event ran
 * (versions are keyed by the block that stamps them). The latest escalation (greatest block, then record id) of a version
 * supplies its settings, replacing all eight (an absent execute_by is "no deadline"). An escalation with no version at or
 * before its block is an orphan: kg-indexer did nothing for it.
 *
 * Same-block ties: two events in one block cannot be ordered here (the key has no log position). An escalation in the
 * block that creates a version is taken to follow it. Two escalations in one block of one proposal resolve by record id,
 * which may differ from Postgres's order; the gate would report that as a value difference.
 *
 * A message with a proposal id that is not 16 bytes or no `settings` is dead-lettered (kg-indexer's map errors). A voting mode
 * other than Fast (0) reads as Slow.
 */
import {parseFields} from "./hermes.js"
import {cid} from "./ids.js"
import {applyKeyedRecord, type KeyedOutcome, type KeyedRecordDeps, keyColumn} from "./keyedRecord.js"
import {i64, id16, lastFields, num} from "./proposalActionsKeyed.js"
import {bytesPart, idPart, intPart, recordEntityId, recordStatementId} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {putIntent, zigString} from "./translate.js"

const KIND = RECORD_KINDS.proposalEscalation.id
const FAMILY = RECORD_KINDS.proposalEscalation.family
const COL = RECORD_COLUMNS.proposalEscalation

export const PROPOSAL_SETTINGS_UPDATED = "PROPOSAL_SETTINGS_UPDATED"

export type ProposalEscalation = {
	proposal: string
	/** The block of the event (u64 bit pattern of the i64). */
	block: bigint
	votingMode: "Fast" | "Slow"
	startTime: bigint
	endTime: bigint
	quorum: bigint
	partial: bigint
	universal: bigint
	flat: bigint
	/** Absent when the event's `execute_by` is 0. */
	executeBy?: bigint
}

/** The escalation one PROPOSAL_SETTINGS_UPDATED message carries. Throws where kg-indexer's mapping errors or protobuf would not decode. */
export function parseProposalEscalation(payload: Uint8Array): ProposalEscalation {
	const f = new Map<number, bigint | Uint8Array>()
	for (const [n, v] of parseFields(payload)) f.set(n, v)
	id16(f.get(1), "space_id")
	const proposal = id16(f.get(2), "proposal_id")
	const settingsBytes = f.get(3)
	if (!(settingsBytes instanceof Uint8Array)) throw new Error("proposal escalation: message has no settings")
	const s = lastFields(settingsBytes)
	let block = 0n
	const meta = f.get(4)
	if (meta !== undefined) {
		if (!(meta instanceof Uint8Array)) throw new Error("proposal escalation: invalid metadata wire type")
		for (const [n, v] of parseFields(meta)) {
			if (n === 3) {
				if (typeof v !== "bigint") throw new Error("proposal escalation: invalid metadata wire type")
				block = v
			}
		}
	}
	const executeBy = num(s.get(14), "execute_by")
	return {
		proposal,
		block: BigInt.asUintN(64, i64(block)),
		votingMode: num(s.get(7), "voting_mode") === 0n ? "Fast" : "Slow",
		startTime: i64(num(s.get(12), "start_date")),
		endTime: i64(num(s.get(13), "last_date")),
		quorum: i64(num(s.get(11), "quorum")),
		partial: i64(num(s.get(8), "partial")),
		universal: i64(num(s.get(9), "universal")),
		flat: i64(num(s.get(10), "flat")),
		...(executeBy !== 0n ? {executeBy: i64(executeBy)} : {}),
	}
}

export const proposalEscalationRecordId = (e: Pick<ProposalEscalation, "proposal" | "block">): string =>
	recordEntityId(KIND, [idPart(e.proposal), intPart(BigInt.asUintN(64, e.block))])

const bits = (v: bigint) => intPart(BigInt.asUintN(64, v))

/** The intents for one escalation. Pure, and depends on the event alone. */
export function proposalEscalationIntents(e: ProposalEscalation): string[] {
	const record = proposalEscalationRecordId(e)
	const proposal = cid("entity", e.proposal)
	const out = [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(recordStatementId(KIND, [idPart(record), idPart(COL.proposal), idPart(proposal)]), record, COL.proposal, `.{ .reference = "${proposal}" }`),
		...keyColumn(recordStatementId(KIND, [idPart(record), idPart(COL.block), bits(e.block)]), record, COL.block, `.{ .number = ${BigInt.asIntN(64, e.block)} }`),
	]
	const put = (property: string, key: ReturnType<typeof intPart>, value: string) =>
		out.push(putIntent({id: recordStatementId(KIND, [idPart(record), idPart(property), key]), entity: record, property, value}))
	put(COL.votingMode, bytesPart(new TextEncoder().encode(e.votingMode)), `.{ .text = ${zigString(e.votingMode)} }`)
	const n = (property: string, x: bigint | undefined) => {
		if (x !== undefined) put(property, bits(x), `.{ .number = ${x} }`)
	}
	n(COL.startTime, e.startTime)
	n(COL.endTime, e.endTime)
	n(COL.quorum, e.quorum)
	n(COL.partial, e.partial)
	n(COL.universal, e.universal)
	n(COL.flat, e.flat)
	n(COL.executeBy, e.executeBy)
	return out
}

export const applyProposalEscalation = (deps: KeyedRecordDeps, e: ProposalEscalation, meta: {partition: number; offset: string}): Promise<KeyedOutcome> =>
	applyKeyedRecord(deps, FAMILY, proposalEscalationIntents(e), {...meta, space: e.proposal})

/**
 * Which escalation (if any) supplies each version's settings, and which escalations hit no version. Pure. Versions and
 * escalations are of ONE proposal, blocks compared as unsigned. `id` breaks a tie between escalations in one block.
 */
export function resolveEscalations<V extends {block: bigint}, E extends {block: bigint; id: string}>(
	versions: V[],
	escalations: E[],
): {applied: Map<V, E>; orphans: E[]} {
	const applied = new Map<V, E>()
	const orphans: E[] = []
	const sorted = [...versions].sort((a, b) => (a.block < b.block ? -1 : a.block > b.block ? 1 : 0))
	const order = [...escalations].sort((a, b) => (a.block < b.block ? -1 : a.block > b.block ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
	for (const e of order) {
		let target: V | undefined
		for (const v of sorted) if (v.block <= e.block) target = v
		if (target === undefined) orphans.push(e)
		else applied.set(target, e)
	}
	return {applied, orphans}
}
