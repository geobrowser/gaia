/**
 * Database connection and detection query for executable slow-path proposals.
 *
 * Detection SQL is a third copy of proposal status logic (after
 * computeProposalStatus() in api/src/proposals/status.ts and sqlIsExecutable()
 * in api/src/proposals/queries.ts). Any change to the decision tree must land
 * in all three. V2 (GEO-485):
 *   - Reads from the `proposals_current` view (identity + current version).
 *   - Uses `partial_percentage_support_threshold` for slow-late and
 *     `universal_percentage_support_threshold` for slow-early.
 *   - Enforces `execute_by` as the deadline, with a MAX_PROPOSAL_AGE fallback
 *     for proposals that have no deadline set (NULL `execute_by`).
 *   - Adds the 60s CLOCK_SKEW_BUFFER only on the slow-late `end_time` check
 *     (slow-early runs while voting is ongoing, so no buffer needed).
 *
 * Fast-path proposals are NOT handled here — they auto-execute on-chain when
 * yes_count reaches `flat_support_threshold`, and the kg-indexer picks up
 * the resulting PROPOSAL_EXECUTED event.
 */

import {Effect} from "effect"
import Pg from "pg"

type PgClient = InstanceType<typeof Pg.Client>

import {InfraError, RATIO_BASE} from "./contracts.js"

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface Proposal {
	/** Proposal UUID (with dashes) */
	id: string
	/** Space UUID (with dashes) */
	spaceId: string
}

/**
 * A membership request: a Fast-mode proposal whose single action is an AddMember
 * in an allowlisted space. Candidate for an auto-accept YES vote.
 */
export interface MembershipRequest {
	/** Proposal UUID (with dashes) */
	id: string
	/** DAO space UUID being joined (with dashes); always in the allowlist */
	spaceId: string
	/** Member space UUID being added (with dashes) */
	requesterId: string
}

// ---------------------------------------------------------------------------
// Clock skew buffer
// ---------------------------------------------------------------------------

/**
 * Seconds added to end_time to guard against clock skew between the CronJob pod
 * and on-chain block.timestamp. Hardcoded — not configurable.
 */
const CLOCK_SKEW_BUFFER = 60

/**
 * Maximum proposal age in seconds. Proposals older than this are ignored.
 * Prevents the executor from retrying permanently stuck proposals (e.g.,
 * proposals whose embedded actions revert due to stale state like addMember
 * for an already-added member). 7 days is well beyond normal voting periods
 * (typically 1 day) while ensuring stale proposals age out naturally.
 */
export const MAX_PROPOSAL_AGE = 7 * 24 * 60 * 60 // 7 days

// ---------------------------------------------------------------------------
// Detection SQL
// ---------------------------------------------------------------------------

/**
 * Finds Slow-mode proposals that are EXECUTABLE right now. Two branches:
 *
 *   Slow-late (after voting ends):
 *     - `now > end_time + CLOCK_SKEW_BUFFER`
 *     - quorum met
 *     - `(RATIO_BASE - partial) × yes > partial × no`
 *
 *   Slow-early (voting still ongoing, NEW in V2 via GEO-514):
 *     - `now <= end_time`
 *     - `universal_percentage_support_threshold > 0` (feature enabled)
 *     - space has indexed editors (`space_editor_counts.total_editors > 0`)
 *     - `yes_count >= ceil(universal × total_editors / RATIO_BASE)`
 *
 * Both branches additionally require:
 *   - not yet executed (`executed_at IS NULL`)
 *   - within execution deadline: `now <= execute_by` when set, else within
 *     MAX_PROPOSAL_AGE of `created_at` (fallback for proposals without a
 *     deadline — prevents the executor from retrying permanently stuck
 *     proposals forever).
 *
 * RATIO_BASE (10,000,000) matches the smart contract. Source of truth:
 * api/src/proposals/types.ts.
 *
 * ORDER BY created_at::bigint ASC for FIFO ordering — created_at is text, so
 * the cast ensures numeric comparison.
 */
