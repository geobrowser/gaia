/**
 * The user-votes twin shadow (GEODB_USER_VOTES_TWIN, off by default).
 *
 * After `/internal/pair-fit` has read its shared claims from Postgres, this reads the same user's and a sample of the
 * candidates' votes from geo-db's `votes` Space (userVotesTwin.ts), applies the SQL's rule and counts whether the claims and
 * sides agree. It also holds the `votedBy` twin to Postgres for the requesting user (positions: kinds 1 and 2, directions 0
 * and 1). Both run in the background, at most one at a time and one per `intervalMs`; the response never waits on it and
 * nothing about it changes, delays or fails a response. Counts only: it never logs who or what anyone voted.
 *
 * `gaia_api_user_votes_twin_total{read,result}`, read = shared_claims (per candidate) or voted_by (per request):
 *   match       the same claims on the same sides (the same ids)
 *   mismatch    they differ
 *   unsupported geo-db could not answer (see `gaia_api_user_votes_twin_unsupported_total{reason}`): no votes Space, a read
 *               that hit the backlink limit, a record that holds two values for a column, a client without the reads, or the
 *               shadow was busy
 *   failed      a read failed
 * `unsupported` is not a mismatch: a user whose votes the feeder has not backfilled shows as `mismatch` (SQL has claims,
 * geo-db none), which is the signal the backfill covers the rows these reads need.
 */

import type {Database} from "../forYou/queries"
import {GeoDbError} from "../geodb/client"
import {AmbiguousRecordError} from "../proposals/governance/records"
import type {SystemSpaceLookup} from "../proposals/governance/shadow"
import {readVotedByIds} from "./queries"
import type {SharedClaim} from "./score"
import {
	readEntityVotedByGeoDb,
	readSharedClaimsGeoDb,
	sameSharedClaims,
	sameVotedBy,
	type VotesReader,
} from "./userVotesTwin"

export type UserVotesShadowConfig = {
	enabled: boolean
	maxInFlight: number
	intervalMs: number
	/** Candidates compared per pass (each is one backlink read and one batch card read). */
	sample: number
}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readUserVotesShadowConfig(env: Record<string, string | undefined>): UserVotesShadowConfig {
	const flag = env.GEODB_USER_VOTES_TWIN?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		maxInFlight: wholeNumber(env.GEODB_USER_VOTES_TWIN_MAX_IN_FLIGHT, 1, 4),
		intervalMs: wholeNumber(env.GEODB_USER_VOTES_TWIN_INTERVAL_MS, 10_000, 3_600_000),
		sample: wholeNumber(env.GEODB_USER_VOTES_TWIN_SAMPLE, 3, 25),
	}
}

export const READS = ["shared_claims", "voted_by"] as const
export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
export const UNSUPPORTED_REASONS = ["no_votes_space", "truncated", "bad_record", "no_reads", "busy"] as const
type Read = (typeof READS)[number]
type Unsupported = (typeof UNSUPPORTED_REASONS)[number]

/** The stance narrowing pair-fit's definition of "a position" uses, and `votedBy`'s `kinds: [1, 2], types: [0, 1]`. */
const POSITIONS = {kinds: [1, 2], types: [0, 1]}

let active = false
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const bump = (map: Map<string, number>, key: string, by = 1): void => {
	map.set(key, (map.get(key) ?? 0) + by)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderUserVotesTwinMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_user_votes_twin_total geo-db twins of the user_votes reads against Postgres, by read (shared_claims per candidate, voted_by per request) and result: match, mismatch, unsupported (geo-db cannot answer) or failed (a geo-db read failed).",
		"# TYPE gaia_api_user_votes_twin_total counter",
	]
	for (const read of READS)
		for (const r of RESULTS)
			lines.push(
				`gaia_api_user_votes_twin_total{read="${read}",result="${r}"} ${results.get(`${read}\t${r}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_user_votes_twin_unsupported_total user_votes twin comparisons geo-db could not answer, by reason.",
		"# TYPE gaia_api_user_votes_twin_unsupported_total counter",
	)
	for (const reason of UNSUPPORTED_REASONS)
		lines.push(`gaia_api_user_votes_twin_unsupported_total{reason="${reason}"} ${unsupported.get(reason) ?? 0}`)
	return lines.join("\n")
}

