/**
 * The "ready to execute" decision for one proposal, computed from geo-db records. It mirrors DETECTION_SQL in
 * `../detect.ts` clause for clause, and answers the same question over the records instead of `proposals_current`,
 * `space_editor_counts` and the tally columns:
 *
 *   preconditions   not executed, not marked unexecutable, Slow, created_at <= now
 *   deadline        execute_by set: now <= execute_by; unset: now - created_at < MAX_PROPOSAL_AGE
 *   slow-late       now > end_time + CLOCK_SKEW_BUFFER, yes+no+abstain >= quorum, (BASE - partial) * yes > partial * no
 *   slow-early      now <= end_time, universal > 0, editors > 0, yes >= ceil(universal * editors / BASE)
 *
 * Reads are lazy: the votes (the expensive read) are taken only for a proposal that reaches a branch needing them, and
 * the editor count only on the early branch. A proposal the records cannot answer for (no version, an ambiguous value,
 * a Fast version over a Slow row, more votes than one read returns) is `unsupported` with a reason, never guessed.
 */
import {RATIO_BASE} from "../contracts.js"
import {CLOCK_SKEW_BUFFER, MAX_PROPOSAL_AGE} from "../detect.js"
import {type GeoDbTwinReader, MAX_BACKLINKS} from "./client.js"
import {entityRef} from "./recordKey.js"
import {
	AmbiguousRecordError,
	type CardRow,
	decodeProposalStatus,
	decodeProposalVersion,
	decodeProposalVote,
	EDITOR_PROPERTY,
	PROPOSAL_VERSION_COLUMNS,
	PROPOSAL_VOTE_COLUMNS,
	proposalStatusRecordId,
} from "./records.js"

export type UnsupportedReason =
	| "space_not_loaded"
	| "system_space_missing"
	| "version_missing"
	| "escalation_pending"
	| "too_many_votes"
	| "ambiguous_record"

export type Verdict = {kind: "decided"; ready: boolean; why: string} | {kind: "unsupported"; reason: UnsupportedReason}

/** Where the records live: the geo-db Space of each system family, and of the proposal's own Space. */
export type Spaces = {
	governance: string | null
	votes: string | null
	/** geo-db Space for a gaia Space id (with or without dashes); null when not loaded. */
	of(gaiaSpaceId: string): string | null
}

const BASE = BigInt(RATIO_BASE)

const unsupported = (reason: UnsupportedReason): Verdict => ({kind: "unsupported", reason})
const decided = (ready: boolean, why: string): Verdict => ({kind: "decided", ready, why})

/** Distinct editor references on a Space's own entity card. */
export function countEditors(rows: CardRow[]): number {
	return new Set(rows.filter((r) => r.prop === EDITOR_PROPERTY).map((r) => r.raw)).size
}

/** yes, no and abstain counts of the votes cast on one version, one vote per (voter) record. */
async function tallyVotes(
	reader: GeoDbTwinReader,
	votesSpace: string,
	proposalId: string,
	ordinal: bigint,
): Promise<{yes: bigint; no: bigint; abstain: bigint} | "too_many"> {
	const links = await reader.readBacklinks(votesSpace, entityRef(proposalId), MAX_BACKLINKS)
	if (links.length >= MAX_BACKLINKS) return "too_many"
	const records = [...new Set(links.filter((l) => l.prop === PROPOSAL_VOTE_COLUMNS.proposal).map((l) => l.from))]
	const cards = await reader.readCards(votesSpace, records)
	let yes = 0n
	let no = 0n
	let abstain = 0n
	for (const id of records) {
		const vote = decodeProposalVote(cards.get(id) ?? [])
		if (vote === null || vote.version !== ordinal) continue
		if (vote.option === 1) yes++
		else if (vote.option === 2) no++
		else abstain++
	}
	return {yes, no, abstain}
}

export async function evaluateProposal(
	reader: GeoDbTwinReader,
	spaces: Spaces,
	proposal: {id: string; spaceId: string},
	nowSeconds: number,
): Promise<Verdict> {
	if (!spaces.governance) return unsupported("system_space_missing")
	const now = BigInt(nowSeconds)
	try {
		// Status first: an executed or unexecutable proposal needs nothing else.
		const status = decodeProposalStatus(
			await reader.readCard(spaces.governance, proposalStatusRecordId(proposal.id)),
		)
		if (status.executedAt !== null) return decided(false, "executed")
		if (status.unexecutableAt !== null) return decided(false, "unexecutable")

		// Versions: the latest block carries the settings in force; its rank among the blocks is the ordinal the votes name.
		const links = await reader.readBacklinks(spaces.governance, entityRef(proposal.id), MAX_BACKLINKS)
		const versionIds = [
			...new Set(links.filter((l) => l.prop === PROPOSAL_VERSION_COLUMNS.proposal).map((l) => l.from)),
		]
		const cards = await reader.readCards(spaces.governance, versionIds)
		const versions = versionIds
			.map((id) => decodeProposalVersion(cards.get(id) ?? []))
			.filter((v): v is NonNullable<typeof v> => v !== null)
			.sort((a, b) => (a.block < b.block ? -1 : a.block > b.block ? 1 : 0))
		const latest = versions.at(-1)
		if (!latest) return unsupported("version_missing")
		// The candidate is a Slow row in Postgres. A Fast version here is the fast-to-slow escalation geo-db does not
		// resolve in this twin yet (an escalation record would move it), so it cannot be decided from the versions.
		if (latest.votingMode !== "Slow") return unsupported("escalation_pending")
		const ordinal = BigInt(versions.length)
		const createdAt = versions[0]?.createdAt ?? latest.createdAt

		if (createdAt > now) return decided(false, "created_in_future")
		if (latest.executeBy !== null) {
			if (now > latest.executeBy) return decided(false, "past_execute_by")
		} else if (now - createdAt >= BigInt(MAX_PROPOSAL_AGE)) return decided(false, "past_max_age")

		const late = now > latest.endTime + BigInt(CLOCK_SKEW_BUFFER)
		const early = now <= latest.endTime
		if (!late && !early) return decided(false, "clock_skew_window")

		let editors = 0
		if (early) {
			if (latest.universal <= 0n) return decided(false, "early_disabled")
			const space = spaces.of(proposal.spaceId)
			if (!space) return unsupported("space_not_loaded")
			editors = countEditors(await reader.readCard(space, entityRef(proposal.spaceId)))
			if (editors === 0) return decided(false, "no_editors")
		}

		if (!spaces.votes) return unsupported("system_space_missing")
		const tally = await tallyVotes(reader, spaces.votes, proposal.id, ordinal)
		if (tally === "too_many") return unsupported("too_many_votes")
		const {yes, no, abstain} = tally

		if (late) {
			if (yes + no + abstain < latest.quorum) return decided(false, "quorum_not_met")
			return (BASE - latest.partial) * yes > latest.partial * no
				? decided(true, "late_passed")
				: decided(false, "late_threshold_not_met")
		}
		// ceil(universal * editors / BASE) on integers
		const need = (latest.universal * BigInt(editors) + BASE - 1n) / BASE
		return yes >= need ? decided(true, "early_passed") : decided(false, "early_threshold_not_met")
	} catch (error) {
		if (error instanceof AmbiguousRecordError) return unsupported("ambiguous_record")
		throw error
	}
}
