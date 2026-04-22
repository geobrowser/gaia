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
	type ProposalListItem,
	type ProposalStatus,
	type ProposalWithVotes,
	RATIO_BASE,
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
	partial_percentage_support_threshold: number | null
	universal_percentage_support_threshold: number | null
	flat_support_threshold: number | null
	disable_fast_path_access_for_new_members: boolean | null
	execution_grace_period: number | null
}

/** Raw vote row from JSON aggregation */
interface VoteJsonRow {
	voter_id: string
	vote: string
}

/**
 * Base proposal row fields shared between queries.
 *
 * Rows come from the `proposals_current` view (identity joined with the
 * current `proposal_versions` entry) LEFT-JOINed implicitly — via a
 * correlated subquery — to `space_editor_counts` for `total_editors`.
 *
 * The legacy `threshold` column is retained on the view for backcompat but
 * new logic reads the per-mode V2 fields directly.
 */
interface BaseProposalRow extends Record<string, unknown> {
	id: string
	space_id: string
	name: string | null
	proposed_by: string
	proposal_version: number
	voting_mode: "Fast" | "Slow"
	start_time: string
	end_time: string
	quorum: string
	threshold: string
	flat_support_threshold: string
	partial_percentage_support_threshold: string
	universal_percentage_support_threshold: string
	execute_by: string | null
	total_editors: string
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
		partialPercentageSupportThreshold: a.partial_percentage_support_threshold,
		universalPercentageSupportThreshold: a.universal_percentage_support_threshold,
		flatSupportThreshold: a.flat_support_threshold,
		disableFastPathAccessForNewMembers: a.disable_fast_path_access_for_new_members,
		executionGracePeriod: a.execution_grace_period,
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
 * Maps shared base fields from a proposal row.
 *
 * Computes the legacy `threshold` compatibility projection: Fast proposals
 * use `flatSupportThreshold`, Slow proposals use
 * `partialPercentageSupportThreshold`. New callers should read the per-mode
 * V2 field directly; `threshold` stays for existing API response clients.
 */
function mapBaseFields(row: BaseProposalRow) {
	const votingMode = row.voting_mode as VotingMode
	const flat = BigInt(row.flat_support_threshold)
	const partial = BigInt(row.partial_percentage_support_threshold)
	const legacyThreshold = votingMode === "Fast" ? flat : partial
	return {
		id: row.id,
		spaceId: row.space_id,
		name: row.name,
		proposedBy: row.proposed_by,
		proposalVersion: row.proposal_version,
		votingMode,
		startTime: BigInt(row.start_time),
		endTime: BigInt(row.end_time),
		quorum: BigInt(row.quorum),
		threshold: legacyThreshold,
		flatSupportThreshold: flat,
		partialPercentageSupportThreshold: partial,
		universalPercentageSupportThreshold: BigInt(row.universal_percentage_support_threshold),
		executeBy: row.execute_by !== null ? BigInt(row.execute_by) : null,
		totalEditors: BigInt(row.total_editors),
		executedAt: row.executed_at !== null ? BigInt(row.executed_at) : null,
		yesCount: BigInt(row.yes_count),
		noCount: BigInt(row.no_count),
		abstainCount: BigInt(row.abstain_count),
		actions: mapActionsFromJson(row.actions_json),
	}
}

/**
 * Maps a proposal row to the full domain type with individual voter records.
 * Used by the single-proposal detail query.
 */
function mapRowToProposal(row: BaseProposalRow, votes: Vote[]): ProposalWithVotes {
	return {...mapBaseFields(row), votes}
}

/**
 * Maps a proposal row to a list item with the requesting user's vote.
 * No individual voter records — only aggregate counts.
 */
function mapRowToListItem(row: BaseProposalRow, userVote: VoteOption | null): ProposalListItem {
	return {...mapBaseFields(row), userVote}
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
			// Vote counts use denormalized columns; individual votes fetched via LATERAL
			const result = await db.execute<BaseProposalRow & {votes_json: VoteJsonRow[] | null}>(sql`
        SELECT
          p.id,
          p.space_id,
          p.name,
          p.proposed_by,
          p.proposal_version,
          p.voting_mode,
          p.start_time,
          p.end_time,
          p.quorum,
          p.threshold,
          p.flat_support_threshold,
          p.partial_percentage_support_threshold,
          p.universal_percentage_support_threshold,
          p.execute_by,
          ${sqlTotalEditors()} AS total_editors,
          p.executed_at,
          p.yes_count,
          p.no_count,
          p.abstain_count,
          v.votes_json,
          a.actions_json
        FROM proposals_current p
        LEFT JOIN LATERAL (
          SELECT COALESCE(json_agg(json_build_object(
            'voter_id', voter_id,
            'vote', vote
          )), '[]'::json) as votes_json
          FROM proposal_votes
          WHERE proposal_id = p.id AND proposal_version = p.proposal_version
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
            'duration', duration,
            'partial_percentage_support_threshold', partial_percentage_support_threshold,
            'universal_percentage_support_threshold', universal_percentage_support_threshold,
            'flat_support_threshold', flat_support_threshold,
            'disable_fast_path_access_for_new_members', disable_fast_path_access_for_new_members,
            'execution_grace_period', execution_grace_period
          )), '[]'::json) as actions_json
          FROM proposal_actions
          WHERE proposal_id = p.id AND proposal_version = p.proposal_version
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
 * Sort order options for listing proposals.
 */
export const PROPOSAL_ORDER_BY = ["created_at", "end_time", "start_time"] as const
export type ProposalOrderBy = (typeof PROPOSAL_ORDER_BY)[number]

export const PROPOSAL_ORDER_DIRECTION = ["asc", "desc"] as const
export type ProposalOrderDirection = (typeof PROPOSAL_ORDER_DIRECTION)[number]

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
	/** Filter by computed proposal status */
	status?: ProposalStatus[]
	/** Field to order by (default: created_at) */
	orderBy?: ProposalOrderBy
	/** Sort direction (default: desc) */
	orderDirection?: ProposalOrderDirection
	/** Voter ID to look up the user's vote on each proposal */
	voterId?: string
}

/**
 * Result of listing proposals with cursor for pagination.
 */
export interface ListProposalsResult {
	proposals: ProposalListItem[]
	nextCursor: string | null
}

// =============================================================================
// Shared SQL Status Fragments
// =============================================================================

/**
 * WARNING: Status computation logic in SQL fragments below MUST match
 * computeProposalStatus() in status.ts. Any changes to status computation
 * must be made in BOTH places. Run tests in __tests__/queries.test.ts to
 * verify implementations match.
 *
 * Fragments assume `p` is an alias of `proposals_current` (the view that
 * joins identity + the current version row). Editor-count is read via a
 * correlated subquery against the `space_editor_counts` view so fragments
 * stay self-contained across call sites.
 */

/** Correlated subquery: per-space editor count, 0 if the space has no indexed editors. */
function sqlTotalEditors() {
	return sql`COALESCE((SELECT sec.total_editors FROM space_editor_counts sec WHERE sec.space_id = p.space_id), 0)`
}

/** Proposal is ACCEPTED: already executed. */
function sqlIsAccepted() {
	return sql`(p.executed_at IS NOT NULL)`
}

/**
 * Proposal is EXECUTABLE iff not executed, deadline not passed, and any
 * path produces an executable outcome:
 *   - Fast: yes_count >= flat_support_threshold
 *   - Slow early: voting ongoing AND total_editors > 0 AND universal > 0
 *                 AND yes_count >= ceil(universal * total_editors / RATIO_BASE)
 *   - Slow late:  voting ended AND quorum met
 *                 AND (RATIO_BASE - partial) * yes > partial * no
 */
function sqlIsExecutable(nowSeconds: bigint) {
	return sql`(
		p.executed_at IS NULL
		AND (p.execute_by IS NULL OR ${nowSeconds}::bigint <= p.execute_by)
		AND (
			(p.voting_mode = 'Fast' AND p.yes_count >= p.flat_support_threshold)
			OR (
				p.voting_mode = 'Slow'
				AND ${nowSeconds}::bigint <= p.end_time
				AND ${sqlTotalEditors()} > 0
				AND p.universal_percentage_support_threshold > 0
				AND p.yes_count::numeric >= CEIL(
					(p.universal_percentage_support_threshold::numeric * ${sqlTotalEditors()}::numeric)
					/ ${RATIO_BASE}::numeric
				)
			)
			OR (
				p.voting_mode = 'Slow'
				AND ${nowSeconds}::bigint > p.end_time
				AND (p.yes_count + p.no_count + p.abstain_count) >= p.quorum
				AND (${RATIO_BASE}::numeric - p.partial_percentage_support_threshold::numeric) * p.yes_count::numeric > p.partial_percentage_support_threshold::numeric * p.no_count::numeric
			)
		)
	)`
}

/**
 * Proposal is PROPOSED iff not executed, deadline not passed, voting
 * ongoing, and no executable path is already matched.
 */
function sqlIsProposed(nowSeconds: bigint) {
	return sql`(
		p.executed_at IS NULL
		AND (p.execute_by IS NULL OR ${nowSeconds}::bigint <= p.execute_by)
		AND ${nowSeconds}::bigint <= p.end_time
		AND NOT (
			(p.voting_mode = 'Fast' AND p.yes_count >= p.flat_support_threshold)
			OR (
				p.voting_mode = 'Slow'
				AND ${sqlTotalEditors()} > 0
				AND p.universal_percentage_support_threshold > 0
				AND p.yes_count::numeric >= CEIL(
					(p.universal_percentage_support_threshold::numeric * ${sqlTotalEditors()}::numeric)
					/ ${RATIO_BASE}::numeric
				)
			)
		)
	)`
}

/**
 * Proposal is REJECTED iff not executed and either the executeBy deadline
 * has passed, or voting ended without any executable path matching.
 */
function sqlIsRejected(nowSeconds: bigint) {
	return sql`(
		p.executed_at IS NULL
		AND (
			(p.execute_by IS NOT NULL AND ${nowSeconds}::bigint > p.execute_by)
			OR (
				${nowSeconds}::bigint > p.end_time
				AND NOT (
					(p.voting_mode = 'Fast' AND p.yes_count >= p.flat_support_threshold)
					OR (
						p.voting_mode = 'Slow'
						AND (p.yes_count + p.no_count + p.abstain_count) >= p.quorum
						AND (${RATIO_BASE}::numeric - p.partial_percentage_support_threshold::numeric) * p.yes_count::numeric > p.partial_percentage_support_threshold::numeric * p.no_count::numeric
					)
				)
			)
		)
	)`
}

/**
 * Builds the SQL ORDER BY clause based on orderBy and orderDirection options.
 * Returns the appropriate SQL fragment for ordering.
 */
function buildOrderClause(
	orderBy: ProposalOrderBy = "created_at",
	orderDirection: ProposalOrderDirection = "desc",
): ReturnType<typeof sql> {
	// Map orderBy to actual column and build ORDER BY
	// Using explicit branches to avoid SQL injection - we control the column names
	if (orderDirection === "asc") {
		switch (orderBy) {
			case "end_time":
				return sql`ORDER BY p.end_time ASC, p.id ASC`
			case "start_time":
				return sql`ORDER BY p.start_time ASC, p.id ASC`
			case "created_at":
			default:
				return sql`ORDER BY p.created_at ASC, p.id ASC`
		}
	} else {
		switch (orderBy) {
			case "end_time":
				return sql`ORDER BY p.end_time DESC, p.id DESC`
			case "start_time":
				return sql`ORDER BY p.start_time DESC, p.id DESC`
			case "created_at":
			default:
				return sql`ORDER BY p.created_at DESC, p.id DESC`
		}
	}
}

/** UUID pattern for cursor validation */
const UUID_PATTERN = /^[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{12}$/i

/** Numeric pattern for bigint cursor values */
const BIGINT_PATTERN = /^-?\d+$/

/**
 * Validates the cursor format and returns parsed components.
 * Returns null if cursor is invalid.
 */
function parseCursor(cursor: string, orderBy: ProposalOrderBy): {orderValue: string; cursorId: string} | null {
	const parts = cursor.split("|")
	if (parts.length !== 2) return null

	const [orderValue, cursorId] = parts
	if (!orderValue || !cursorId) return null

	// Validate cursorId is a valid UUID
	if (!UUID_PATTERN.test(cursorId)) return null

	// Validate orderValue format based on orderBy type
	if (orderBy === "created_at") {
		// Should be ISO timestamp - basic format check
		if (!/^\d{4}-\d{2}-\d{2}/.test(orderValue)) return null
	} else {
		// Should be bigint for end_time/start_time
		if (!BIGINT_PATTERN.test(orderValue)) return null
	}

	return {orderValue, cursorId}
}

/**
 * Builds the cursor condition for pagination based on orderBy and direction.
 * Uses (order_column, id) tuple comparison for stable pagination.
 * Returns empty SQL fragment if cursor is missing or invalid.
 */
function buildCursorCondition(
	cursor: string | undefined,
	orderBy: ProposalOrderBy = "created_at",
	orderDirection: ProposalOrderDirection = "desc",
): ReturnType<typeof sql> {
	if (!cursor) return sql``

	const parsed = parseCursor(cursor, orderBy)
	if (!parsed) {
		// Log warning for debugging - invalid cursor format causes pagination to restart from beginning
		console.warn(
			`[queries] Invalid cursor format ignored, restarting pagination: cursor=${cursor}, orderBy=${orderBy}`,
		)
		return sql``
	}

	const {orderValue, cursorId} = parsed

	// Build comparison based on direction (< for DESC, > for ASC)
	// Using tuple comparison for stable pagination with ties
	if (orderDirection === "asc") {
		switch (orderBy) {
			case "end_time":
				return sql`AND (p.end_time, p.id) > (${orderValue}::bigint, ${cursorId}::uuid)`
			case "start_time":
				return sql`AND (p.start_time, p.id) > (${orderValue}::bigint, ${cursorId}::uuid)`
			case "created_at":
			default:
				return sql`AND (p.created_at, p.id) > (${orderValue}::timestamptz, ${cursorId}::uuid)`
		}
	} else {
		switch (orderBy) {
			case "end_time":
				return sql`AND (p.end_time, p.id) < (${orderValue}::bigint, ${cursorId}::uuid)`
			case "start_time":
				return sql`AND (p.start_time, p.id) < (${orderValue}::bigint, ${cursorId}::uuid)`
			case "created_at":
			default:
				return sql`AND (p.created_at, p.id) < (${orderValue}::timestamptz, ${cursorId}::uuid)`
		}
	}
}

/**
 * Extracts the cursor value from a row based on orderBy field.
 * Returns format "order_value|id" for stable pagination.
 */
function extractCursorValue(
	row: BaseProposalRow & {created_at: string},
	orderBy: ProposalOrderBy = "created_at",
): string {
	switch (orderBy) {
		case "end_time":
			return `${row.end_time}|${row.id}`
		case "start_time":
			return `${row.start_time}|${row.id}`
		case "created_at":
		default:
			return `${row.created_at}|${row.id}`
	}
}

/**
 * Checks whether a specific member space has an active (PROPOSED or EXECUTABLE) ADD_MEMBER
 * proposal in the given space. Returns a simple boolean via SELECT EXISTS.
 *
 * @param db - Drizzle database instance
 * @param spaceId - UUID of the space to check
 * @param memberSpaceId - UUID of the member space to check for
 * @returns true if an active ADD_MEMBER proposal exists for this member
 */
export function hasActiveMemberProposal(
	db: Database,
	spaceId: string,
	memberSpaceId: string,
): Effect.Effect<boolean, QueryError> {
	return hasActiveProposalForTarget(db, spaceId, memberSpaceId, "AddMember", "hasActiveMemberProposal")
}

/**
 * Checks whether a specific member space has an active (PROPOSED or EXECUTABLE) ADD_EDITOR
 * proposal in the given space. Returns a simple boolean via SELECT EXISTS.
 *
 * @param db - Drizzle database instance
 * @param spaceId - UUID of the space to check
 * @param editorSpaceId - UUID of the editor space to check for
 * @returns true if an active ADD_EDITOR proposal exists for this editor
 */
export function hasActiveEditorProposal(
	db: Database,
	spaceId: string,
	editorSpaceId: string,
): Effect.Effect<boolean, QueryError> {
	return hasActiveProposalForTarget(db, spaceId, editorSpaceId, "AddEditor", "hasActiveEditorProposal")
}

/**
 * Shared implementation for checking active proposals targeting a specific member/editor.
 * Runs a single SELECT EXISTS query scoped to the action type and target.
 *
 * Uses the shared SQL status fragment builders (sqlIsProposed, sqlIsExecutable)
 * to stay in sync with listProposalsInSpace and computeProposalStatus (status.ts).
 */
function hasActiveProposalForTarget(
	db: Database,
	spaceId: string,
	targetSpaceId: string,
	actionType: "AddMember" | "AddEditor",
	operationName: string,
): Effect.Effect<boolean, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			const nowSeconds = BigInt(Math.floor(Date.now() / 1000))

			const result = await db.execute<{exists: boolean}>(sql`
				SELECT EXISTS (
					SELECT 1 FROM proposals_current p
					WHERE p.space_id = ${spaceId}::uuid
						AND p.executed_at IS NULL
						AND EXISTS (
							SELECT 1 FROM proposal_actions pa
							WHERE pa.proposal_id = p.id
								AND pa.proposal_version = p.proposal_version
								AND pa.action_type = ${actionType}::"proposalActionType"
								AND pa.target_id = ${targetSpaceId}::uuid
						)
						AND (${sqlIsProposed(nowSeconds)} OR ${sqlIsExecutable(nowSeconds)})
				) as exists
			`)

			// SELECT EXISTS always returns exactly 1 row in PostgreSQL.
			// Zero rows would indicate a broken query or connection issue.
			const row = result.rows[0]
			if (!row) {
				throw new Error(`SELECT EXISTS returned ${result.rows.length} rows, expected 1`)
			}
			return row.exists
		},
		catch: (error) =>
			new QueryError({
				operation: operationName,
				cause: error instanceof Error ? error : new Error(String(error)),
			}),
	}).pipe(
		Effect.withSpan(`queries.${operationName}`, {
			attributes: {
				"query.space_id": spaceId,
				"query.target_space_id": targetSpaceId,
			},
		}),
	)
}

/**
 * Lists proposals in a space with optional filtering by action type and status.
 * Uses cursor-based pagination with configurable ordering.
 * Returns vote counts only (not individual voters) for performance.
 *
 * Status filtering is computed in SQL using the same logic as computeProposalStatus:
 * - ACCEPTED: executed_at IS NOT NULL
 * - Fast path EXECUTABLE: yes_count >= threshold
 * - Fast path REJECTED: voting ended and threshold not met
 * - Slow path EXECUTABLE: voting ended, quorum met, and (RATIO_BASE - threshold) * yes > threshold * no
 * - Slow path REJECTED: voting ended and (quorum not met OR threshold not met)
 * - PROPOSED: voting not ended and not yet executable
 *
 * @param db - Drizzle database instance
 * @param options - Query options (spaceId, limit, cursor, actionType, status, orderBy)
 * @returns Paginated list of proposals with vote counts and actions
 */
export function listProposalsInSpace(
	db: Database,
	options: ListProposalsOptions,
): Effect.Effect<ListProposalsResult, QueryError> {
	const {spaceId, limit, cursor, actionTypes, excludeActionTypes, status, orderBy, orderDirection, voterId} = options

	return Effect.tryPromise({
		try: async () => {
			// Build dynamic query conditions
			const cursorCondition = buildCursorCondition(cursor, orderBy, orderDirection)
			const orderClause = buildOrderClause(orderBy, orderDirection)

			// Build action type filter conditions.
			// Use EXISTS subquery instead of JOIN to avoid SQL injection and cartesian products.
			// Scope by proposal_version so historical actions from prior versions don't
			// match the current-version list.
			let actionTypeCondition = sql``
			if (actionTypes && actionTypes.length > 0) {
				// Include filter: proposal must have at least one of these action types
				// Build safe parameterized OR conditions
				const actionTypeChecks = actionTypes.map((t) => sql`pa_filter.action_type = ${t}::"proposalActionType"`)
				const actionTypeOr = actionTypeChecks.reduce((acc, check) => sql`${acc} OR ${check}`)
				actionTypeCondition = sql`AND EXISTS (
          SELECT 1 FROM proposal_actions pa_filter
          WHERE pa_filter.proposal_id = p.id
            AND pa_filter.proposal_version = p.proposal_version
            AND (${actionTypeOr})
        )`
			} else if (excludeActionTypes && excludeActionTypes.length > 0) {
				// Exclude filter: proposal must NOT have any of these action types
				const excludeTypeChecks = excludeActionTypes.map(
					(t) => sql`pa_exclude.action_type = ${t}::"proposalActionType"`,
				)
				const excludeTypeOr = excludeTypeChecks.reduce((acc, check) => sql`${acc} OR ${check}`)
				actionTypeCondition = sql`AND NOT EXISTS (
          SELECT 1 FROM proposal_actions pa_exclude
          WHERE pa_exclude.proposal_id = p.id
            AND pa_exclude.proposal_version = p.proposal_version
            AND (${excludeTypeOr})
        )`
			}

			// Build status filter condition using shared SQL fragment builders.
			// These fragments match the logic in status.ts computeProposalStatus.
			let statusCondition = sql``
			if (status && status.length > 0) {
				const nowSeconds = BigInt(Math.floor(Date.now() / 1000))

				const statusChecks = status.map((s) => {
					switch (s) {
						case "ACCEPTED":
							return sqlIsAccepted()
						case "EXECUTABLE":
							return sqlIsExecutable(nowSeconds)
						case "REJECTED":
							return sqlIsRejected(nowSeconds)
						case "PROPOSED":
							return sqlIsProposed(nowSeconds)
						default: {
							// Exhaustiveness check - TypeScript will error if a new status is added
							const _exhaustive: never = s
							return sql`FALSE`
						}
					}
				})

				const statusOr = statusChecks.reduce((acc, check) => sql`${acc} OR ${check}`)
				statusCondition = sql`AND (${statusOr})`
			}

			// Vote counts are denormalized on the current proposal_version.
			// Individual voters are omitted for performance (use single proposal endpoint for full voter list).
			// When voterId is provided, we fetch just the user's vote on the current version via a LATERAL join.
			const userVoteJoin = voterId
				? sql`LEFT JOIN LATERAL (
            SELECT vote FROM proposal_votes
            WHERE proposal_id = p.id
              AND proposal_version = p.proposal_version
              AND voter_id = ${voterId}::uuid
            LIMIT 1
          ) user_vote ON true`
				: sql``
			const userVoteSelect = voterId ? sql`, user_vote.vote as user_vote` : sql``

			const result = await db.execute<BaseProposalRow & {created_at: string; user_vote?: string | null}>(
				sql`
        SELECT
          p.id,
          p.space_id,
          p.name,
          p.proposed_by,
          p.proposal_version,
          p.voting_mode,
          p.start_time,
          p.end_time,
          p.quorum,
          p.threshold,
          p.flat_support_threshold,
          p.partial_percentage_support_threshold,
          p.universal_percentage_support_threshold,
          p.execute_by,
          ${sqlTotalEditors()} AS total_editors,
          p.executed_at,
          p.created_at,
          p.yes_count,
          p.no_count,
          p.abstain_count,
          actions_agg.actions_json
          ${userVoteSelect}
        FROM proposals_current p
        LEFT JOIN LATERAL (
          SELECT COALESCE(json_agg(json_build_object(
            'action_type', action_type,
            'target_id', target_id,
            'content_uri', content_uri,
            'content_id', encode(content_id, 'hex'),
            'quorum', quorum,
            'fast_threshold', fast_threshold,
            'slow_threshold', slow_threshold,
            'duration', duration,
            'partial_percentage_support_threshold', partial_percentage_support_threshold,
            'universal_percentage_support_threshold', universal_percentage_support_threshold,
            'flat_support_threshold', flat_support_threshold,
            'disable_fast_path_access_for_new_members', disable_fast_path_access_for_new_members,
            'execution_grace_period', execution_grace_period
          )), '[]'::json) as actions_json
          FROM proposal_actions
          WHERE proposal_id = p.id AND proposal_version = p.proposal_version
        ) actions_agg ON true
        ${userVoteJoin}
        WHERE p.space_id = ${spaceId}::uuid
        ${cursorCondition}
        ${actionTypeCondition}
        ${statusCondition}
        ${orderClause}
        LIMIT ${limit + 1}
      `,
			)

			const rows = result.rows
			const hasMore = rows.length > limit
			const proposalRows = hasMore ? rows.slice(0, limit) : rows

			const proposals = proposalRows.map((row) => {
				const rawVote = row.user_vote?.toUpperCase()
				const userVote =
					rawVote && VOTE_OPTIONS.includes(rawVote as VoteOption) ? (rawVote as VoteOption) : null
				return mapRowToListItem(row, userVote)
			})

			// Next cursor is the order value + id of the last item
			const lastRow = proposalRows[proposalRows.length - 1]
			const nextCursor = hasMore && lastRow ? extractCursorValue(lastRow, orderBy) : null

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
				"query.status": status?.join(",") ?? "all",
				"query.order_by": orderBy ?? "created_at",
				"query.order_direction": orderDirection ?? "desc",
			},
		}),
	)
}
