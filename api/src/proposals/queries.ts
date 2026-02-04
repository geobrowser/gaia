/**
 * Database queries for proposal status.
 *
 * Uses Effect for error handling and tracing.
 */

import { Data, Effect } from "effect";
import { sql } from "drizzle-orm";
import type { NodePgDatabase } from "drizzle-orm/node-postgres";
import type { ProposalWithVotes, VotingMode } from "./types";

type Database = NodePgDatabase<Record<string, unknown>>;

/**
 * Database query error with structured context.
 * Using Data.TaggedError for exhaustive pattern matching with Effect.
 */
export class QueryError extends Data.TaggedError("QueryError")<{
  readonly operation: string;
  readonly cause: Error | string;
}> {
  get message(): string {
    return this.cause instanceof Error ? this.cause.message : this.cause;
  }
}

/**
 * Fetches a proposal with aggregated vote counts in a single query.
 * Uses bigint for all numeric values to match contract precision.
 *
 * @param db - Drizzle database instance
 * @param proposalId - UUID of the proposal to fetch
 * @returns The proposal with vote counts, or null if not found
 */
export function getProposalWithVotes(
  db: Database,
  proposalId: string,
): Effect.Effect<ProposalWithVotes | null, QueryError> {
  return Effect.tryPromise({
    try: async () => {
      // PostgreSQL returns bigint columns as strings to preserve precision
      const result = await db.execute<{
        id: string;
        space_id: string;
        proposed_by: string;
        voting_mode: "Fast" | "Slow";
        start_time: string;
        end_time: string;
        quorum: string;
        threshold: string;
        executed_at: string | null;
        yes_count: string;
        no_count: string;
        abstain_count: string;
      }>(sql`
				SELECT 
					p.id,
					p.space_id,
					p.proposed_by,
					p.voting_mode,
					p.start_time,
					p.end_time,
					p.quorum,
					p.threshold,
					p.executed_at,
					COALESCE(COUNT(*) FILTER (WHERE pv.vote = 'Yes'), 0) as yes_count,
					COALESCE(COUNT(*) FILTER (WHERE pv.vote = 'No'), 0) as no_count,
					COALESCE(COUNT(*) FILTER (WHERE pv.vote = 'Abstain'), 0) as abstain_count
				FROM proposals p
				LEFT JOIN proposal_votes pv ON pv.proposal_id = p.id
				WHERE p.id = ${proposalId}::uuid
				GROUP BY p.id
			`);

      const row = result.rows[0];
      if (!row) return null;

      // Convert PostgreSQL bigint strings to JavaScript bigint
      return {
        id: row.id,
        spaceId: row.space_id,
        proposedBy: row.proposed_by,
        votingMode: row.voting_mode as VotingMode,
        startTime: BigInt(row.start_time),
        endTime: BigInt(row.end_time),
        quorum: BigInt(row.quorum),
        threshold: BigInt(row.threshold),
        executedAt: row.executed_at ? BigInt(row.executed_at) : null,
        yesCount: BigInt(row.yes_count),
        noCount: BigInt(row.no_count),
        abstainCount: BigInt(row.abstain_count),
      };
    },
    catch: (error) =>
      new QueryError({
        operation: "getProposalWithVotes",
        cause: error instanceof Error ? error : String(error),
      }),
  }).pipe(
    Effect.withSpan("queries.getProposalWithVotes", {
      attributes: { "query.proposal_id": proposalId },
    }),
  );
}
