/**
 * `POST /internal/pair-fit` (GEO-3224): how good a debate pairing one user would make with each of
 * up to MAX_PAIR_FIT_CANDIDATES candidates. See score.ts for the formula.
 *
 * PRIVATE, like `/internal/for-you`: it is a route on the same internal router, so it exists only
 * when GAIA_INTERNAL_TOKEN is set (404 otherwise) and every request must carry that token. Answers
 * are `private, no-store`; nothing is cached or stored, and nothing about anyone's positions is
 * logged, only counts.
 *
 * Request:  {userId, candidateIds}
 *   userId        a personal space id (geo-chat's users.profile_space_id; user_votes.user_id)
 *   candidateIds  personal space ids, in the caller's own order (ties keep it)
 *
 * Response: {ranking: {name, version}, items: [{userId, score, parts, disagreeing, reason}],
 *            excluded: [{userId, reason}]}, items best fit first.
 */

import type {Context} from "hono"
import {log} from "../services/telemetry"
import {isValidUuid, normalizeUuid, toDashedUuid} from "../utils/uuid"
import {type Database, readAccounts, readNames, readOwnWeights, readSharedClaims} from "./queries"
import {PAIR_FIT_CODE_VERSION, type PairFitCandidate, type PairFitResult, scorePairs} from "./score"

/** The admin roster is in the tens today; room to grow without letting a caller ask for thousands. */
export const MAX_PAIR_FIT_CANDIDATES = 300

type Body = {userId: string; candidateIds: string[]}

export function parsePairFitBody(raw: unknown): Body | string {
	if (!raw || typeof raw !== "object") return "body must be a JSON object"
	const {userId, candidateIds} = raw as Record<string, unknown>
	if (typeof userId !== "string" || !isValidUuid(userId)) return "userId must be a UUID"
	if (!Array.isArray(candidateIds)) return "candidateIds must be an array"
	if (candidateIds.length > MAX_PAIR_FIT_CANDIDATES) return `at most ${MAX_PAIR_FIT_CANDIDATES} candidateIds`
	const user = normalizeUuid(userId)
	const ids: string[] = []
	const seen = new Set<string>([user])
	for (const id of candidateIds) {
		if (typeof id !== "string" || !isValidUuid(id)) return "every candidateId must be a UUID"
		const n = normalizeUuid(id)
		if (!seen.has(n)) {
			seen.add(n)
			ids.push(n)
		}
	}
	return {userId: user, candidateIds: ids}
}

const dashless = (id: string) => (isValidUuid(id) ? normalizeUuid(id) : id)

function remap<T>(map: Map<string, T>): Map<string, T> {
	return new Map([...map].map(([k, v]) => [dashless(k), v]))
}

export async function computePairFit(db: Database, body: Body): Promise<PairFitResult> {
	if (body.candidateIds.length === 0) return {items: [], excluded: []}
	const dashedUser = toDashedUuid(body.userId)
	const dashedCandidates = body.candidateIds.map(toDashedUuid)
	const [weights, shared, accounts] = await Promise.all([
		readOwnWeights(db, [dashedUser, ...dashedCandidates]).then(remap),
		readSharedClaims(db, dashedUser, dashedCandidates).then(remap),
		readAccounts(db, dashedCandidates).then(remap),
	])
	const candidates: PairFitCandidate[] = body.candidateIds.map((userId) => ({
		userId,
		weights: (weights.get(userId) ?? []).map((w) => ({...w, topicId: dashless(w.topicId)})),
		sharedClaims: (shared.get(userId) ?? []).map((c) => ({
			...c,
			claimId: dashless(c.claimId),
			topicIds: c.topicIds.map(dashless),
		})),
		accountWeight: accounts.get(userId)?.weight ?? null,
		excluded: accounts.get(userId)?.excluded ?? false,
	}))
	const user = {
		userId: body.userId,
		weights: (weights.get(body.userId) ?? []).map((w) => ({...w, topicId: dashless(w.topicId)})),
	}

	// Scored once to find what the reasons name, then again with the names. The choice of reason
	// never depends on names, so both passes pick the same claims and topics.
	const unnamed = scorePairs({user, candidates, names: new Map()})
	const reasonIds = [
		...new Set(
			unnamed.items.flatMap((i) =>
				i.reason ? [i.reason.kind === "disagree" ? i.reason.claimId : i.reason.topicId] : [],
			),
		),
	]
	const names = remap(await readNames(db, reasonIds.map(toDashedUuid)))
	return scorePairs({user, candidates, names})
}

export function pairFitHandler(db: Database) {
	return async (c: Context) => {
		let raw: unknown
		try {
			raw = await c.req.json()
		} catch {
			return c.json({error: "invalid JSON"}, 400, {"cache-control": "no-store"})
		}
		const body = parsePairFitBody(raw)
		if (typeof body === "string") return c.json({error: body}, 400, {"cache-control": "no-store"})

		const started = performance.now()
		try {
			const result = await computePairFit(db, body)
			// Counts only: never who, and never anyone's side.
			log.info("pair-fit scored", {
				candidates: body.candidateIds.length,
				disagreeing: result.items.filter((i) => i.disagreeing).length,
				excluded: result.excluded.length,
				durationMs: Math.round(performance.now() - started),
			})
			return c.json(
				{
					ranking: {name: "pair-fit", version: `pair-fit-${PAIR_FIT_CODE_VERSION}`},
					items: result.items,
					excluded: result.excluded,
				},
				200,
				{"cache-control": "private, no-store"},
			)
		} catch (error) {
			log.error("pair-fit failed", {error: String(error)})
			return c.json({error: "pair_fit_unavailable"}, 500, {"cache-control": "no-store"})
		}
	}
}
