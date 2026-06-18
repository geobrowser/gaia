/**
 * Proposals router.
 *
 * Provides REST endpoints for querying proposal status.
 */

import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Data, Effect, Either} from "effect"
import {Hono} from "hono"
import {describeRoute} from "hono-openapi"

import type {AppRuntime} from "../services/runtime"
import {isValidUuid, normalizeUuid} from "../utils/uuid"
import {
	getProposalWithVotes,
	hasActiveEditorProposal,
	hasActiveMemberProposal,
	listProposalsInSpace,
	PROPOSAL_ORDER_BY,
	PROPOSAL_ORDER_DIRECTION,
	type ProposalOrderBy,
	type ProposalOrderDirection,
	type QueryError,
} from "./queries"
import {computeProposalStatus, getCurrentTimeSeconds} from "./status"
import {
	type ActionResponse,
	PROPOSAL_ACTION_TYPES,
	PROPOSAL_STATUSES,
	type ProposalActionType,
	type ProposalListItem,
	type ProposalListItemResponse,
	type ProposalStatus,
	type ProposalStatusResponse,
	type ProposalWithVotes,
	RATIO_BASE,
	type Vote,
	type VoteOption,
} from "./types"

type Database = NodePgDatabase<Record<string, unknown>>

type AppEnv = {
	Variables: {
		requestId: string
	}
}

class ValidationError extends Data.TaggedError("ValidationError")<{
	message: string
}> {}

class NotFoundError extends Data.TaggedError("NotFoundError")<{
	message: string
}> {}

type ProposalStatusError = ValidationError | NotFoundError | QueryError
type ProposalListError = ValidationError | QueryError

/**
 * Maps an internal ProposalAction to the discriminated union ActionResponse.
 * Each action type has its own shape with only the relevant fields.
 * Returns UNKNOWN if required fields are missing (defensive against bad DB data).
 */
function mapToActionResponse(action: ProposalWithVotes["actions"][number]): ActionResponse {
	switch (action.actionType) {
		case "AddMember":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {
				actionType: "ADD_MEMBER",
				targetId: normalizeUuid(action.targetId),
			}
		case "RemoveMember":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {
				actionType: "REMOVE_MEMBER",
				targetId: normalizeUuid(action.targetId),
			}
		case "AddEditor":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {
				actionType: "ADD_EDITOR",
				targetId: normalizeUuid(action.targetId),
			}
		case "RemoveEditor":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {
				actionType: "REMOVE_EDITOR",
				targetId: normalizeUuid(action.targetId),
			}
		case "UnflagEditor":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {
				actionType: "UNFLAG_EDITOR",
				targetId: normalizeUuid(action.targetId),
			}
		case "Publish":
			if (!action.contentUri) return {actionType: "UNKNOWN"}
			return {
				actionType: "PUBLISH",
				contentUri: action.contentUri,
			}
		case "Flag":
			if (!action.contentId) return {actionType: "UNKNOWN"}
			return {
				actionType: "FLAG",
				contentId: action.contentId,
			}
		case "Unflag":
			if (!action.contentId) return {actionType: "UNKNOWN"}
			return {
				actionType: "UNFLAG",
				contentId: action.contentId,
			}
		case "UpdateVotingSettings":
			if (
				action.quorum == null ||
				action.fastThreshold == null ||
				action.slowThreshold == null ||
				action.duration == null
			) {
				return {actionType: "UNKNOWN"}
			}
			return {
				actionType: "UPDATE_VOTING_SETTINGS",
				quorum: action.quorum,
				fastThreshold: action.fastThreshold,
				slowThreshold: action.slowThreshold,
				duration: action.duration,
				partialPercentageSupportThreshold: action.partialPercentageSupportThreshold,
				universalPercentageSupportThreshold: action.universalPercentageSupportThreshold,
				flatSupportThreshold: action.flatSupportThreshold,
				disableFastPathAccessForNewMembers: action.disableFastPathAccessForNewMembers,
				executionGracePeriod: action.executionGracePeriod,
			}
		case "SubspaceVerified":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {actionType: "SUBSPACE_VERIFIED", targetSpaceId: action.targetId}
		case "SubspaceUnverified":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {actionType: "SUBSPACE_UNVERIFIED", targetSpaceId: action.targetId}
		case "SubspaceRelated":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {actionType: "SUBSPACE_RELATED", targetSpaceId: action.targetId}
		case "SubspaceUnrelated":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {actionType: "SUBSPACE_UNRELATED", targetSpaceId: action.targetId}
		case "SubspaceTopicDeclared":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {actionType: "SUBSPACE_TOPIC_DECLARED", targetTopicId: action.targetId}
		case "SubspaceTopicRemoved":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {actionType: "SUBSPACE_TOPIC_REMOVED", targetTopicId: action.targetId}
		case "SetTopic":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {actionType: "SET_TOPIC", targetTopicId: action.targetId}
		case "UnsetTopic":
			if (!action.targetId) return {actionType: "UNKNOWN"}
			return {actionType: "UNSET_TOPIC", targetTopicId: action.targetId}
		default:
			return {actionType: "UNKNOWN"}
	}
}