export const DETECTION_SQL = `
SELECT pc.id, pc.space_id AS "spaceId"
FROM proposals_current pc
WHERE pc.executed_at IS NULL
  AND pc.voting_mode = 'Slow'
  AND pc.created_at::bigint <= $1::bigint
  AND (
    (pc.execute_by IS NOT NULL AND $1::bigint <= pc.execute_by)
    OR (pc.execute_by IS NULL AND $1::bigint - pc.created_at::bigint < ${MAX_PROPOSAL_AGE})
  )
  AND (
    -- Slow-late: voting ended + quorum met + partial ratio
    (
      $1::bigint > pc.end_time + ${CLOCK_SKEW_BUFFER}
      AND (pc.yes_count + pc.no_count + pc.abstain_count) >= pc.quorum
      AND (${RATIO_BASE} - pc.partial_percentage_support_threshold::numeric) * pc.yes_count::numeric
          > pc.partial_percentage_support_threshold::numeric * pc.no_count::numeric
    )
    -- Slow-early: voting still ongoing + yes_count meets universal × total_editors ceiling
    OR (
      $1::bigint <= pc.end_time
      AND pc.universal_percentage_support_threshold > 0
      AND COALESCE(
        (SELECT sec.total_editors FROM space_editor_counts sec WHERE sec.space_id = pc.space_id),
        0
      ) > 0
      AND pc.yes_count::numeric >= CEIL(
        pc.universal_percentage_support_threshold::numeric
        * COALESCE(
          (SELECT sec.total_editors FROM space_editor_counts sec WHERE sec.space_id = pc.space_id),
          0
        )::numeric
        / ${RATIO_BASE}::numeric
      )
    )
  )
ORDER BY pc.created_at::bigint ASC
`

// ---------------------------------------------------------------------------
// DB connection
// ---------------------------------------------------------------------------

/**
 * Connect to PostgreSQL. Uses a single Client (not a pool) since this is a
 * short-lived CronJob process that runs one query then exits.
 *
 * Connection settings are chosen for a batch CronJob with activeDeadlineSeconds: 290:
 * - connectionTimeoutMillis: 5s — fail fast if DB/PgBouncer is unreachable
 * - keepAlive: true — detect broken TCP connections (e.g., pod network disruption)
 * - keepAliveInitialDelayMillis: 10s — start probing quickly (short-lived process)
 * - application_name: shows in pg_stat_activity for observability
 */
export function connectDb(databaseUrl: string): Effect.Effect<PgClient, InfraError> {
	return Effect.tryPromise({
		try: async () => {
			const client = new Pg.Client({
				connectionString: databaseUrl,
				connectionTimeoutMillis: 5_000,
				keepAlive: true,
				keepAliveInitialDelayMillis: 10_000,
				application_name: "proposal-executor",
			})
			await client.connect()
			return client
		},
		catch: (error) => {
			// Sanitize: pg connection errors may contain the connection string (with password).
			// Extract only the error code and a safe summary — never interpolate the raw error.
			const code = error instanceof Error && "code" in error ? (error as {code: string}).code : "UNKNOWN"
			const safe =
				error instanceof Error
					? error.message.replace(/postgresql?:\/\/[^\s]+/gi, "<redacted>")
					: "unknown error"
			return new InfraError({message: `DB connect failed: [${code}] ${safe}`, durationMs: 0})
		},
	})
}

/**
 * Disconnect from PostgreSQL. Best-effort — errors are logged but not propagated.
 */
export function disconnectDb(client: PgClient): Effect.Effect<void> {
	return Effect.tryPromise({
		try: () => client.end(),
		catch: () => undefined,
	}).pipe(Effect.catchAll(() => Effect.void))
}

// ---------------------------------------------------------------------------
// Detection query
// ---------------------------------------------------------------------------

/**
 * Find all slow-path proposals that are EXECUTABLE right now.
 *
 * @param client - Connected pg.Client
 * @param nowSeconds - Current Unix timestamp in seconds (passed as Number, cast to bigint in SQL)
 */
export function findExecutableProposals(client: PgClient, nowSeconds: number): Effect.Effect<Proposal[], InfraError> {
	return Effect.tryPromise({
		try: async () => {
			const result = await client.query<Proposal>(DETECTION_SQL, [nowSeconds])
			return result.rows
		},
		catch: (error) => {
			// Sanitize: pg errors on broken connections may contain the connection string (with password).
			const safe =
				error instanceof Error
					? error.message.replace(/postgresql?:\/\/[^\s]+/gi, "<redacted>")
					: "unknown error"
			return new InfraError({message: `Detection query failed: ${safe}`, durationMs: 0})
		},
	})
}

// ---------------------------------------------------------------------------
// Membership-request detection (stage 1)
// ---------------------------------------------------------------------------

