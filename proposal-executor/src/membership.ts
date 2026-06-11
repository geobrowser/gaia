/**
 * Membership-accept path: stage-2 on-chain eligibility read + PROPOSAL_VOTED (Yes)
 * vote cast, both performed by the dedicated bot wallet.
 *
 * Stage 1 (the indexer side) lives in detect.ts (findMembershipRequests). This module
 * is the authoritative stage-2 half of the "untouched" check: it reads the live on-chain
 * tally from the per-space DAOSpace contract and casts a single YES vote on a still-
 * untouched request. Because allowlisted spaces run with fastPathFlatThreshold = 1, that
 * one YES vote meets the threshold and the contract executes the AddMember action in the
 * same transaction — admitting the joiner with no human in the loop.
 *
 * Reuses the executor's smart-wallet, enter() calldata pattern, encoding helpers, and
 * error classification (classifyAsRevert) wholesale — see execute.ts.
 */

import {Effect} from "effect"
import {type Address, encodeAbiParameters, encodeFunctionData, type Hex} from "viem"
import {
	DAOSpaceAbi,
	EMPTY_SIGNATURE,
	InfraError,
	PROPOSAL_VOTED_ACTION,
	RevertError,
	SpaceRegistryAbi,
	VOTE_YES,
} from "./contracts.js"
import type {MembershipRequest} from "./detect.js"
import {classifyAsRevert, padBytes16ToBytes32, type SmartWallet, uuidToBytes16} from "./execute.js"

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** The on-chain Tally plus the executed flag from getLatestProposalInformation. */
export interface ProposalTally {
	executed: boolean
	yes: bigint
	no: bigint
	abstain: bigint
}

/** SpaceRegistry.spaceIdToAddress returns this for an unregistered space. */
const ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

// ---------------------------------------------------------------------------
// Sanitization — never leak a bundler URL's API key into an error message.
// Defense in depth: mirrors the pattern in execute.ts / detect.ts.
// ---------------------------------------------------------------------------

