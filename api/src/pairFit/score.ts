/**
 * Pair fit (GEO-3224): how good a debate pairing one user and one candidate would make, from data
 * gaia already holds, with the parts that produced it and one card-readable reason.
 *
 * Pure: the route (`router.ts`) does the reads and passes everything in, so every part of the
 * score is unit-tested without a database. Nothing here is stored: pairs are scored on request.
 *
 * THE SCORE, per (user u, candidate v), every part in [0, 1]:
 *
 *   interest      cosine similarity of u's and v's OWN topic weights (GEO-3088's
 *                 user_topic_weights, own_weight > 0). Own rather than total weight: spreading
 *                 lends a topic weight nobody engaged with, and two people who both merely
 *                 neighbour a topic do not share it. Cosine, not a sum of minimums, so a heavy
 *                 user does not out-match a light one with the same tastes.
 *
 *   relevance(c)  for a claim c both hold a stance on (Agree/Disagree, user_votes vote_kind 1):
 *                   0.5 + 0.5 * sqrt(a_u(c) * a_v(c)),
 *                 where a_x(c) is x's strongest weight on c's topics divided by x's strongest
 *                 weight overall. A claim neither cares about still counts half (most claims
 *                 carry few topic tags); one both care about most counts fully. The geometric
 *                 mean means both have to care.
 *
 *   raw           sum of relevance over claims they hold OPPOSITE positions on, less
 *                 AGREEMENT_PENALTY (1/4) of the sum over claims they agree on, floored at 0.
 *                 Agreeing is mild evidence against a debate, but a pair who agree on some
 *                 things and disagree on others is the best kind of pair (common ground, real
 *                 difference), so agreement only dampens. It never makes a pair worse than one
 *                 with no shared votes at all, because of the floor.
 *
 *   disagreement  accountWeight(v) * raw / (raw + DISAGREEMENT_HALF_SATURATION). One fully
 *                 relevant disagreement gives 0.5, three give 0.75; it saturates so one prolific
 *                 voter cannot run away with it. Multiplied by the CANDIDATE's account weight
 *                 (0101, GEO-3141): a new or one-sided account's votes say less about a real
 *                 disagreement. The user's own weight is not applied: this is their own read.
 *
 *   score         (interest + 2 * disagreement) / 3. Disagreement is what makes a debate, so it
 *                 counts double; interest is what is left when two people share no votes, and
 *                 is what keeps a disagreement on something neither cares about from topping
 *                 the list.
 *
 * A pair with no shared stance votes gets disagreement 0, so an interest-only score, and is never
 * labelled as disagreeing. Excluded (test and junk) candidates are not scored at all.
 *
 * PRIVACY. Per-user positions may be political-opinion data. This returns pair-level aggregates
 * only (how many claims the two share, oppose and agree on, and the one claim or topic of the
 * reason), never which side either person took.
 */

/** Bump on any change to this file's logic. Served as `pair-fit-<this>`. */
export const PAIR_FIT_CODE_VERSION = 1

export const AGREEMENT_PENALTY = 0.25
export const DISAGREEMENT_HALF_SATURATION = 1
export const INTEREST_WEIGHT = 1
export const DISAGREEMENT_WEIGHT = 2

/** One of a person's own topic weights. */
export type OwnTopicWeight = {topicId: string; weight: number}

/** A claim both people hold a stance on: whether they are on opposite sides, and its topics. */
export type SharedClaim = {claimId: string; opposite: boolean; topicIds: string[]}

export type PairFitCandidate = {
	userId: string
	weights: OwnTopicWeight[]
	sharedClaims: SharedClaim[]
	/** account_weights.weight; null when the account has never voted (and so shares no claims). */
	accountWeight: number | null
	excluded: boolean
}

export type PairFitReason =
	| {kind: "disagree"; claimId: string; name: string | null; text: string}
	| {kind: "shared_topic"; topicId: string; name: string | null; text: string}

export type PairFitItem = {
	userId: string
	score: number
	parts: {
		interest: number
		disagreement: number
		/** Claims both hold a stance on, and how many of those they are on opposite sides of. */
		sharedClaims: number
		opposed: number
		agreed: number
		accountWeight: number
	}
	/** True only when they hold opposite positions on at least one claim and that counts. */
	disagreeing: boolean
	reason: PairFitReason | null
}

export type PairFitResult = {
	items: PairFitItem[]
	excluded: {userId: string; reason: "excluded_account"}[]
}

function toMap(weights: OwnTopicWeight[]): Map<string, number> {
	const map = new Map<string, number>()
	for (const w of weights) if (w.weight > 0) map.set(w.topicId, (map.get(w.topicId) ?? 0) + w.weight)
	return map
}

function maxOf(map: Map<string, number>): number {
	let max = 0
	for (const v of map.values()) if (v > max) max = v
	return max
}

