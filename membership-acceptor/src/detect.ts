/**
 * Membership-request detection.
 *
 * The acceptor receives the full notification firehose. This module decides
 * which deliveries are *membership requests* worth acting on, and de-duplicates
 * the copies the notification fan-out produces.
 *
 * Detection is intentionally a cheap, payload-only check — it is a trigger, not
 * a source of truth. The authoritative "is this still open / untouched" check
 * happens on-chain in M3 before any vote is cast, because webhook payloads can
 * be stale or racy.
 */

/** A delivery that matches the membership-request signature. */
export interface MembershipRequest {
	/** Proposal UUID (the dedupe key). */
	proposalId: string
	/** DAO space UUID the request is to join. */
	spaceId: string
	/**
	 * Personal-space id of the user requesting membership — i.e. the space being
	 * added to `spaceId`. Taken from the single `add_member` action's
	 * `target_address`, which (despite the name) carries the requester's space id
	 * as a hex-encoded bytes16. Passed through verbatim; M3
	 * normalizes it as needed for the on-chain check.
	 */
	requesterSpaceId: string
}

// Membership-request signature (see WEBHOOK_INTEGRATION.md / notification-indexer models).
const MEMBERSHIP_EVENT_TYPE = "proposal_created"
const FAST_VOTING_MODE = "fast"
const ADD_MEMBER_ACTION = "add_member"

function asRecord(value: unknown): Record<string, unknown> | null {
	return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null
}

/**
 * Return a {@link MembershipRequest} if `payload` is a fast-mode, single
 * `add_member` `proposal_created` event that still looks open, else `null`.
 *
 * The "still open" check is best-effort from `settings.end_date`; the real
 * voting-window / untouched check is on-chain in M3.
 */
export function detectMembershipRequest(
	payload: unknown,
	nowSeconds: number = Math.floor(Date.now() / 1000),
): MembershipRequest | null {
	const p = asRecord(payload)
	if (!p) return null

	if (p.event_type !== MEMBERSHIP_EVENT_TYPE) return null
	if (p.voting_mode !== FAST_VOTING_MODE) return null

	// Exactly one action, and it must be add_member.
	if (!Array.isArray(p.actions) || p.actions.length !== 1) return null
	const action = asRecord(p.actions[0])
	if (!action || action.type !== ADD_MEMBER_ACTION) return null

	const proposalId = typeof p.proposal_id === "string" ? p.proposal_id : null
	const spaceId = typeof p.space_id === "string" ? p.space_id : null
	if (!proposalId || !spaceId) return null

	// Best-effort still-open check: if the payload advertises an end_date already
	// in the past, the voting window is closed — skip. Authoritative check is M3.
	const settings = asRecord(p.settings)
	const endDate = settings?.end_date
	if (typeof endDate === "number" && endDate <= nowSeconds) return null

	const requesterSpaceId = typeof action.target_address === "string" ? action.target_address : ""
	return {proposalId, spaceId, requesterSpaceId}
}

/**
 * Bounded de-duplication set keyed by proposal id.
 *
 * The notification fan-out delivers one copy of `proposal_created` per editor of
 * the space (distinct `idempotency_key`s, differing `user_space_id`), so the same
 * proposal arrives N times. We must dedupe on `proposal_id`, NOT `idempotency_key`.
 *
 * This is in-memory and best-effort: it is reset on restart, which only means a
 * proposal could be re-detected (and, in M3, re-checked on-chain — where the
 * untouched/idempotency guard makes a redundant vote a no-op). It is not a
 * durability mechanism.
 */
export class SeenProposals {
	private readonly seenIds = new Set<string>()
	private readonly insertionOrder: string[] = []

	constructor(private readonly capacity = 10_000) {}

	/**
	 * Record `proposalId` and report whether it had already been seen.
	 * Returns `true` if this is a duplicate, `false` if it is the first sighting.
	 */
	seen(proposalId: string): boolean {
		if (this.seenIds.has(proposalId)) return true

		this.seenIds.add(proposalId)
		this.insertionOrder.push(proposalId)
		if (this.insertionOrder.length > this.capacity) {
			const evicted = this.insertionOrder.shift()
			if (evicted !== undefined) this.seenIds.delete(evicted)
		}
		return false
	}

	/**
	 * Forget `proposalId` so it can be processed again.
	 *
	 * Used to roll back an optimistic `seen()` after an infrastructure failure:
	 * we mark before voting (so the concurrent fan-out copies dedupe), but if the
	 * vote fails for a retryable reason we must un-mark, or the delivery-worker's
	 * retry would be silently deduped and the vote never lands. (We leave the id
	 * recorded for benign reverts and successes — those should not retry.)
	 */
	unmark(proposalId: string): void {
		if (!this.seenIds.delete(proposalId)) return
		const idx = this.insertionOrder.indexOf(proposalId)
		if (idx !== -1) this.insertionOrder.splice(idx, 1)
	}
}
