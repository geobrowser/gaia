/**
 * Types for proposal status computation.
 *
 * Uses bigint for all contract-derived numeric values to prevent overflow
 * and match smart contract precision.
 */

/**
 * RATIO_BASE constant from the smart contract.
 * Used in percentage threshold calculations.
 */
export const RATIO_BASE = 10_000_000n

/**
 * Proposal action types matching the database enum.
 */
export const PROPOSAL_ACTION_TYPES = [
	"AddMember",
	"RemoveMember",
	"AddEditor",
	"RemoveEditor",
	"UnflagEditor",
	"Publish",
	"Flag",
	"Unflag",
	"UpdateVotingSettings",
	"Unknown",
	// Subspace proposal actions
	"SubspaceVerified",
	"SubspaceUnverified",
	"SubspaceRelated",
	"SubspaceUnrelated",
	"SubspaceTopicDeclared",
	"SubspaceTopicRemoved",
	"SetTopic",
	"UnsetTopic",
] as const
export type ProposalActionType = (typeof PROPOSAL_ACTION_TYPES)[number]

/**
 * Proposal status values matching the governance contract states.
 */
export const PROPOSAL_STATUSES = ["PROPOSED", "EXECUTABLE", "ACCEPTED", "REJECTED"] as const
export type ProposalStatus = (typeof PROPOSAL_STATUSES)[number]

/**
 * Voting modes supported by the governance contract.
 * - Fast: Uses flat threshold (absolute yes votes needed)
 * - Slow: Uses percentage threshold with quorum, voting must end first
 */
export const VOTING_MODES = ["Fast", "Slow"] as const
export type VotingMode = (typeof VOTING_MODES)[number]

/**
 * Vote option values matching the database enum.
 */
export const VOTE_OPTIONS = ["YES", "NO", "ABSTAIN"] as const
export type VoteOption = (typeof VOTE_OPTIONS)[number]

/**
 * Individual vote from a voter.
 */
export interface Vote {
	voterId: string
	vote: VoteOption
}

/**
 * Proposal action with its type and optional payload.
 * Internal type that includes all possible fields from the database.
 */
export interface ProposalAction {
	actionType: ProposalActionType
	/** Target entity ID (e.g., member being added/removed) */
	targetId: string | null
	/** IPFS URI for publish actions */
	contentUri: string | null
	/** Content ID for flag/unflag actions (hex-encoded bytes) */
	contentId: string | null
	/** New quorum for UpdateVotingSettings */
	quorum: number | null
	/** New fast threshold for UpdateVotingSettings (legacy: == flatSupportThreshold) */
	fastThreshold: number | null
	/** New slow threshold for UpdateVotingSettings (legacy: == partialPercentageSupportThreshold) */
	slowThreshold: number | null
	/** New duration for UpdateVotingSettings */
	duration: number | null
	/** V2: slow-path late-execution threshold (percentage of RATIO_BASE) */
	partialPercentageSupportThreshold: number | null
	/** V2: slow-path early-execution threshold (percentage of RATIO_BASE) */
	universalPercentageSupportThreshold: number | null
	/** V2: fast-path threshold (absolute yes-count) */
	flatSupportThreshold: number | null
	/** V2: whether newly added members get fast-path access */
	disableFastPathAccessForNewMembers: boolean | null
	/** V2: grace period in seconds applied after voting ends */
	executionGracePeriod: number | null
}

/**
 * Domain type for a proposal with aggregated vote counts.
 * Uses bigint for contract values to prevent overflow.
 */
export interface ProposalWithVotes {
	id: string
	spaceId: string
	/** Human-readable name derived from proposal actions */
	name: string | null
	/** Member space ID of the proposer */
	proposedBy: string
	/** Proposal version number (incremented on each update; starts at 1) */
	proposalVersion: number
	votingMode: VotingMode
	/** Unix timestamp in seconds when voting starts */
	startTime: bigint
	/** Unix timestamp in seconds when voting ends */
	endTime: bigint
	/** Minimum total votes required (for slow path) */
	quorum: bigint
	/**
	 * Legacy threshold compatibility projection.
	 *
	 * For Fast proposals: mirrors `flatSupportThreshold`.
	 * For Slow proposals: mirrors `partialPercentageSupportThreshold`.
	 *
	 * New code should read the per-mode V2 field directly.
	 */
	threshold: bigint
	/** V2: fast-path threshold (absolute yes-count needed to pass) */
	flatSupportThreshold: bigint
	/** V2: slow-path late-execution threshold (percentage of RATIO_BASE) */
	partialPercentageSupportThreshold: bigint
	/** V2: slow-path early-execution threshold (percentage of RATIO_BASE) */
	universalPercentageSupportThreshold: bigint
	/** V2: execution deadline (Unix seconds) — past this, the proposal is REJECTED. Null on legacy rows. */
	executeBy: bigint | null
	/** V2: total editor count for the proposal's space, used by the slow-path early-execution formula */
	totalEditors: bigint
	/** Unix timestamp when executed, null if not executed */
	executedAt: bigint | null
	/** Number of yes votes */
	yesCount: bigint
	/** Number of no votes */
	noCount: bigint
	/** Number of abstain votes */
	abstainCount: bigint
	/** Individual votes from voters */
	votes: Vote[]
	/** Actions in this proposal */
	actions: ProposalAction[]
}