export function __resetUserVotesTwinMetricsForTests(): void {
	active = false
	results.clear()
	unsupported.clear()
}

export type UserVotesShadow = {
	/**
	 * `userId` and the candidate ids are 32-hex (dashless); `served` is what `readSharedClaims` returned, keyed the same.
	 * Returns at once; the comparison runs in the background.
	 */
	compare(userId: string, candidateIds: string[], served: Map<string, SharedClaim[]>): void
}

export function createUserVotesShadow(deps: {
	config: UserVotesShadowConfig
	db: Database
	geoDb: VotesReader
	systemSpaceOf: SystemSpaceLookup
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): UserVotesShadow {
	const {config, db, geoDb, systemSpaceOf, log} = deps
	const now = deps.now ?? Date.now
	active = true
	let inFlight = 0
	let lastAt = Number.NEGATIVE_INFINITY
	let logged = 0

	const count = (read: Read, result: (typeof RESULTS)[number], by = 1) => bump(results, `${read}\t${result}`, by)
	const refuse = (read: Read, reason: Unsupported, by = 1) => {
		count(read, "unsupported", by)
		bump(unsupported, reason, by)
	}
	/** Classifies a thrown error for `read`; counts only. */
	const failed = (read: Read, error: unknown, by = 1) => {
		if (error instanceof AmbiguousRecordError) return refuse(read, "bad_record", by)
		count(read, "failed", by)
		if (!(error instanceof GeoDbError)) log.warn("user votes twin failed", {read, error: String(error)})
	}

	async function run(userId: string, candidates: string[], served: Map<string, SharedClaim[]>) {
		let votesSpace: string | null
		try {
			votesSpace = await systemSpaceOf("votes")
		} catch (error) {
			failed("shared_claims", error, candidates.length)
			failed("voted_by", error)
			return
		}
		if (!votesSpace) {
			refuse("shared_claims", "no_votes_space", candidates.length)
			refuse("voted_by", "no_votes_space")
			return
		}
		if (!geoDb.readBacklinks || !geoDb.readCards) {
			refuse("shared_claims", "no_reads", candidates.length)
			refuse("voted_by", "no_reads")
			return
		}
		// One user at a time, so a candidate's own read failing counts that candidate only.
		try {
			const {byCandidate, truncated} = await readSharedClaimsGeoDb(geoDb, votesSpace, userId, candidates)
			if (truncated) refuse("shared_claims", "truncated", candidates.length)
			else
				for (const id of candidates) {
					const same = sameSharedClaims(served.get(id) ?? [], byCandidate.get(id) ?? [])
					count("shared_claims", same ? "match" : "mismatch")
					if (!same && logged < 20) {
						logged++
						log.warn("user votes twin mismatch", {
							read: "shared_claims",
							servedCount: served.get(id)?.length ?? 0,
						})
					}
				}
		} catch (error) {
			failed("shared_claims", error, candidates.length)
		}
		try {
			const [twin, servedIds] = await Promise.all([
				readEntityVotedByGeoDb(geoDb, votesSpace, userId, POSITIONS),
				readVotedByIds(db, userId, POSITIONS),
			])
			if (twin.truncated) refuse("voted_by", "truncated")
			else {
				const same = sameVotedBy(servedIds, twin.refs)
				count("voted_by", same ? "match" : "mismatch")
				if (!same && logged < 20) {
					logged++
					log.warn("user votes twin mismatch", {
						read: "voted_by",
						servedCount: servedIds.length,
						twinCount: twin.refs.length,
					})
				}
			}
		} catch (error) {
			failed("voted_by", error)
		}
	}

	return {
		compare(userId, candidateIds, served) {
			if (!config.enabled) return
			const t = now()
			if (t - lastAt < config.intervalMs) return
			lastAt = t
			const sampled = candidateIds.filter((id) => /^[0-9a-f]{32}$/.test(id)).slice(0, config.sample)
			if (!/^[0-9a-f]{32}$/.test(userId) || sampled.length === 0) return
			if (inFlight >= config.maxInFlight) {
				refuse("shared_claims", "busy", sampled.length)
				refuse("voted_by", "busy")
				return
			}
			inFlight++
			void run(userId, sampled, served)
				.catch(() => {})
				.finally(() => {
					inFlight--
				})
		},
	}
}
