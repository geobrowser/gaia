/**
 * Database queries for proposal status.
 *
 * Uses Effect for error handling and tracing.
 */

import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Data, Effect} from "effect"
import {
	type ProposalAction,
	type ProposalActionType,
	type ProposalWithVotes,
	VOTE_OPTIONS,
	type Vote,
	type VoteOption,
	type VotingMode,
} from "./types"

type Database = NodePgDatabase<Record<string, unknown>>

// =============================================================================
// Shared Types and Mappers
// =============================================================================

/** Raw action row from JSON aggregation */
interface ActionJsonRow {
	action_type: string
	target_id: string | null
	content_uri: string | null
	content_id: string | null
	quorum: number | null
	fast_threshold: number | null
	slow_threshold: number | null
	duration: number | null
}

/** Raw vote row from JSON aggregation */
interface VoteJsonRow {
	voter_id: string
	vote: string
}

/** Base proposal row fields shared between queries */
interface BaseProposalRow extends Record<string, unknown> {
	id: string
	space_id: string
	name: string | null
	proposed_by: string
	voting_mode: "Fast" | "Slow"
	start_time: string
	end_time: string
	quorum: string
	threshold: string
	executed_at: string | null
	yes_count: string
	no_count: string
	abstain_count: string
	actions_json: ActionJsonRow[] | null
}

/**
 * Maps raw action JSON to domain ProposalAction.
 */
function mapActionsFromJson(actionsJson: ActionJsonRow[] | null): ProposalAction[] {
	return (actionsJson ?? []).map((a) => ({
		actionType: a.action_type as ProposalActionType,
		targetId: a.target_id,
		contentUri: a.content_uri,
		contentId: a.content_id,
		quorum: a.quorum,
		fastThreshold: a.fast_threshold,
		slowThreshold: a.slow_threshold,
		duration: a.duration,
	}))
}

/**
 * Maps raw vote JSON to domain Vote, filtering invalid values.
 */
function mapVotesFromJson(votesJson: VoteJsonRow[] | null): Vote[] {
	return (votesJson ?? [])
		.map((v) => {
			const voteUpper = v.vote.toUpperCase()
			if (!VOTE_OPTIONS.includes(voteUpper as VoteOption)) {
				// Skip invalid votes rather than crash or pass bad data
				return null
			}
			return {
				voterId: v.voter_id,
				vote: voteUpper as VoteOption,
			}
		})
		.filter((v): v is Vote => v !== null)
}

/**
 * Maps base proposal row fields to domain ProposalWithVotes.
 * Votes array must be provided separately (empty for list, populated for single).
 */
function mapRowToProposal(row: BaseProposalRow, votes: Vote[]): ProposalWithVotes {
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
		votes,
		actions: mapActionsFromJson(row.actions_json),
	}
}

// =============================================================================
// Errors
// =============================================================================

/**
 * Database query error with structured context.
 * Using Data.TaggedError for exhaustive pattern matching with Effect.
 */
export class QueryError extends Data.TaggedError("QueryError")<{
	readonly operation: string
	readonly cause: Error
}> {
	get message(): string {
		return this.cause.message
	}
}

/**
 * Fetches a proposal with aggregated vote counts, individual votes, and actions.
 * Uses JSON aggregation to fetch everything in a single round trip.
 *
 * @param db - Drizzle database instance
 * @param proposalId - UUID of the proposal to fetch
 * @returns The proposal with vote counts, votes, and actions, or null if not found
 */
export function getProposalWithVotes(
	db: Database,
	proposalId: string,
): Effect.Effect<ProposalWithVotes | null, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			// PostgreSQL returns bigint columns as strings to preserve precision
			// Use JSON aggregation to get votes and actions in a single query
			const result = await db.execute<BaseProposalRow & {votes_json: VoteJsonRow[] | null}>(sql`
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
          COALESCE(vc.yes_count, 0) as yes_count,
          COALESCE(vc.no_count, 0) as no_count,
          COALESCE(vc.abstain_count, 0) as abstain_count,
          v.votes_json,
          a.actions_json
        FROM proposals p
        LEFT JOIN LATERAL (
          SELECT
            COUNT(*) FILTER (WHERE vote = 'Yes') as yes_count,
            COUNT(*) FILTER (WHERE vote = 'No') as no_count,
            COUNT(*) FILTER (WHERE vote = 'Abstain') as abstain_count
          FROM proposal_votes
          WHERE proposal_id = p.id
        ) vc ON true
        LEFT JOIN LATERAL (
          SELECT COALESCE(json_agg(json_build_object(
            'voter_id', voter_id,
            'vote', vote
          )), '[]'::json) as votes_json
          FROM proposal_votes
          WHERE proposal_id = p.id
        ) v ON true
        LEFT JOIN LATERAL (
          SELECT COALESCE(json_agg(json_build_object(
            'action_type', action_type,
            'target_id', target_id,
            'content_uri', content_uri,
            'content_id', encode(content_id, 'hex'),
            'quorum', quorum,
            'fast_threshold', fast_threshold,
            'slow_threshold', slow_threshold,
            'duration', duration
          )), '[]'::json) as actions_json
          FROM proposal_actions
          WHERE proposal_id = p.id
        ) a ON true
        WHERE p.id = ${proposalId}::uuid
      `)

			const row = result.rows[0]
			if (!row) return null

			const votes = mapVotesFromJson(row.votes_json)
			return mapRowToProposal(row, votes)
		},
		catch: (error) =>
			new QueryError({
				operation: "getProposalWithVotes",
				cause: error as Error,
			}),
	}).pipe(
		Effect.withSpan("queries.getProposalWithVotes", {
			attributes: {"query.proposal_id": proposalId},
		}),
	)
}