function sanitizeError(error: unknown): string {
	return String(error).replace(/apikey=[^\s&"]+/gi, "apikey=<redacted>")
}

// ---------------------------------------------------------------------------
// Vote-data encoding
// ---------------------------------------------------------------------------

/**
 * ABI-encode the PROPOSAL_VOTED data payload: abi.encode(bytes16 proposalId, uint8 voteOption).
 * A YES vote passes VOTE_YES (1) — see the IDAOSpace.VoteOption enum (None=0, Yes=1, No=2, Abstain=3).
 */
export function encodeVoteData(proposalIdHex: Hex, voteOption: number): Hex {
	return encodeAbiParameters(
		[
			{name: "proposalId", type: "bytes16"},
			{name: "voteOption", type: "uint8"},
		],
		[proposalIdHex, voteOption],
	)
}

// ---------------------------------------------------------------------------
// DAO space address resolution
// ---------------------------------------------------------------------------

/**
 * Resolve a DAO space's on-chain contract address via SpaceRegistry.spaceIdToAddress.
 * The vote-tally getters live on the per-space DAOSpace contract, not the registry, so
 * this must be resolved before reading the tally. Resolve once per space (cache per run).
 *
 * A zero address means the space is unregistered (misconfiguration) — surfaced as an
 * InfraError so the space is skipped and retried next cycle.
 */
export function resolveDaoSpaceAddress(
	wallet: SmartWallet,
	spaceRegistryAddress: Address,
	daoSpaceId: Hex,
): Effect.Effect<Address, InfraError> {
	return Effect.tryPromise({
		try: async () => {
			const address = await wallet.publicClient.readContract({
				address: spaceRegistryAddress,
				abi: SpaceRegistryAbi,
				functionName: "spaceIdToAddress",
				args: [daoSpaceId],
			})
			if (address.toLowerCase() === ZERO_ADDRESS) {
				throw new Error(
					`DAO space ${daoSpaceId} is not registered (spaceIdToAddress returned the zero address)`,
				)
			}
			return address
		},
		catch: (error) =>
			new InfraError({message: `DAO space address resolution failed: ${sanitizeError(error)}`, durationMs: 0}),
	})
}

// ---------------------------------------------------------------------------
// Stage-2 authoritative untouched read
// ---------------------------------------------------------------------------

/**
 * Read the live on-chain tally for a proposal from its DAOSpace contract.
 *
 * This is the authoritative stage-2 untouched check. It closes the indexing-lag window
 * (a vote may already be on-chain before the indexer surfaces it) and, because the bot's
 * own prior vote shows up here immediately, it makes the job idempotent across cycles.
 */
export function readProposalTally(
	wallet: SmartWallet,
	daoSpaceAddress: Address,
	proposalId: string,
): Effect.Effect<ProposalTally, InfraError> {
	return Effect.tryPromise({
		try: async () => {
			const proposalIdHex = uuidToBytes16(proposalId)
			const [executed, , , tally] = await wallet.publicClient.readContract({
				address: daoSpaceAddress,
				abi: DAOSpaceAbi,
				functionName: "getLatestProposalInformation",
				args: [proposalIdHex],
			})
			return {executed, yes: tally.yes, no: tally.no, abstain: tally.abstain}
		},
		catch: (error) =>
			new InfraError({
				proposalId,
				message: `Proposal tally read failed: ${sanitizeError(error)}`,
				durationMs: 0,
			}),
	})
}

/**
 * A request is eligible for an auto-accept YES vote iff it has not executed and no vote of
 * any kind is recorded on-chain. A non-zero tally covers both a human's vote (indexing lag)
 * and the bot's own in-flight vote → SKIP (reason: onchain_tally_nonzero), idempotency.
 */
export function isEligibleToVote(tally: ProposalTally): boolean {
	return !tally.executed && tally.yes === 0n && tally.no === 0n && tally.abstain === 0n
}

// ---------------------------------------------------------------------------
// Vote cast
// ---------------------------------------------------------------------------

/**
 * Classify a vote-cast failure, reusing the executor's revert detection.
 *
 * Expected reverts (the request is already resolved by someone else): "already executed",
 * CanNotExecute, InvalidAction → RevertError(expected=true), logged INFO. A CanNotVote
 * revert means the bot lacks the EDITOR role — a real misconfiguration → expected=false,
 * skipped and retried next cycle. Everything else is infrastructure → InfraError.
 */
function classifyVoteError(error: unknown, proposalId: string, durationMs: number): RevertError | InfraError {
	const message = sanitizeError(error)
	const errorName = error instanceof Error ? error.name : typeof error

	if (classifyAsRevert(error, errorName, message)) {
		const isExpected =
			message.includes("already executed") ||
			message.includes("ProposalAlreadyExecuted") ||
			message.includes("CanNotExecute") ||
			message.includes("InvalidAction")
		return new RevertError({proposalId, message: `[${errorName}] ${message}`, expected: isExpected, durationMs})
	}

	return new InfraError({proposalId, message: `[${errorName}] ${message}`, durationMs})
}

/**
 * Cast a single YES vote on a membership request via the bot wallet.
 *
 * Builds the same gas-sponsored enter() UserOperation the executor uses, with the
 * PROPOSAL_VOTED action and the (proposalId, Yes) vote data:
 *
 *   enter(botSpaceId, daoSpaceId, PROPOSAL_VOTED, bytes32(proposalId),
 *         abi.encode(bytes16 proposalId, uint8 1), "0x")
 *
 * Returns the transaction hash on success. Errors are classified as RevertError /
 * InfraError; retry and timeout are composed externally in index.ts.
 */
export function castMembershipVote(
	wallet: SmartWallet,
	request: MembershipRequest,
	botSpaceId: Hex,
	spaceRegistryAddress: Address,
): Effect.Effect<string, RevertError | InfraError> {
	// Effect.suspend captures `start` fresh on each retry attempt so durationMs reflects
	// only the current attempt's wall time (matches executeProposal).
	return Effect.suspend(() => {
		const start = Date.now()

		return Effect.tryPromise({
			try: async () => {
				const daoSpaceId = uuidToBytes16(request.spaceId)
				const proposalIdHex = uuidToBytes16(request.id)

				const calldata = encodeFunctionData({
					abi: SpaceRegistryAbi,
					functionName: "enter",
					args: [
						botSpaceId, // bytes16: membership bot's personal space ID
						daoSpaceId, // bytes16: DAO space being joined
						PROPOSAL_VOTED_ACTION, // bytes32: action hash
						padBytes16ToBytes32(proposalIdHex), // bytes32: topic = bytes32(proposalId), left-aligned
						encodeVoteData(proposalIdHex, VOTE_YES), // bytes: abi.encode(bytes16 proposalId, uint8 1)
						EMPTY_SIGNATURE, // bytes: "0x" (ignored when msg.sender == _fromSpace)
					],
				})

				const account = wallet.smartAccountClient.account
				if (!account) {
					throw new Error("Smart account client has no account — bot wallet was not initialized correctly")
				}

				const hash = await wallet.smartAccountClient.sendTransaction({
					account,
					chain: wallet.chain,
					to: spaceRegistryAddress,
					data: calldata,
				})

				return hash
			},
			catch: (error) => classifyVoteError(error, request.id, Date.now() - start),
		})
	})
}