/**
 * Computes the shared response fields from a proposal domain object.
 * Used by both the detail and list response builders.
 */
function computeResponseFields(proposal: ProposalWithVotes | ProposalListItem, nowSeconds: bigint) {
	const {status, isQuorumReached, isThresholdReached} = computeProposalStatus(proposal, nowSeconds)

	const now = Number(nowSeconds)
	const endTime = Number(proposal.endTime)
	const isVotingEnded = now > endTime
	const timeRemaining = isVotingEnded ? null : endTime - now

	const totalVotes = proposal.yesCount + proposal.noCount + proposal.abstainCount
	const quorumRequired = Number(proposal.quorum)
	const quorumCurrent = Number(totalVotes)
	const quorumProgress = quorumRequired > 0 ? Math.min(quorumCurrent / quorumRequired, 1) : 1

	const thresholdRequired = proposal.threshold
	let thresholdCurrent: number
	let thresholdProgress: number

	if (proposal.votingMode === "Fast") {
		thresholdCurrent = Number(proposal.yesCount)
		thresholdProgress = thresholdRequired > 0n ? Math.min(thresholdCurrent / Number(thresholdRequired), 1) : 1
	} else {
		const yesVotes = Number(proposal.yesCount)
		const noVotes = Number(proposal.noCount)
		thresholdCurrent = yesVotes

		if (yesVotes + noVotes === 0) {
			thresholdProgress = 0
		} else {
			const requiredYesRatio = 1 - Number(thresholdRequired) / Number(RATIO_BASE)
			const actualYesRatio = yesVotes / (yesVotes + noVotes)
			thresholdProgress = requiredYesRatio > 0 ? Math.min(actualYesRatio / requiredYesRatio, 1) : 1
		}
	}

	return {
		proposalId: normalizeUuid(proposal.id),
		spaceId: normalizeUuid(proposal.spaceId),
		name: proposal.name,
		proposedBy: normalizeUuid(proposal.proposedBy),
		proposalVersion: proposal.proposalVersion,
		executeBy: proposal.executeBy !== null ? Number(proposal.executeBy) : null,
		status,
		votingMode: proposal.votingMode.toUpperCase() as "FAST" | "SLOW",
		actions: proposal.actions.map(mapToActionResponse),
		votes: {
			yes: Number(proposal.yesCount),
			no: Number(proposal.noCount),
			abstain: Number(proposal.abstainCount),
			total: Number(totalVotes),
		},
		quorum: {
			required: quorumRequired,
			current: quorumCurrent,
			progress: quorumProgress,
			reached: isQuorumReached,
		},
		threshold: {
			required: thresholdRequired.toString(),
			current: thresholdCurrent,
			progress: thresholdProgress,
			reached: isThresholdReached,
		},
		timing: {
			startTime: Number(proposal.startTime),
			endTime,
			timeRemaining,
			isVotingEnded,
		},
		canExecute: status === "EXECUTABLE",
	}
}

/**
 * Builds the full detail response for a single proposal.
 * Includes individual voter records and resolves userVote from them.
 */