export function cosine(a: Map<string, number>, b: Map<string, number>): number {
	let dot = 0
	let na = 0
	let nb = 0
	for (const [k, v] of a) {
		na += v * v
		const w = b.get(k)
		if (w !== undefined) dot += v * w
	}
	for (const v of b.values()) nb += v * v
	if (dot <= 0 || na === 0 || nb === 0) return 0
	return Math.min(1, dot / Math.sqrt(na * nb))
}

/** How much a person cares about a claim, in [0, 1], relative to their strongest topic. */
function affinity(weights: Map<string, number>, max: number, topicIds: string[]): number {
	if (max <= 0) return 0
	let best = 0
	for (const t of topicIds) {
		const w = weights.get(t)
		if (w !== undefined && w > best) best = w
	}
	return best / max
}

export function relevance(aUser: number, aCandidate: number): number {
	return 0.5 + 0.5 * Math.sqrt(Math.max(0, aUser) * Math.max(0, aCandidate))
}

const byIdThen = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0)
const EPSILON = 1e-12

type TopClaim = {claimId: string; r: number}

function beats(a: TopClaim, b: TopClaim): boolean {
	if (Math.abs(a.r - b.r) > EPSILON) return a.r > b.r
	return byIdThen(a.claimId, b.claimId) < 0
}

export function scorePairs(args: {
	user: {userId: string; weights: OwnTopicWeight[]}
	candidates: PairFitCandidate[]
	names: Map<string, string>
}): PairFitResult {
	const mine = toMap(args.user.weights)
	const myMax = maxOf(mine)
	const items: PairFitItem[] = []
	const excluded: PairFitResult["excluded"] = []
	const order = new Map<string, number>()

	for (const [index, candidate] of args.candidates.entries()) {
		if (candidate.userId === args.user.userId) continue
		if (candidate.excluded) {
			excluded.push({userId: candidate.userId, reason: "excluded_account"})
			continue
		}
		order.set(candidate.userId, index)
		const theirs = toMap(candidate.weights)
		const theirMax = maxOf(theirs)
		const interest = cosine(mine, theirs)
		const accountWeight = Math.min(1, Math.max(0, candidate.accountWeight ?? 0))

		let opposedSum = 0
		let agreedSum = 0
		let opposed = 0
		let top: TopClaim | null = null
		for (const claim of candidate.sharedClaims) {
			const r = relevance(affinity(mine, myMax, claim.topicIds), affinity(theirs, theirMax, claim.topicIds))
			if (claim.opposite) {
				opposed++
				opposedSum += r
				// The most relevant disagreement; among equals the lowest id, so the reason is stable
				// across requests. Names play no part, so the route can read them afterwards.
				const next = {claimId: claim.claimId, r}
				if (!top || beats(next, top)) top = next
			} else {
				agreedSum += r
			}
		}
		const raw = Math.max(0, opposedSum - AGREEMENT_PENALTY * agreedSum)
		const disagreement = accountWeight * (raw / (raw + DISAGREEMENT_HALF_SATURATION))
		const disagreeing = opposed > 0 && disagreement > 0
		const score =
			(INTEREST_WEIGHT * interest + DISAGREEMENT_WEIGHT * disagreement) / (INTEREST_WEIGHT + DISAGREEMENT_WEIGHT)

		let reason: PairFitReason | null = null
		if (disagreeing && top) {
			const name = args.names.get(top.claimId) ?? null
			reason = {
				kind: "disagree",
				claimId: top.claimId,
				name,
				text: name
					? `You two disagree on ${name}`
					: `You two disagree on ${opposed === 1 ? "a claim" : `${opposed} claims`}`,
			}
		} else if (interest > 0) {
			// The topic both weigh most, each relative to their own strongest.
			let best: {topicId: string; v: number} | null = null
			for (const [topicId, w] of mine) {
				const other = theirs.get(topicId)
				if (other === undefined) continue
				const v = (w / myMax) * (other / theirMax)
				if (
					!best ||
					v > best.v + EPSILON ||
					(Math.abs(v - best.v) <= EPSILON && byIdThen(topicId, best.topicId) < 0)
				) {
					best = {topicId, v}
				}
			}
			if (best) {
				const name = args.names.get(best.topicId) ?? null
				reason = {
					kind: "shared_topic",
					topicId: best.topicId,
					name,
					text: name ? `You both care about ${name}` : "You care about the same topics",
				}
			}
		}

		items.push({
			userId: candidate.userId,
			score,
			parts: {
				interest,
				disagreement,
				sharedClaims: candidate.sharedClaims.length,
				opposed,
				agreed: candidate.sharedClaims.length - opposed,
				accountWeight,
			},
			disagreeing,
			reason,
		})
	}

	// Best fit first; ties keep the caller's order, which is the surface's own ranking.
	items.sort((a, b) => b.score - a.score || (order.get(a.userId) ?? 0) - (order.get(b.userId) ?? 0))
	return {items, excluded}
}
