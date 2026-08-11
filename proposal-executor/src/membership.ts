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

/**
 * The on-chain Tally plus the executed flag and voting-window bounds from
 * getLatestProposalInformation. startDate/lastDate come from ProposalParameters
 * and are authoritative: the protocol rejects a vote whose block.timestamp falls
 * outside [startDate, lastDate], so stage-2 eligibility must honour them.
 */
export interface ProposalTally {
	executed: boolean
	yes: bigint
	no: bigint
	abstain: bigint
	/**
	 * Unix seconds the voting window opens (ProposalParameters.startDate).
	 * Zero until the first vote is cast — see {@link isVotingOpen}.
	 */
	startDate: bigint
	/** Unix seconds the voting window closes (ProposalParameters.lastDate); later votes revert. */
	lastDate: bigint
	/**
	 * Whether the DAO actually knows this proposal — i.e. `_creator` is not the zero
	 * space id. An unknown proposal id does not revert; it returns an all-zero struct
	 * that is otherwise identical to a real proposal awaiting its first vote, so this
	 * is the only field that separates the two. See {@link isEligibleToVote}.
	 */
	existsOnChain: boolean
}

/** SpaceRegistry.spaceIdToAddress returns this for an unregistered space. */
const ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

/**
 * Whether a bytes16 space id is the zero id, tolerating any hex casing and an
 * absent `0x` prefix. `getLatestProposalInformation` returns a zero `_creator`
 * for a proposal the DAO has never heard of.
 */
function isZeroSpaceId(spaceId: Hex): boolean {
	return /^(0x)?0*$/i.test(spaceId)
}

/**
 * Seconds of slack added past the on-chain voting window's close when deciding
 * whether to vote. The buffer is applied leniently (it extends the window rather
 * than shrinking it): missing a genuinely-open request during its final seconds
 * is worse than casting a vote that the contract rejects. So near the boundary we
 * prefer to vote and risk an at-most-CLOCK_SKEW_BUFFER_SECONDS-late revert over a
 * conservative skip. Mirrors the detection query's clock-skew constant.
 */
export const CLOCK_SKEW_BUFFER_SECONDS = 60n

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
 *
 * TODO(caching): this helper always performs a readContract() despite the "cache per run"
 * note above — caching is the caller's responsibility today, which is easy to get wrong once
 * a future orchestrator reuses this module. Either (a) add a memoizing wrapper here, e.g.
 * makeDaoSpaceResolver(wallet, registryAddr) returning a fn backed by an internal
 * Map<Hex, Address>, or (b) reword the docstring to state explicitly that callers must cache.
 */
export function resolveDaoSpaceAddress(
	wallet: SmartWallet,
	spaceRegistryAddress: Address,
	daoSpaceId: Hex,
): Effect.Effect<Address, InfraError> {
	// Effect.suspend captures `start` fresh on each retry attempt, so durationMs
	// reflects only the current attempt's wall time — mirrors executeProposal().
	return Effect.suspend(() => {
		const start = Date.now()
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
				new InfraError({
					message: `DAO space address resolution failed: ${sanitizeError(error)}`,
					durationMs: Date.now() - start,
				}),
		})
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
	// Effect.suspend captures `start` fresh on each retry attempt, so durationMs
	// reflects only the current attempt's wall time — mirrors executeProposal().
	return Effect.suspend(() => {
		const start = Date.now()
		return Effect.tryPromise({
			try: async () => {
				const proposalIdHex = uuidToBytes16(proposalId)
				const [executed, creator, parameters, tally] = await wallet.publicClient.readContract({
					address: daoSpaceAddress,
					abi: DAOSpaceAbi,
					functionName: "getLatestProposalInformation",
					args: [proposalIdHex],
				})
				return {
					executed,
					yes: tally.yes,
					no: tally.no,
					abstain: tally.abstain,
					startDate: parameters.startDate,
					lastDate: parameters.lastDate,
					existsOnChain: !isZeroSpaceId(creator),
				}
			},
			catch: (error) =>
				new InfraError({
					proposalId,
					message: `Proposal tally read failed: ${sanitizeError(error)}`,
					durationMs: Date.now() - start,
				}),
		})
	})
}