/**
 * Domain type for a proposal in a list query.
 * No individual voter records — only aggregate counts and the requesting user's vote.
 */
export interface ProposalListItem extends Omit<ProposalWithVotes, "votes"> {
	/** The requesting user's vote, if voterId was provided and they voted */
	userVote: VoteOption | null
}

/**
 * Result of status computation including intermediate flags.
 * Used internally and returned in API response.
 */
export interface StatusComputationResult {
	status: ProposalStatus
	isQuorumReached: boolean
	isThresholdReached: boolean
	/**
	 * True when a Slow-mode proposal became EXECUTABLE *before* its voting
	 * period ended via the slow-path early-execution formula
	 * (`yesCount >= ceil(universalPercentageSupportThreshold * totalEditors / RATIO_BASE)`).
	 * Always false for Fast-mode and for Slow proposals that execute after
	 * voting ends.
	 */
	isEarlyExecutable: boolean
}

// =============================================================================
// Action Response Types (Discriminated Union)
// =============================================================================

/**
 * Action to add a member to the space.
 */
export interface AddMemberAction {
	actionType: "ADD_MEMBER"
	/** Member space ID of the user being added */
	targetId: string
}

/**
 * Action to remove a member from the space.
 */
export interface RemoveMemberAction {
	actionType: "REMOVE_MEMBER"
	/** Member space ID of the user being removed */
	targetId: string
}

/**
 * Action to add an editor to the space.
 */
export interface AddEditorAction {
	actionType: "ADD_EDITOR"
	/** Member space ID of the user being granted editor role */
	targetId: string
}

/**
 * Action to remove an editor from the space.
 */
export interface RemoveEditorAction {
	actionType: "REMOVE_EDITOR"
	/** Member space ID of the user having editor role revoked */
	targetId: string
}

/**
 * Action to unflag an editor (restore their editing privileges after being flagged).
 */
export interface UnflagEditorAction {
	actionType: "UNFLAG_EDITOR"
	/** Member space ID of the editor being unflagged */
	targetId: string
}

/**
 * Action to publish content to the space.
 */
export interface PublishAction {
	actionType: "PUBLISH"
	/** IPFS URI of the content being published */
	contentUri: string
}

/**
 * Action to flag content for review/removal.
 */
export interface FlagAction {
	actionType: "FLAG"
	/** Content ID being flagged (hex-encoded bytes) */
	contentId: string
}

/**
 * Action to unflag previously flagged content.
 */
export interface UnflagAction {
	actionType: "UNFLAG"
	/** Content ID being unflagged (hex-encoded bytes) */
	contentId: string
}

/**
 * Action to update the space's voting settings.
 */
export interface UpdateVotingSettingsAction {
	actionType: "UPDATE_VOTING_SETTINGS"
	/** New minimum total votes required for slow path */
	quorum: number
	/** New threshold for fast path (legacy: == flatSupportThreshold) */
	fastThreshold: number
	/** New threshold for slow path (legacy: == partialPercentageSupportThreshold) */
	slowThreshold: number
	/** New voting duration in seconds */
	duration: number
	/** V2: slow-path late-execution threshold (percentage of RATIO_BASE) */
	partialPercentageSupportThreshold: number | null
	/** V2: slow-path early-execution threshold (percentage of RATIO_BASE) */
	universalPercentageSupportThreshold: number | null
	/** V2: fast-path absolute yes-count threshold */
	flatSupportThreshold: number | null
	/** V2: whether newly added members get fast-path access */
	disableFastPathAccessForNewMembers: boolean | null
	/** V2: grace period in seconds applied after voting ends */
	executionGracePeriod: number | null
}

