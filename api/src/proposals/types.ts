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
export const RATIO_BASE = 10_000_000n;

/**
 * Proposal status values matching the governance contract states.
 */
export const PROPOSAL_STATUSES = [
  "PROPOSED",
  "EXECUTABLE",
  "ACCEPTED",
  "REJECTED",
] as const;
export type ProposalStatus = (typeof PROPOSAL_STATUSES)[number];

/**
 * Voting modes supported by the governance contract.
 * - Fast: Uses flat threshold (absolute yes votes needed)
 * - Slow: Uses percentage threshold with quorum, voting must end first
 */
export const VOTING_MODES = ["Fast", "Slow"] as const;
export type VotingMode = (typeof VOTING_MODES)[number];

/**
 * Domain type for a proposal with aggregated vote counts.
 * Uses bigint for contract values to prevent overflow.
 */
export interface ProposalWithVotes {
  id: string;
  spaceId: string;
  proposedBy: string;
  votingMode: VotingMode;
  /** Unix timestamp in seconds when voting starts */
  startTime: bigint;
  /** Unix timestamp in seconds when voting ends */
  endTime: bigint;
  /** Minimum total votes required (for slow path) */
  quorum: bigint;
  /** Threshold for passing - interpretation depends on votingMode */
  threshold: bigint;
  /** Unix timestamp when executed, null if not executed */
  executedAt: bigint | null;
  /** Number of yes votes */
  yesCount: bigint;
  /** Number of no votes */
  noCount: bigint;
  /** Number of abstain votes */
  abstainCount: bigint;
}

/**
 * Result of status computation including intermediate flags.
 * Used internally and returned in API response.
 */
export interface StatusComputationResult {
  status: ProposalStatus;
  isQuorumReached: boolean;
  isThresholdReached: boolean;
}

/**
 * API response for proposal status endpoint.
 * All bigint values are serialized appropriately for JSON.
 */
export interface ProposalStatusResponse {
  proposalId: string;
  status: ProposalStatus;
  votingMode: VotingMode;
  votes: {
    yes: number;
    no: number;
    abstain: number;
    total: number;
  };
  thresholds: {
    /** Serialized as string to preserve bigint precision */
    quorum: string;
    /** Serialized as string to preserve bigint precision */
    threshold: string;
  };
  timing: {
    startTime: number;
    endTime: number;
    timeRemaining: number | null;
    isVotingEnded: boolean;
  };
  isQuorumReached: boolean;
  isThresholdReached: boolean;
  canExecute: boolean;
}