/**
 * Options for listing proposals in a space.
 */
export interface ListProposalsOptions {
	spaceId: string
	limit: number
	cursor?: string
	/** Include only proposals with these action types */
	actionTypes?: ProposalActionType[]
	/** Exclude proposals with these action types */
	excludeActionTypes?: ProposalActionType[]
}

/**
 * Result of listing proposals with cursor for pagination.
 */
export interface ListProposalsResult {
	proposals: ProposalWithVotes[]
	nextCursor: string | null
}

/**
 * Lists proposals in a space with optional filtering by action type.
 * Uses cursor-based pagination ordered by created_at DESC.
 * Returns vote counts only (not individual voters) for performance.
 *
 * @param db - Drizzle database instance
 * @param options - Query options (spaceId, limit, cursor, actionType)
 * @returns Paginated list of proposals with vote counts and actions
 */
export function listProposalsInSpace(
	db: Database,
	options: ListProposalsOptions,
): Effect.Effect<ListProposalsResult, QueryError> {
	const {spaceId, limit, cursor, actionTypes, excludeActionTypes} = options

	return Effect.tryPromise({
		try: async () => {
			// Build the query dynamically based on filters
			// Cursor is the created_at timestamp for stable pagination
			const cursorCondition = cursor ? sql`AND p.created_at < ${cursor}` : sql``

			// Build action type filter conditions
			// Use EXISTS subquery instead of JOIN to avoid SQL injection and cartesian products
			let actionTypeCondition = sql``
			if (actionTypes && actionTypes.length > 0) {
				// Include filter: proposal must have at least one of these action types
				// Build safe parameterized OR conditions
				const actionTypeChecks = actionTypes.map((t) => sql`pa_filter.action_type = ${t}::"proposalActionType"`)
				const actionTypeOr = actionTypeChecks.reduce((acc, check) => sql`${acc} OR ${check}`)
				actionTypeCondition = sql`AND EXISTS (
          SELECT 1 FROM proposal_actions pa_filter 
          WHERE pa_filter.proposal_id = p.id AND (${actionTypeOr})
        )`
			} else if (excludeActionTypes && excludeActionTypes.length > 0) {
				// Exclude filter: proposal must NOT have any of these action types
				const excludeTypeChecks = excludeActionTypes.map(
					(t) => sql`pa_exclude.action_type = ${t}::"proposalActionType"`,
				)
				const excludeTypeOr = excludeTypeChecks.reduce((acc, check) => sql`${acc} OR ${check}`)
				actionTypeCondition = sql`AND NOT EXISTS (
          SELECT 1 FROM proposal_actions pa_exclude 
          WHERE pa_exclude.proposal_id = p.id AND (${excludeTypeOr})
        )`
			}

			// Note: votes_json omitted for performance - use single proposal endpoint for voters
			const result = await db.execute<BaseProposalRow & {created_at: string}>(
				sql`
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
          p.created_at,
          COALESCE(vote_counts.yes_count, 0) as yes_count,
          COALESCE(vote_counts.no_count, 0) as no_count,
          COALESCE(vote_counts.abstain_count, 0) as abstain_count,
          actions_agg.actions_json
        FROM proposals p
        LEFT JOIN LATERAL (
          SELECT
            COUNT(*) FILTER (WHERE vote = 'Yes') as yes_count,
            COUNT(*) FILTER (WHERE vote = 'No') as no_count,
            COUNT(*) FILTER (WHERE vote = 'Abstain') as abstain_count
          FROM proposal_votes
          WHERE proposal_id = p.id
        ) vote_counts ON true
        LEFT JOIN LATERAL (
          SELECT COALESCE(json_agg(json_build_object(
            'action_type', action_type,
            'target_id', target_id,
            'content_uri', content_uri,
            'content_id', encode(content_id, 'hex'),
            'quorum', quorum,
            'fast_threshold', fast_threshold,
            'slow_threshold', slow_threshold,
            'duration', duration
          )), '[]'::json) as actions_json
          FROM proposal_actions
          WHERE proposal_id = p.id
        ) actions_agg ON true
        WHERE p.space_id = ${spaceId}::uuid
        ${cursorCondition}
        ${actionTypeCondition}
        ORDER BY p.created_at DESC
        LIMIT ${limit + 1}
      `,
			)

			const rows = result.rows
			const hasMore = rows.length > limit
			const proposalRows = hasMore ? rows.slice(0, limit) : rows

			// Map rows to domain objects with empty votes (use single endpoint for voters)
			const proposals = proposalRows.map((row) => mapRowToProposal(row, []))

			// Next cursor is the created_at of the last item
			const lastRow = proposalRows[proposalRows.length - 1]
			const nextCursor = hasMore && lastRow ? lastRow.created_at : null

			return {proposals, nextCursor}
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
	)
}