function buildProposalResponse(
	proposal: ProposalWithVotes,
	nowSeconds: bigint,
	voterId?: string,
): ProposalStatusResponse {
	const base = computeResponseFields(proposal, nowSeconds)

	const voters: Vote[] = proposal.votes.map((v) => ({
		voterId: normalizeUuid(v.voterId),
		vote: v.vote,
	}))

	let userVote: VoteOption | null = null
	if (voterId) {
		const normalizedVoterId = normalizeUuid(voterId)
		const userVoteRecord = proposal.votes.find((v) => normalizeUuid(v.voterId) === normalizedVoterId)
		userVote = userVoteRecord?.vote ?? null
	}

	return {
		...base,
		votes: {...base.votes, voters},
		userVote,
	}
}

/**
 * Builds a list item response for a proposal.
 * No individual voter records — userVote comes directly from the query.
 */
function buildListItemResponse(proposal: ProposalListItem, nowSeconds: bigint): ProposalListItemResponse {
	const base = computeResponseFields(proposal, nowSeconds)
	return {
		...base,
		userVote: proposal.userVote,
	}
}

/**
 * Parse and validate a comma-separated list of action types.
 * Returns an Effect that fails with ValidationError on invalid input.
 */
function parseActionTypes(param: string | undefined): Effect.Effect<ProposalActionType[] | undefined, ValidationError> {
	if (!param) return Effect.succeed(undefined)
	const types = param.split(",").map((t) => t.trim())
	const invalid = types.filter((t) => !PROPOSAL_ACTION_TYPES.includes(t as ProposalActionType))
	if (invalid.length > 0) {
		return Effect.fail(
			new ValidationError({
				message: `Invalid action types: ${invalid.join(", ")}. Valid: ${PROPOSAL_ACTION_TYPES.join(", ")}`,
			}),
		)
	}
	return Effect.succeed(types as ProposalActionType[])
}

/**
 * Parse and validate a comma-separated list of proposal statuses.
 * Returns an Effect that fails with ValidationError on invalid input.
 */
function parseStatuses(param: string | undefined): Effect.Effect<ProposalStatus[] | undefined, ValidationError> {
	if (!param) return Effect.succeed(undefined)
	const statuses = param.split(",").map((s) => s.trim().toUpperCase())
	const invalid = statuses.filter((s) => !PROPOSAL_STATUSES.includes(s as ProposalStatus))
	if (invalid.length > 0) {
		return Effect.fail(
			new ValidationError({
				message: `Invalid statuses: ${invalid.join(", ")}. Valid: ${PROPOSAL_STATUSES.join(", ")}`,
			}),
		)
	}
	return Effect.succeed(statuses as ProposalStatus[])
}

/**
 * Parse and validate orderBy parameter.
 * Returns an Effect that fails with ValidationError on invalid input.
 */
function parseOrderBy(param: string | undefined): Effect.Effect<ProposalOrderBy | undefined, ValidationError> {
	if (!param) return Effect.succeed(undefined)
	const orderBy = param.trim().toLowerCase()
	if (!PROPOSAL_ORDER_BY.includes(orderBy as ProposalOrderBy)) {
		return Effect.fail(
			new ValidationError({
				message: `Invalid orderBy: ${param}. Valid: ${PROPOSAL_ORDER_BY.join(", ")}`,
			}),
		)
	}
	return Effect.succeed(orderBy as ProposalOrderBy)
}

/**
 * Parse and validate orderDirection parameter.
 * Returns an Effect that fails with ValidationError on invalid input.
 */
function parseOrderDirection(
	param: string | undefined,
): Effect.Effect<ProposalOrderDirection | undefined, ValidationError> {
	if (!param) return Effect.succeed(undefined)
	const direction = param.trim().toLowerCase()
	if (!PROPOSAL_ORDER_DIRECTION.includes(direction as ProposalOrderDirection)) {
		return Effect.fail(
			new ValidationError({
				message: `Invalid orderDirection: ${param}. Valid: ${PROPOSAL_ORDER_DIRECTION.join(", ")}`,
			}),
		)
	}
	return Effect.succeed(direction as ProposalOrderDirection)
}

/**
 * Create the proposals router.
 */