/**
 * Action to add/remove a verified or related subspace edge.
 * The `actionType` discriminates the specific operation.
 */
export interface SubspaceEdgeAction {
	actionType: "SUBSPACE_VERIFIED" | "SUBSPACE_UNVERIFIED" | "SUBSPACE_RELATED" | "SUBSPACE_UNRELATED"
	/** Target child space ID */
	targetSpaceId: string
}

/**
 * Action to declare or remove a topic on a subspace.
 * The `actionType` discriminates the specific operation.
 */
export interface SubspaceTopicAction {
	actionType: "SUBSPACE_TOPIC_DECLARED" | "SUBSPACE_TOPIC_REMOVED"
	/** Topic entity ID */
	targetTopicId: string
}

/**
 * Action to set the current space topic.
 */
export interface SetTopicAction {
	actionType: "SET_TOPIC"
	/** Topic entity ID */
	targetTopicId: string
}

/**
 * Action to unset the current space topic.
 */
export interface UnsetTopicAction {
	actionType: "UNSET_TOPIC"
	/** Topic entity ID */
	targetTopicId: string
}

/**
 * Unknown action type - used for forward compatibility.
 */
export interface UnknownAction {
	actionType: "UNKNOWN"
}

/**
 * Discriminated union of all possible proposal actions.
 * Use the `actionType` field to narrow the type.
 *
 * @example
 * ```typescript
 * function handleAction(action: ActionResponse) {
 *   switch (action.actionType) {
 *     case "ADD_MEMBER":
 *       console.log(`Adding member: ${action.targetId}`);
 *       break;
 *     case "PUBLISH":
 *       console.log(`Publishing: ${action.contentUri}`);
 *       break;
 *     // ... handle other action types
 *   }
 * }
 * ```
 */
export type ActionResponse =
	| AddMemberAction
	| RemoveMemberAction
	| AddEditorAction
	| RemoveEditorAction
	| UnflagEditorAction
	| PublishAction
	| FlagAction
	| UnflagAction
	| UpdateVotingSettingsAction
	| SubspaceEdgeAction
	| SubspaceTopicAction
	| SetTopicAction
	| UnsetTopicAction
	| UnknownAction

/**
 * Shared fields between detail and list proposal responses.
 */
interface ProposalResponseBase {
	proposalId: string
	spaceId: string
	name: string | null
	/** Member space ID of the proposer (dashless UUID) */
	proposedBy: string
	/** Proposal version number (incremented on each update; starts at 1) */
	proposalVersion: number
	/** Execution deadline (Unix seconds) — past this, the proposal is REJECTED. Null on legacy rows. */
	executeBy: number | null
	status: ProposalStatus
	votingMode: "FAST" | "SLOW"
	/** Actions in this proposal */
	actions: ActionResponse[]
	/** Current user's vote if voterId query param was provided */
	userVote: VoteOption | null
	quorum: {
		/** Required votes for quorum */
		required: number
		/** Current total votes (yes + no + abstain) */
		current: number
		/** Progress as decimal (0.0 to 1.0+), capped at 1.0 for display */
		progress: number
		reached: boolean
	}
	threshold: {
		/** Required threshold value (interpretation depends on votingMode) */
		required: string
		/**
		 * For Fast path: current yes votes
		 * For Slow path: effective yes percentage accounting for the formula
		 */
		current: number
		/** Progress as decimal (0.0 to 1.0+), capped at 1.0 for display */
		progress: number
		reached: boolean
	}
	timing: {
		startTime: number
		endTime: number
		timeRemaining: number | null
		isVotingEnded: boolean
	}
	canExecute: boolean
}

/**
 * API response for single proposal detail endpoint.
 * Includes individual voter records.
 */
export interface ProposalStatusResponse extends ProposalResponseBase {
	votes: {
		yes: number
		no: number
		abstain: number
		total: number
		/** Individual votes from voters */
		voters: Vote[]
	}
}

/**
 * API response for proposals in a list.
 * Omits individual voter records for performance — only aggregate counts.
 */
export interface ProposalListItemResponse extends ProposalResponseBase {
	votes: {
		yes: number
		no: number
		abstain: number
		total: number
	}
}

/**
 * API response for listing proposals in a space.
 */
export interface ProposalListResponse {
	proposals: ProposalListItemResponse[]
	nextCursor: string | null
}