/**
 * Read the current chain time (latest block's timestamp) as Unix seconds.
 *
 * The voting-window check should compare against block.timestamp — the same clock
 * the protocol enforces — not the pod's wall clock, which can drift. If the block
 * read fails (transient RPC hiccup), fall back to the pod wall clock rather than
 * failing the batch; CLOCK_SKEW_BUFFER_SECONDS absorbs the drift. Read once per
 * run and reuse across requests in the batch.
 */
export function readChainTimeSeconds(wallet: SmartWallet): Effect.Effect<bigint> {
	return Effect.tryPromise({
		try: async () => (await wallet.publicClient.getBlock()).timestamp,
		catch: (error) => error,
	}).pipe(Effect.orElseSucceed(() => BigInt(Math.floor(Date.now() / 1000))))
}

/**
 * A proposal's voting window is open iff its timers have not been snapshotted yet,
 * or now is at/after startDate and at/before lastDate extended by the clock-skew
 * buffer. The bound is inclusive on both ends, matching the inclusive upper bound in
 * the stage-1 detection SQL ($2 <= end_time + skew) so the two stages agree at the
 * skew boundary. The close is widened, not narrowed:
 * during the final CLOCK_SKEW_BUFFER_SECONDS — and up to that long past lastDate —
 * the bot still votes, accepting a possible late revert over wrongly skipping a
 * still-open request. now should be a chain-sourced timestamp (readChainTimeSeconds).
 *
 * A zero `lastDate` means OPEN, not closed. Under governance v2 the voting window is
 * lazy: DAOSpace leaves startDate/lastDate/executeBy at zero until the first vote is
 * cast (`_startProposalVotingWindow`), and the contract itself uses `lastDate == 0`
 * as that "not started" sentinel. Treating zero as a closed window is what made the
 * whole auto-accept path dead on v2 — every untouched request, which is precisely the
 * set the bot exists to vote on, has a zero window by definition.
 */
export function isVotingOpen(
	nowSeconds: bigint,
	startDate: bigint,
	lastDate: bigint,
	skewSeconds: bigint = CLOCK_SKEW_BUFFER_SECONDS,
): boolean {
	if (lastDate === 0n) return true
	return startDate <= nowSeconds && nowSeconds <= lastDate + skewSeconds
}

/**
 * A request is eligible for an auto-accept YES vote iff:
 * - the DAO actually has the proposal (see below),
 * - it has not executed,
 * - no vote of any kind is recorded on-chain — a non-zero tally covers both a human's
 *   vote (indexing lag) and the bot's own in-flight vote → SKIP (onchain_tally_nonzero),
 *   which is what makes the job idempotent across cycles, and
 * - its on-chain voting window is still open (now within [startDate, lastDate], with the
 *   clock-skew buffer applied to the close, and a not-yet-snapshotted zero window
 *   counting as open). The protocol rejects votes outside the window, so this is the
 *   authoritative stage-2 guard against voting on a closed request.
 *
 * The `existsOnChain` term is load-bearing precisely *because* a zero window now counts
 * as open. A proposal that lives in the database but was never created on chain — the
 * migration artifacts described in the executor's canExecuteProposal pre-check — reads
 * back as all zeros: not executed, empty tally, zero window. Every one of those would
 * otherwise look permanently eligible, and the bot would burn a sponsored UserOperation
 * on a guaranteed revert for each, every five minutes, forever. They never age out
 * either: the membership query has no MAX_PROPOSAL_AGE cutoff. A zero `_creator` is the
 * only thing that distinguishes them from a genuine untouched request.
 *
 * now should be a chain-sourced timestamp (readChainTimeSeconds).
 */
export function isEligibleToVote(
	tally: ProposalTally,
	nowSeconds: bigint,
	skewSeconds: bigint = CLOCK_SKEW_BUFFER_SECONDS,
): boolean {
	return (
		tally.existsOnChain &&
		!tally.executed &&
		tally.yes === 0n &&
		tally.no === 0n &&
		tally.abstain === 0n &&
		isVotingOpen(nowSeconds, tally.startDate, tally.lastDate, skewSeconds)
	)
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