export function createProposalsRouter(db: Database, runtime: AppRuntime) {
	const router = new Hono<AppEnv>()

	// GET /proposals/:id/status - Single proposal status
	router.get(
		"/:id/status",
		describeRoute({
			tags: ["Proposals"],
			summary: "Get proposal status",
			description: "Computes proposal status matching the smart contract's isSupportThresholdReached() logic.",
			parameters: [
				{
					name: "id",
					in: "path",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "voterId",
					in: "query",
					description: "UUID of the voter to check for their vote",
					schema: {type: "string", format: "uuid"},
				},
			],
			responses: {
				200: {
					description: "Proposal status",
					content: {
						"application/json": {
							schema: {$ref: "#/components/schemas/ProposalStatusResponse"},
						},
					},
				},
				400: {description: "Invalid parameter"},
				404: {description: "Proposal not found"},
				500: {description: "Internal server error"},
			},
		}),
		async (c) => {
			const proposalId = c.req.param("id")
			const voterId = c.req.query("voterId")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				yield* Effect.logInfo("GetProposalStatus started", {
					proposalId,
					voterId,
				})

				if (!isValidUuid(proposalId)) {
					return yield* Effect.fail(
						new ValidationError({
							message: "Proposal ID must be a valid UUID",
						}),
					)
				}

				if (voterId && !isValidUuid(voterId)) {
					return yield* Effect.fail(
						new ValidationError({
							message: "Voter ID must be a valid UUID",
						}),
					)
				}

				const proposal = yield* getProposalWithVotes(db, proposalId)

				if (!proposal) {
					return yield* Effect.fail(
						new NotFoundError({
							message: `Proposal '${proposalId}' not found`,
						}),
					)
				}

				const nowSeconds = getCurrentTimeSeconds()
				return buildProposalResponse(proposal, nowSeconds, voterId)
			}).pipe(
				Effect.tapError((error) => {
					switch (error._tag) {
						case "QueryError":
							return Effect.logError("GetProposalStatus failed", {
								errorType: "database_error",
								operation: error.operation,
								message: error.cause.message,
							})
						case "ValidationError":
							return Effect.logWarning("GetProposalStatus failed", {
								errorType: "validation_error",
								message: error.message,
							})
						case "NotFoundError":
							return Effect.logInfo("GetProposalStatus failed", {
								errorType: "not_found",
								message: error.message,
							})
					}
				}),
				Effect.withSpan("GET /proposals/:id/status"),
				Effect.annotateLogs({requestId, proposalId}),
				Effect.annotateSpans({requestId, proposalId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: ProposalStatusError) => {
					switch (error._tag) {
						case "ValidationError":
							return c.json({error: "Invalid parameter", message: error.message}, 400)
						case "NotFoundError":
							return c.json({error: "Not found", message: error.message}, 404)
						case "QueryError":
							return c.json(
								{
									error: "Internal server error",
									message: "An unexpected error occurred",
								},
								500,
							)
					}
				},
				onRight: (response) => c.json(response),
			})
		},
	)

	// GET /proposals/space/:spaceId/status - List proposals in a space
	router.get(
		"/space/:spaceId/status",
		describeRoute({
			tags: ["Proposals"],
			summary: "List proposal statuses in a space",
			description:
				"Lists proposals with cursor pagination. Filter with actionTypes, excludeActionTypes, or status (comma-separated). Sort with orderBy and orderDirection.",
			parameters: [
				{
					name: "spaceId",
					in: "path",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "limit",
					in: "query",
					schema: {type: "integer", minimum: 1, maximum: 100, default: 20},
				},
				{name: "cursor", in: "query", schema: {type: "string"}},
				{
					name: "actionTypes",
					in: "query",
					description: "Include only these action types (comma-separated)",
					schema: {type: "string"},
				},
				{
					name: "excludeActionTypes",
					in: "query",
					description: "Exclude these action types (comma-separated)",
					schema: {type: "string"},
				},
				{
					name: "status",
					in: "query",
					description:
						"Filter by proposal status (comma-separated): PROPOSED, EXECUTABLE, ACCEPTED, REJECTED",
					schema: {type: "string"},
				},
				{
					name: "orderBy",
					in: "query",
					description: "Field to order by: created_at (default), end_time, start_time",
					schema: {
						type: "string",
						enum: ["created_at", "end_time", "start_time"],
					},
				},
				{
					name: "orderDirection",
					in: "query",
					description: "Sort direction: desc (default), asc",
					schema: {type: "string", enum: ["asc", "desc"]},
				},
				{
					name: "voterId",
					in: "query",
					description: "UUID of the voter to check for their vote on each proposal",
					schema: {type: "string", format: "uuid"},
				},
			],
			responses: {
				200: {
					description: "List of proposal statuses",
					content: {
						"application/json": {
							schema: {$ref: "#/components/schemas/ProposalListResponse"},
						},
					},
				},
				400: {description: "Invalid parameter"},
				500: {description: "Internal server error"},
			},
		}),
		async (c) => {
			const spaceId = c.req.param("spaceId")
			const requestId = c.get("requestId") ?? "unknown"
			const limitParam = c.req.query("limit")
			const cursor = c.req.query("cursor")
			const actionTypesParam = c.req.query("actionTypes")
			const excludeActionTypesParam = c.req.query("excludeActionTypes")
			const statusParam = c.req.query("status")
			const orderByParam = c.req.query("orderBy")
			const orderDirectionParam = c.req.query("orderDirection")
			const voterId = c.req.query("voterId")

			const program = Effect.gen(function* () {
				yield* Effect.logInfo("ListProposalStatuses started", {
					spaceId,
					limit: limitParam,
					cursor,
					actionTypes: actionTypesParam,
					excludeActionTypes: excludeActionTypesParam,
					status: statusParam,
					orderBy: orderByParam,
					orderDirection: orderDirectionParam,
					voterId,
				})

				if (!isValidUuid(spaceId)) {
					return yield* Effect.fail(new ValidationError({message: "Space ID must be a valid UUID"}))
				}

				if (voterId && !isValidUuid(voterId)) {
					return yield* Effect.fail(new ValidationError({message: "Voter ID must be a valid UUID"}))
				}

				const limit = limitParam ? parseInt(limitParam, 10) : 20
				if (Number.isNaN(limit) || limit < 1 || limit > 100) {
					return yield* Effect.fail(new ValidationError({message: "Limit must be between 1 and 100"}))
				}

				const actionTypes = yield* parseActionTypes(actionTypesParam)
				const excludeActionTypes = yield* parseActionTypes(excludeActionTypesParam)

				// Validate that actionTypes and excludeActionTypes are mutually exclusive
				if (actionTypes && excludeActionTypes) {
					return yield* Effect.fail(
						new ValidationError({
							message: "Cannot specify both actionTypes and excludeActionTypes. Use one or the other.",
						}),
					)
				}

				const status = yield* parseStatuses(statusParam)
				const orderBy = yield* parseOrderBy(orderByParam)
				const orderDirection = yield* parseOrderDirection(orderDirectionParam)

				const normalizedVoterId = voterId ? normalizeUuid(voterId) : undefined
				const {proposals, nextCursor} = yield* listProposalsInSpace(db, {
					spaceId,
					limit,
					cursor,
					actionTypes,
					excludeActionTypes,
					status,
					orderBy,
					orderDirection,
					voterId: normalizedVoterId,
				})

				const nowSeconds = getCurrentTimeSeconds()
				const proposalResponses = proposals.map((p) => buildListItemResponse(p, nowSeconds))

				return {
					proposals: proposalResponses,
					nextCursor,
				}
			}).pipe(
				Effect.tapError((error) => {
					switch (error._tag) {
						case "QueryError":
							return Effect.logError("ListProposalStatuses failed", {
								errorType: "database_error",
								operation: error.operation,
								message: error.cause.message,
							})
						case "ValidationError":
							return Effect.logWarning("ListProposalStatuses failed", {
								errorType: "validation_error",
								message: error.message,
							})
					}
				}),
				Effect.withSpan("GET /proposals/space/:spaceId/status"),
				Effect.annotateLogs({requestId, spaceId}),
				Effect.annotateSpans({requestId, spaceId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: ProposalListError) => {
					switch (error._tag) {
						case "ValidationError":
							return c.json({error: "Invalid parameter", message: error.message}, 400)
						case "QueryError":
							return c.json(
								{
									error: "Internal server error",
									message: "An unexpected error occurred",
								},
								500,
							)
					}
				},
				onRight: (response) => c.json(response),
			})
		},
	)

	// Active proposal check endpoints — shared handler with per-route config
	registerActiveProposalRoute(router, db, runtime, {
		path: "/space/:spaceId/members/:memberSpaceId/active",
		targetParam: "memberSpaceId",
		targetLabel: "Member space ID",
		summary: "Check if a member has an active ADD_MEMBER proposal",
		description:
			"Returns whether the specified member space has an active (PROPOSED or EXECUTABLE) ADD_MEMBER proposal in the given space.",
		queryFn: hasActiveMemberProposal,
		operationName: "HasActiveMemberProposal",
	})

	registerActiveProposalRoute(router, db, runtime, {
		path: "/space/:spaceId/editors/:editorSpaceId/active",
		targetParam: "editorSpaceId",
		targetLabel: "Editor space ID",
		summary: "Check if a member has an active ADD_EDITOR proposal",
		description:
			"Returns whether the specified member space has an active (PROPOSED or EXECUTABLE) ADD_EDITOR proposal in the given space.",
		queryFn: hasActiveEditorProposal,
		operationName: "HasActiveEditorProposal",
	})

	return router
}

