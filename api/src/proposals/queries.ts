/**
 * Database queries for proposal status.
 *
 * Uses Effect for error handling and tracing.
 */

import { Data, Effect } from "effect";
import { sql } from "drizzle-orm";
import type { NodePgDatabase } from "drizzle-orm/node-postgres";
import type {
  ProposalActionType,
  ProposalWithVotes,
  VotingMode,
} from "./types";

type Database = NodePgDatabase<Record<string, unknown>>;

/**
 * Database query error with structured context.
 * Using Data.TaggedError for exhaustive pattern matching with Effect.
 */
export class QueryError extends Data.TaggedError("QueryError")<{
  readonly operation: string;
  readonly cause: Error;
}> {
  get message(): string {
    return this.cause.message;
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
        name: string | null;
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
					p.name,
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
        name: row.name,
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
        cause: error as Error,
      }),
  }).pipe(
    Effect.withSpan("queries.getProposalWithVotes", {
      attributes: { "query.proposal_id": proposalId },
    }),
  );
}

/**
 * Options for listing proposals in a space.
 */
export interface ListProposalsOptions {
  spaceId: string;
  limit: number;
  cursor?: string;
  /** Include only proposals with these action types */
  actionTypes?: ProposalActionType[];
  /** Exclude proposals with these action types */
  excludeActionTypes?: ProposalActionType[];
}

/**
 * Result of listing proposals with cursor for pagination.
 */
export interface ListProposalsResult {
  proposals: ProposalWithVotes[];
  nextCursor: string | null;
}

/**
 * Lists proposals in a space with optional filtering by action type.
 * Uses cursor-based pagination ordered by created_at DESC.
 *
 * @param db - Drizzle database instance
 * @param options - Query options (spaceId, limit, cursor, actionType)
 * @returns Paginated list of proposals with vote counts
 */
export function listProposalsInSpace(
  db: Database,
  options: ListProposalsOptions,
): Effect.Effect<ListProposalsResult, QueryError> {
  const { spaceId, limit, cursor, actionTypes, excludeActionTypes } = options;

  return Effect.tryPromise({
    try: async () => {
      // Build the query dynamically based on filters
      // Cursor is the created_at timestamp for stable pagination
      const cursorCondition = cursor
        ? sql`AND p.created_at < ${cursor}`
        : sql``;

      // Build action type filter conditions
      let actionTypeJoin = sql``;
      if (actionTypes && actionTypes.length > 0) {
        // Include filter: proposal must have at least one of these action types
        const actionTypesArray = `{${actionTypes.join(",")}}`;
        actionTypeJoin = sql`INNER JOIN proposal_actions pa ON pa.proposal_id = p.id AND pa.action_type = ANY(${actionTypesArray}::"proposalActionType"[])`;
      } else if (excludeActionTypes && excludeActionTypes.length > 0) {
        // Exclude filter: proposal must NOT have any of these action types
        const excludeTypesArray = `{${excludeActionTypes.join(",")}}`;
        actionTypeJoin = sql`LEFT JOIN proposal_actions pa_exclude ON pa_exclude.proposal_id = p.id AND pa_exclude.action_type = ANY(${excludeTypesArray}::"proposalActionType"[])`;
      }

      const excludeCondition =
        excludeActionTypes &&
        excludeActionTypes.length > 0 &&
        (!actionTypes || actionTypes.length === 0)
          ? sql`AND pa_exclude.id IS NULL`
          : sql``;

      const result = await db.execute<{
        id: string;
        space_id: string;
        name: string | null;
        proposed_by: string;
        voting_mode: "Fast" | "Slow";
        start_time: string;
        end_time: string;
        quorum: string;
        threshold: string;
        executed_at: string | null;
        created_at: string;
        yes_count: string;
        no_count: string;
        abstain_count: string;
      }>(sql`
        SELECT DISTINCT ON (p.id)
          p.id,
          p.space_id,
          p.name,
          p.proposed_by,
          p.voting_mode,
          p.start_time,
          p.end_time,
          p.quorum,
          p.threshold,
          p.executed_at,
          p.created_at,
          COALESCE(vote_counts.yes_count, 0) as yes_count,
          COALESCE(vote_counts.no_count, 0) as no_count,
          COALESCE(vote_counts.abstain_count, 0) as abstain_count
        FROM proposals p
        ${actionTypeJoin}
        LEFT JOIN LATERAL (
          SELECT
            COUNT(*) FILTER (WHERE vote = 'Yes') as yes_count,
            COUNT(*) FILTER (WHERE vote = 'No') as no_count,
            COUNT(*) FILTER (WHERE vote = 'Abstain') as abstain_count
          FROM proposal_votes
          WHERE proposal_id = p.id
        ) vote_counts ON true
        WHERE p.space_id = ${spaceId}::uuid
        ${cursorCondition}
        ${excludeCondition}
        ORDER BY p.id, p.created_at DESC
        LIMIT ${limit + 1}
      `);

      const rows = result.rows;
      const hasMore = rows.length > limit;
      const proposalRows = hasMore ? rows.slice(0, limit) : rows;

      const proposals: ProposalWithVotes[] = proposalRows.map((row) => ({
        id: row.id,
        spaceId: row.space_id,
        name: row.name,
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
      }));

      // Next cursor is the created_at of the last item
      const lastRow = proposalRows[proposalRows.length - 1];
      const nextCursor = hasMore && lastRow ? lastRow.created_at : null;

      return { proposals, nextCursor };
    },
    catch: (error) =>
      new QueryError({
        operation: "listProposalsInSpace",
        cause: error as Error,
      }),
  }).pipe(
    Effect.withSpan("queries.listProposalsInSpace", {
      attributes: {
        "query.space_id": spaceId,
        "query.limit": limit,
        "query.cursor": cursor ?? "none",
        "query.action_types": actionTypes?.join(",") ?? "all",
        "query.exclude_action_types": excludeActionTypes?.join(",") ?? "none",
      },
    }),
  );
}
