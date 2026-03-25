/**
 * Database connection and detection query for executable slow-path proposals.
 *
 * Detection SQL is a third copy of proposal status logic (after computeProposalStatus()
 * in api/src/proposals/status.ts and sqlIsExecutable() in api/src/proposals/queries.ts).
 * It adds a 60s clock-skew buffer not present in the other copies.
 *
 * See: docs/plans/2026-03-02-feat-proposal-auto-executor-plan.md §Detection Query
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
 * Finds slow-path proposals that are EXECUTABLE:
 * - Not yet executed (executed_at IS NULL)
 * - Slow voting mode
 * - Created in the past (guards against corrupt future timestamps)
 * - Created within MAX_PROPOSAL_AGE (7 days) — older proposals are likely stuck
 * - Voting period ended (with clock-skew buffer)
 * - Quorum reached (total votes >= quorum)
 * - Threshold reached: (RATIO_BASE - threshold) * yes > threshold * no
 *
 * The RATIO_BASE constant (10,000,000) matches the smart contract value.
 * Source: api/src/proposals/types.ts — RATIO_BASE = 10_000_000n
 *
 * ORDER BY created_at::bigint ASC for FIFO ordering. The created_at column
 * stores Unix timestamps as text, so ::bigint cast ensures numeric ordering.
 */
const DETECTION_SQL = `
SELECT p.id, p.space_id AS "spaceId"
FROM proposals p
WHERE p.executed_at IS NULL
  AND p.voting_mode = 'Slow'
  AND p.created_at::bigint <= $1::bigint
  AND $1::bigint - p.created_at::bigint < ${MAX_PROPOSAL_AGE}
  AND $1::bigint > p.end_time + ${CLOCK_SKEW_BUFFER}
  AND (p.yes_count + p.no_count + p.abstain_count) >= p.quorum
  -- RATIO_BASE = 10,000,000 (protocol constant from api/src/proposals/types.ts)
  AND (${RATIO_BASE} - p.threshold::numeric) * p.yes_count::numeric
      > p.threshold::numeric * p.no_count::numeric
ORDER BY p.created_at::bigint ASC
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