// =============================================================================
// Active Proposal Check — Shared Route Handler
// =============================================================================

interface ActiveProposalRouteConfig {
	/** Route path pattern, e.g. "/space/:spaceId/members/:memberSpaceId/active" */
	path: string
	/** Name of the target path parameter, e.g. "memberSpaceId" */
	targetParam: string
	/** Human-readable label for the target param in validation errors */
	targetLabel: string
	/** OpenAPI summary */
	summary: string
	/** OpenAPI description */
	description: string
	/** Query function to call with (db, spaceId, targetId) */
	queryFn: (db: Database, spaceId: string, targetId: string) => Effect.Effect<boolean, QueryError>
	/** Operation name for logs and spans, e.g. "HasActiveMemberProposal" */
	operationName: string
}

/**
 * Registers a GET route that checks whether an active proposal exists for a
 * specific target. Both the member and editor active-check endpoints share
 * this handler — only the config differs.
 */
function registerActiveProposalRoute(
	router: Hono<AppEnv>,
	db: Database,
	runtime: AppRuntime,
	config: ActiveProposalRouteConfig,
) {
	router.get(
		config.path,
		describeRoute({
			tags: ["Proposals"],
			summary: config.summary,
			description: config.description,
			parameters: [
				{
					name: "spaceId",
					in: "path",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: config.targetParam,
					in: "path",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
			],
			responses: {
				200: {
					description: "Active proposal check result",
					content: {
						"application/json": {
							schema: {$ref: "#/components/schemas/ActiveProposalCheckResponse"},
						},
					},
				},
				400: {description: "Invalid parameter"},
				500: {description: "Internal server error"},
			},
		}),
		async (c) => {
			// Path params are always present for matched routes. The dynamic lookup
			// returns string | undefined because Hono can't narrow from a variable key.
			const spaceId = c.req.param("spaceId") as string
			const targetId = c.req.param(config.targetParam) as string
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				yield* Effect.logInfo(`${config.operationName} started`, {
					spaceId,
					[config.targetParam]: targetId,
				})

				if (!isValidUuid(spaceId)) {
					return yield* Effect.fail(new ValidationError({message: "Space ID must be a valid UUID"}))
				}
				if (!isValidUuid(targetId)) {
					return yield* Effect.fail(
						new ValidationError({message: `${config.targetLabel} must be a valid UUID`}),
					)
				}

				const active = yield* config.queryFn(db, spaceId, targetId)
				return {active}
			}).pipe(
				Effect.tapError((error) => {
					switch (error._tag) {
						case "QueryError":
							return Effect.logError(`${config.operationName} failed`, {
								errorType: "database_error",
								operation: error.operation,
								message: error.cause.message,
							})
						case "ValidationError":
							return Effect.logWarning(`${config.operationName} failed`, {
								errorType: "validation_error",
								message: error.message,
							})
					}
				}),
				Effect.withSpan(`GET /proposals${config.path}`),
				Effect.annotateLogs({requestId, spaceId, [config.targetParam]: targetId}),
				Effect.annotateSpans({requestId, spaceId, [config.targetParam]: targetId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: ProposalListError) => {
					switch (error._tag) {
						case "ValidationError":
							return c.json({error: "Invalid parameter", message: error.message}, 400)
						case "QueryError":
							return c.json(
								{
									error: "Internal server error",
									message: "An unexpected error occurred",
								},
								500,
							)
					}
				},
				onRight: (response) => c.json(response),
			})
		},
	)
}