/**
 * Finds open, untouched, allowlisted request-to-join proposals (stage 1 of the
 * two-stage untouched check — the indexer side). A row qualifies iff:
 * - space_id is in the allowlist ($1)
 * - Fast voting mode (a single YES vote can execute via the fast-path threshold)
 * - Not yet executed (executed_at IS NULL)
 * - Created in the past (guards against corrupt future timestamps; no age cutoff
 *   — backlog is admitted, unlike the executor's MAX_PROPOSAL_AGE)
 * - Voting period has not ended (now <= end_time + CLOCK_SKEW_BUFFER) — the protocol
 *   rejects votes cast after the period closes, and an untouched Fast proposal whose
 *   period has ended is already classified REJECTED (threshold not reached). The same
 *   60s skew buffer the stage-2 eligibility check applies (isVotingOpen, membership.ts)
 *   is added here so detection never excludes a request that stage 2 would still vote
 *   on — `now` must therefore be the same chain-sourced timestamp stage 2 uses.
 * - Exactly one action, and it is an AddMember
 * - No indexed votes (NOT EXISTS in proposal_votes) — checks raw indexed votes,
 *   not the async-denormalized yes_count/no_count/abstain_count columns
 *
 * NOTE: this does not distinguish a self-service request-to-join from an editor-initiated
 * AddMember — any untouched, in-window, single-AddMember Fast proposal in an allowlisted
 * space is auto-accepted. If product requires restricting to self-service requests, we'll
 * add that constraint.
 *
 * This filters voting_mode = 'Fast' while the executor's query filters 'Slow',
 * so the two paths never return the same proposal.
 *
 * ORDER BY created_at::bigint ASC for FIFO ordering (created_at is text Unix
 * seconds, so the ::bigint cast ensures numeric ordering).
 */
export const MEMBERSHIP_DETECTION_SQL = `
SELECT p.id, p.space_id AS "spaceId", a.target_id AS "requesterId"
FROM proposals p
JOIN proposal_actions a ON a.proposal_id = p.id
WHERE p.space_id = ANY($1)
  AND p.voting_mode = 'Fast'
  AND p.executed_at IS NULL
  AND p.created_at::bigint <= $2::bigint
  AND $2::bigint <= p.end_time + ${CLOCK_SKEW_BUFFER}
  AND a.action_type = 'AddMember'
  AND NOT EXISTS (SELECT 1 FROM proposal_votes pv WHERE pv.proposal_id = p.id)
  AND (SELECT COUNT(*) FROM proposal_actions a2 WHERE a2.proposal_id = p.id) = 1
ORDER BY p.created_at::bigint ASC
`

/**
 * Find all untouched request-to-join proposals in allowlisted spaces that are
 * candidates for an auto-accept YES vote.
 *
 * Short-circuits to [] without querying when the allowlist is empty — the
 * kill switch (an emptied MEMBERSHIP_AUTOACCEPT_SPACE_IDS stops all activity).
 *
 * @param client - Connected pg.Client
 * @param allowlistSpaceIds - Dashed UUIDs of allowlisted DAO spaces (from bytes16 config)
 * @param nowSeconds - Current Unix timestamp in seconds. MUST be the same chain-sourced
 *   clock stage 2 uses (readChainTimeSeconds), so the two stages share one notion of
 *   "now" and the same skew policy — passing the pod wall clock would reintroduce the
 *   drift the +CLOCK_SKEW_BUFFER window is meant to absorb.
 */
export function findMembershipRequests(
	client: PgClient,
	allowlistSpaceIds: string[],
	nowSeconds: number,
): Effect.Effect<MembershipRequest[], InfraError> {
	if (allowlistSpaceIds.length === 0) {
		return Effect.succeed([])
	}
	return Effect.tryPromise({
		try: async () => {
			const result = await client.query<MembershipRequest>(MEMBERSHIP_DETECTION_SQL, [
				allowlistSpaceIds,
				nowSeconds,
			])
			return result.rows
		},
		catch: (error) => {
			// Sanitize: pg errors on broken connections may contain the connection string (with password).
			const safe =
				error instanceof Error
					? error.message.replace(/postgresql?:\/\/[^\s]+/gi, "<redacted>")
					: "unknown error"
			return new InfraError({message: `Membership detection query failed: ${safe}`, durationMs: 0})
		},
	})
}
