/**
 * Proposal Auto-Executor — Effect-TS entry point.
 *
 * Two independent action paths share this process and ~5-minute cadence:
 *
 *  1. Execute path — detects slow-path governance proposals in EXECUTABLE status and
 *     calls enter(PROPOSAL_EXECUTED) on the Space Registry via the executor wallet.
 *  2. Membership-accept path — detects untouched request-to-join proposals in an
 *     allowlist of spaces and casts a single enter(PROPOSAL_VOTED, Yes) from a SECOND,
 *     distinct bot wallet. Because allowlisted spaces run with fastPathFlatThreshold = 1,
 *     that YES vote admits the joiner in the same transaction. A two-stage untouched
 *     check (indexer SQL → on-chain tally) makes it safe and idempotent.
 *
 * The two paths never share a wallet identity and are isolated: a failure in one does
 * not abort the other.
 *
 * Concurrency: parallel across spaces (unbounded), sequential within each space.
 * Error handling: RevertError → skip, InfraError → retry 2x then abort space.
 *
 * See: docs/plans/2026-03-02-feat-proposal-auto-executor-plan.md
 */

import {Config, Duration, Effect, Redacted, Schedule} from "effect"
import type {Hex} from "viem"
import {type Address, getAddress} from "viem"

import {InfraError, type RevertError, type SupportedChainId} from "./contracts.js"
import {
	connectDb,
	disconnectDb,
	findExecutableProposals,
	findMembershipRequests,
	MAX_PROPOSAL_AGE,
	type MembershipRequest,
	type Proposal,
} from "./detect.js"
import {createSmartWallet, executeProposal, type SmartWallet, uuidToBytes16, verifyExecutorSetup} from "./execute.js"
import {
	castMembershipVote,
	isEligibleToVote,
	type ProposalTally,
	readChainTimeSeconds,
	readProposalTally,
	resolveDaoSpaceAddress,
} from "./membership.js"
import {flush, TelemetryLive} from "./telemetry.js"

// Re-export tagged errors so tests and consumers have a single import
export {InfraError, RevertError} from "./contracts.js"

// ---------------------------------------------------------------------------
// Config parsing — uses Effect Config module (matches API + geo-cli patterns)
// ---------------------------------------------------------------------------

export interface ExecutorEnv {
	/** Redacted — unwrap with Redacted.value() only at the point of use */
	databaseUrl: Redacted.Redacted
	/** Redacted — unwrap with Redacted.value() only at the point of use */
	privateKey: Redacted.Redacted<`0x${string}`>
	/** Redacted — unwrap with Redacted.value() only at the point of use */
	pimlicoApiKey: Redacted.Redacted
	executorSpaceId: Hex
	spaceRegistryAddress: Address
	/** Redacted — may contain API keys in the path */
	rpcUrl: Redacted.Redacted
	chainId: SupportedChainId
	/**
	 * Redacted — embeds a ZeroDev project ID. Required only when chainId is
	 * 55516 (see ExecutorConfig.zerodevSponsorshipRpcUrl in execute.ts for why).
	 */
	zerodevSponsorshipRpcUrl: Redacted.Redacted

	// --- Membership-accept path (dedicated bot identity, distinct from the executor) ---
	/**
	 * Redacted — unwrap with Redacted.value() only at the point of use.
	 * Dedicated bot signing key — MUST differ from `privateKey`. Auto-prefixed 0x.
	 */
	membershipBotPrivateKey: Redacted.Redacted<`0x${string}`>
	/** Bot's registered personal space (bytes16) — MUST differ from `executorSpaceId`. */
	membershipBotSpaceId: Hex
	/**
	 * Spaces whose request-to-join proposals are auto-admitted. Empty ⇒ kill switch
	 * (no detection, no votes). Trimmed, validated, and de-duplicated at parse time.
	 */
	membershipAutoacceptSpaceIds: Hex[]
}

/** bytes16 hex: 0x-prefixed, 32 hex chars, 34 total */
const BYTES16_RE = /^0x[0-9a-fA-F]{32}$/

/** 0x-prefixed 64 hex chars (32 bytes) */
const PRIVATE_KEY_RE = /^0x[0-9a-fA-F]{64}$/

export const parseConfig: Effect.Effect<ExecutorEnv, InfraError> = Effect.gen(function* () {
	// Sensitive — wrapped in Redacted to prevent accidental logging/serialization
	const databaseUrl = yield* Config.redacted("DATABASE_URL")
	const rawPrivateKey = yield* Config.redacted("EXECUTOR_PRIVATE_KEY")
	const pimlicoApiKey = yield* Config.redacted("PIMLICO_API_KEY")
	const rawMembershipBotPrivateKey = yield* Config.redacted("MEMBERSHIP_BOT_PRIVATE_KEY")
	// Only required for chainId 55516 — validated below once chainId is known.
	const zerodevSponsorshipRpcUrl = yield* Config.redacted("ZERODEV_SPONSORSHIP_RPC_URL").pipe(
		Config.withDefault(Redacted.make("")),
	)

	const rpcUrl = yield* Config.redacted("RPC_URL")

	// Non-sensitive
	const rawExecutorSpaceId = yield* Config.string("EXECUTOR_SPACE_ID")
	const rawSpaceRegistryAddress = yield* Config.string("SPACE_REGISTRY_ADDRESS")
	const chainId = yield* Config.integer("CHAIN_ID")
	const rawMembershipBotSpaceId = yield* Config.string("MEMBERSHIP_BOT_SPACE_ID")
	// Empty/unset is valid — it is the kill switch (no detection, no votes).
	const rawMembershipAutoacceptSpaceIds = yield* Config.string("MEMBERSHIP_AUTOACCEPT_SPACE_IDS").pipe(
		Config.withDefault(""),
	)

	// --- Validate private key (on a plain temporary; re-wrapped before storing) ---
	let privateKey = Redacted.value(rawPrivateKey)
	if (!privateKey.startsWith("0x")) {
		privateKey = `0x${privateKey}`
	}
	if (!PRIVATE_KEY_RE.test(privateKey)) {
		return yield* Effect.fail(
			new InfraError({
				message: "Invalid EXECUTOR_PRIVATE_KEY: expected a 32-byte hex-encoded key with 0x prefix",
				durationMs: 0,
			}),
		)
	}

	// --- Validate executor space ID (bytes16) ---
	if (!BYTES16_RE.test(rawExecutorSpaceId)) {
		return yield* Effect.fail(
			new InfraError({
				message: `Invalid EXECUTOR_SPACE_ID: expected 0x-prefixed bytes16 (34 chars), got "${rawExecutorSpaceId}"`,
				durationMs: 0,
			}),
		)
	}

	// --- Validate space registry address ---
	let spaceRegistryAddress: Address
	try {
		spaceRegistryAddress = getAddress(rawSpaceRegistryAddress)
	} catch {
		return yield* Effect.fail(
			new InfraError({
				message: `Invalid SPACE_REGISTRY_ADDRESS: "${rawSpaceRegistryAddress}" is not a valid Ethereum address`,
				durationMs: 0,
			}),
		)
	}

	// --- Validate RPC URL ---
	const rpcUrlValue = Redacted.value(rpcUrl)
	if (!rpcUrlValue.startsWith("http://") && !rpcUrlValue.startsWith("https://")) {
		return yield* Effect.fail(
			new InfraError({
				message: "Invalid RPC_URL: expected http:// or https:// URL",
				durationMs: 0,
			}),
		)
	}

	// --- Validate chain ID ---
	if (chainId !== 80451 && chainId !== 19411 && chainId !== 55516) {
		return yield* Effect.fail(
			new InfraError({
				message: `Invalid CHAIN_ID: ${chainId}. Expected 80451 (mainnet), 19411 (testnet), or 55516 (testnet v2).`,
				durationMs: 0,
			}),
		)
	}

	// --- Validate ZeroDev sponsorship URL (only required on chain 55516, which has
	// no Safe infra and uses ZeroDev's EIP-7702 Kernel bundler instead of Pimlico) ---
	const zerodevSponsorshipRpcUrlValue = Redacted.value(zerodevSponsorshipRpcUrl)
	if (chainId === 55516) {
		if (
			!zerodevSponsorshipRpcUrlValue.startsWith("http://") &&
			!zerodevSponsorshipRpcUrlValue.startsWith("https://")
		) {
			return yield* Effect.fail(
				new InfraError({
					message:
						"Invalid ZERODEV_SPONSORSHIP_RPC_URL: expected http:// or https:// URL (required when CHAIN_ID=55516)",
					durationMs: 0,
				}),
			)
		}
	}

	// --- Validate membership-bot private key (auto-prefix 0x like the executor key;
	// validated on a plain temporary, then re-wrapped before storing) ---
	let membershipBotPrivateKey = Redacted.value(rawMembershipBotPrivateKey)
	if (!membershipBotPrivateKey.startsWith("0x")) {
		membershipBotPrivateKey = `0x${membershipBotPrivateKey}`
	}
	if (!PRIVATE_KEY_RE.test(membershipBotPrivateKey)) {
		return yield* Effect.fail(
			new InfraError({
				message: "Invalid MEMBERSHIP_BOT_PRIVATE_KEY: expected a 32-byte hex-encoded key with 0x prefix",
				durationMs: 0,
			}),
		)
	}

	// --- Validate membership-bot space ID (bytes16) ---
	if (!BYTES16_RE.test(rawMembershipBotSpaceId)) {
		return yield* Effect.fail(
			new InfraError({
				message: `Invalid MEMBERSHIP_BOT_SPACE_ID: expected 0x-prefixed bytes16 (34 chars), got "${rawMembershipBotSpaceId}"`,
				durationMs: 0,
			}),
		)
	}

	// --- Distinct-identity guard: the bot MUST NOT reuse the executor's identity ---
	if (membershipBotPrivateKey.toLowerCase() === privateKey.toLowerCase()) {
		return yield* Effect.fail(
			new InfraError({
				message:
					"MEMBERSHIP_BOT_PRIVATE_KEY must differ from EXECUTOR_PRIVATE_KEY (distinct bot identity required)",
				durationMs: 0,
			}),
		)
	}
	if (rawMembershipBotSpaceId.toLowerCase() === rawExecutorSpaceId.toLowerCase()) {
		return yield* Effect.fail(
			new InfraError({
				message: "MEMBERSHIP_BOT_SPACE_ID must differ from EXECUTOR_SPACE_ID (distinct bot identity required)",
				durationMs: 0,
			}),
		)
	}

	// --- Parse & validate the auto-accept allowlist (kill switch when empty) ---
	// Trim each entry, drop blanks, validate as bytes16, and de-duplicate
	// case-insensitively (first-seen casing wins).
	const seenSpaceIds = new Set<string>()
	const membershipAutoacceptSpaceIds: Hex[] = []
	for (const raw of rawMembershipAutoacceptSpaceIds.split(",")) {
		const entry = raw.trim()
		if (entry.length === 0) continue
		if (!BYTES16_RE.test(entry)) {
			return yield* Effect.fail(
				new InfraError({
					message: `Invalid MEMBERSHIP_AUTOACCEPT_SPACE_IDS entry: expected 0x-prefixed bytes16 (34 chars), got "${entry}"`,
					durationMs: 0,
				}),
			)
		}
		const key = entry.toLowerCase()
		if (seenSpaceIds.has(key)) continue
		seenSpaceIds.add(key)
		membershipAutoacceptSpaceIds.push(entry as Hex)
	}

	return {
		databaseUrl,
		// Re-wrap the validated keys so they stay redacted in the long-lived config and
		// are unwrapped only at the createSmartWallet call sites.
		privateKey: Redacted.make(privateKey as `0x${string}`),
		pimlicoApiKey,
		executorSpaceId: rawExecutorSpaceId as Hex,
		spaceRegistryAddress,
		rpcUrl,
		chainId: chainId as SupportedChainId,
		zerodevSponsorshipRpcUrl,
		membershipBotPrivateKey: Redacted.make(membershipBotPrivateKey as `0x${string}`),
		membershipBotSpaceId: rawMembershipBotSpaceId as Hex,
		membershipAutoacceptSpaceIds,
	}
}).pipe(
	Effect.catchTag("ConfigError", (e) =>
		Effect.fail(new InfraError({message: `Config error: ${e.message}`, durationMs: 0})),
	),
)

// ---------------------------------------------------------------------------
// Retry policy: exponential backoff, only for InfraError
// ---------------------------------------------------------------------------

const infraRetryPolicy = Schedule.compose(Schedule.exponential(Duration.seconds(1)), Schedule.recurs(2))

// ---------------------------------------------------------------------------
// Per-proposal execution with retry + timeout
// ---------------------------------------------------------------------------

function executeWithRetry(
	wallet: SmartWallet,
	proposal: Proposal,
	executorSpaceId: Hex,
	spaceRegistryAddress: Address,
): Effect.Effect<string, InfraError | RevertError> {
	return executeProposal(wallet, proposal, executorSpaceId, spaceRegistryAddress).pipe(
		Effect.timeoutFail({
			duration: Duration.seconds(30),
			onTimeout: () =>
				new InfraError({proposalId: proposal.id, message: "Execution timed out after 30s", durationMs: 30_000}),
		}),
		Effect.retry({schedule: infraRetryPolicy, while: (e) => e._tag === "InfraError"}),
		Effect.withSpan("proposal-executor.execute-proposal", {
			attributes: {proposalId: proposal.id, spaceId: proposal.spaceId},
		}),
	)
}

// ---------------------------------------------------------------------------
// Per-space: sequential execution, skip reverts, propagate infra errors
// ---------------------------------------------------------------------------

function executeSpaceProposals(
	spaceId: string,
	proposals: Proposal[],
	wallet: SmartWallet,
	executorSpaceId: Hex,
	spaceRegistryAddress: Address,
): Effect.Effect<(string | "skipped")[], InfraError> {
	return Effect.forEach(
		proposals,
		(proposal) =>
			executeWithRetry(wallet, proposal, executorSpaceId, spaceRegistryAddress).pipe(
				Effect.tap((txHash) =>
					Effect.logInfo("proposal_executed").pipe(
						Effect.annotateLogs({proposalId: proposal.id, spaceId, txHash}),
					),
				),
				Effect.catchTag("RevertError", (e) =>
					Effect.logInfo(e.expected ? "proposal_skip_expected" : "proposal_reverted").pipe(
						Effect.annotateLogs({
							proposalId: proposal.id,
							spaceId,
							error: e.message,
							expected: e.expected,
							durationMs: e.durationMs,
						}),
						Effect.as("skipped" as const),
					),
				),
				// InfraError propagates — fail-fast aborts remaining proposals in this space
			),
		{concurrency: 1},
	)
}

// ---------------------------------------------------------------------------
// Membership-accept path — orchestration helpers
// ---------------------------------------------------------------------------

/**
 * Convert a bytes16 hex space ID (0x + 32 hex chars) back to a dashed UUID.
 *
 * The allowlist is configured as bytes16 (MEMBERSHIP_AUTOACCEPT_SPACE_IDS), but the
 * indexer stores space_id as a dashed, lower-cased UUID. The detection query matches
 * on the UUID form, so the allowlist must be converted before it is passed in. This is
 * the inverse of execute.ts' uuidToBytes16.
 */
export function bytes16ToUuid(spaceId: Hex): string {
	const hex = spaceId.startsWith("0x") ? spaceId.slice(2) : spaceId
	if (hex.length !== 32 || !/^[0-9a-fA-F]+$/.test(hex)) {
		throw new Error(`Invalid bytes16 for UUID conversion: ${spaceId}`)
	}
	const h = hex.toLowerCase()
	return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
}

/**
 * Why a membership request was skipped instead of voted on. The bot only votes on a
 * genuinely untouched, still-open request; every other state maps to a distinct, typed
 * reason so the telemetry says *why* nothing was done:
 * - `indexed_vote` — stage 1 (indexer): a vote is already in proposal_votes. Enforced
 *   by the detection SQL's NOT EXISTS, so survivors reaching stage 2 have passed it;
 *   surfaced here for taxonomy completeness (not emitted by this loop at runtime).
 * - `already_executed` — stage 2: the proposal has already executed (resolved).
 * - `onchain_tally_nonzero` — stage 2: a vote is already recorded in the on-chain
 *   tally. This is also what the bot's own prior vote trips, making the job idempotent.
 * - `voting_window_closed` — stage 2: the voting period has ended, so the protocol
 *   would reject a late vote. An untouched-but-expired request is left alone — this is
 *   independent of the tally, which may well be zero.
 */
export type MembershipSkipReason =
	| "indexed_vote"
	| "already_executed"
	| "onchain_tally_nonzero"
	| "voting_window_closed"

/**
 * The outcome of processing one membership request:
 * - `voted`    — a YES vote was cast (the requester is admitted),
 * - `skipped`  — ineligible at stage 2 (touched/resolved/closed) → not our business,
 * - `reverted` — the vote reverted on-chain (e.g. already executed, or the bot lacks
 *   the EDITOR role); logged and left for the next cycle, never aborts other requests.
 */
type MembershipOutcome =
	| {status: "voted"; txHash: string}
	| {status: "skipped"; reason: MembershipSkipReason}
	| {status: "reverted"; expected: boolean}

/** Result of processing all requests in a single space. */
type MembershipSpaceResult =
	| {status: "ok"; spaceId: string; outcomes: MembershipOutcome[]}
	| {status: "infraError"; spaceId: string; outcomes: MembershipOutcome[]}

interface MembershipSummary {
	/** YES votes cast (requesters admitted this cycle). */
	admitted: number
	/** Requests left alone — ineligible (stage 2) or reverted. */
	skipped: number
	/** Spaces aborted by a persistent infrastructure error. */
	failed: number
	/** Candidate requests surfaced by stage-1 detection. */
	total: number
	/** Allowlisted spaces with at least one candidate. */
	spaces: number
}

/**
 * Classify a stage-1 survivor after the authoritative on-chain (stage-2) read.
 * Returns `null` when the request is still eligible (the bot should vote), otherwise
 * the specific reason it is ineligible. Stays in lock-step with isEligibleToVote — it
 * returns null in exactly the cases that function returns true, then names the cause:
 * executed first, then a non-zero tally, and finally a closed voting window (the
 * remaining cause for an untouched, unexecuted request).
 */
export function classifyMembershipSkip(tally: ProposalTally, nowSeconds: bigint): MembershipSkipReason | null {
	if (isEligibleToVote(tally, nowSeconds)) return null
	if (tally.executed) return "already_executed"
	if (tally.yes !== 0n || tally.no !== 0n || tally.abstain !== 0n) return "onchain_tally_nonzero"
	return "voting_window_closed"
}

/**
 * Aggregate per-space results into the run summary. Votes count as `admitted`;
 * both ineligible skips and on-chain reverts count as `skipped` (non-admitting,
 * non-failing); spaces aborted by infra count as `failed`.
 */
export function aggregateMembership(results: MembershipSpaceResult[]): {
	admitted: number
	skipped: number
	failed: number
} {
	let admitted = 0
	let skipped = 0
	for (const r of results) {
		for (const o of r.outcomes) {
			if (o.status === "voted") admitted++
			else skipped++
		}
	}
	const failed = results.filter((r) => r.status === "infraError").length
	return {admitted, skipped, failed}
}

/** Cast a membership YES vote with the same timeout + InfraError retry the executor uses. */
function castVoteWithRetry(
	wallet: SmartWallet,
	request: MembershipRequest,
	botSpaceId: Hex,
	spaceRegistryAddress: Address,
): Effect.Effect<string, InfraError | RevertError> {
	return castMembershipVote(wallet, request, botSpaceId, spaceRegistryAddress).pipe(
		Effect.timeoutFail({
			duration: Duration.seconds(30),
			onTimeout: () =>
				new InfraError({proposalId: request.id, message: "Vote cast timed out after 30s", durationMs: 30_000}),
		}),
		Effect.retry({schedule: infraRetryPolicy, while: (e) => e._tag === "InfraError"}),
		Effect.withSpan("proposal-executor.membership-vote", {
			attributes: {proposalId: request.id, spaceId: request.spaceId},
		}),
	)
}

/** Read the on-chain tally (stage 2) with the same timeout + InfraError retry. */
function readTallyWithRetry(
	wallet: SmartWallet,
	daoSpaceAddress: Address,
	proposalId: string,
): Effect.Effect<ProposalTally, InfraError> {
	return readProposalTally(wallet, daoSpaceAddress, proposalId).pipe(
		Effect.timeoutFail({
			duration: Duration.seconds(30),
			onTimeout: () =>
				new InfraError({proposalId, message: "Tally read timed out after 30s", durationMs: 30_000}),
		}),
		Effect.retry({schedule: infraRetryPolicy, while: (e) => e._tag === "InfraError"}),
	)
}

/**
 * Resolve a space's DAOSpace address (stage-2 prerequisite) with the same per-attempt
 * timeout + InfraError retry as the other membership RPC reads. The timeout guards
 * against a hung lookup that never rejects — without it a single stuck call would block
 * the fiber until the global 270s run timeout, stalling every other space.
 */
function resolveDaoSpaceAddressWithRetry(
	wallet: SmartWallet,
	spaceRegistryAddress: Address,
	daoSpaceId: Hex,
): Effect.Effect<Address, InfraError> {
	return resolveDaoSpaceAddress(wallet, spaceRegistryAddress, daoSpaceId).pipe(
		Effect.timeoutFail({
			duration: Duration.seconds(30),
			onTimeout: () =>
				new InfraError({message: "DAO space address resolution timed out after 30s", durationMs: 30_000}),
		}),
		Effect.retry({schedule: infraRetryPolicy, while: (e) => e._tag === "InfraError"}),
	)
}

/**
 * Process one membership request: stage-2 on-chain read → eligibility gate → vote.
 * A RevertError is caught and turned into a `reverted` outcome (logged, not fatal) so
 * it never aborts the rest of the space. An InfraError propagates to abort this space
 * (retried next cycle), matching the executor's per-space fail-fast.
 */
export function processMembershipRequest(
	wallet: SmartWallet,
	request: MembershipRequest,
	daoSpaceAddress: Address,
	botSpaceId: Hex,
	spaceRegistryAddress: Address,
	nowSeconds: bigint,
): Effect.Effect<MembershipOutcome, InfraError> {
	return readTallyWithRetry(wallet, daoSpaceAddress, request.id).pipe(
		Effect.flatMap((tally) => {
			const skipReason = classifyMembershipSkip(tally, nowSeconds)
			if (skipReason !== null) {
				return Effect.logInfo("membership_skip").pipe(
					Effect.annotateLogs({
						proposalId: request.id,
						spaceId: request.spaceId,
						targetId: request.requesterId,
						reason: skipReason,
						yes: tally.yes.toString(),
						no: tally.no.toString(),
						abstain: tally.abstain.toString(),
						executed: tally.executed,
					}),
					Effect.as({status: "skipped", reason: skipReason} as MembershipOutcome),
				)
			}

			return castVoteWithRetry(wallet, request, botSpaceId, spaceRegistryAddress).pipe(
				Effect.tap((txHash) =>
					Effect.logInfo("membership_vote_cast").pipe(
						Effect.annotateLogs({
							proposalId: request.id,
							spaceId: request.spaceId,
							targetId: request.requesterId,
							txHash,
						}),
					),
				),
				Effect.map((txHash) => ({status: "voted", txHash}) as MembershipOutcome),
				Effect.catchTag("RevertError", (e) =>
					Effect.logInfo(e.expected ? "membership_skip_expected" : "membership_vote_reverted").pipe(
						Effect.annotateLogs({
							proposalId: request.id,
							spaceId: request.spaceId,
							error: e.message,
							expected: e.expected,
							durationMs: e.durationMs,
						}),
						Effect.as({status: "reverted", expected: e.expected} as MembershipOutcome),
					),
				),
			)
		}),
	)
}

/**
 * Process every request in one space: resolve its DAOSpace address once (cached for
 * the space), then run requests sequentially. An InfraError propagates to the caller,
 * which catches it at the space boundary so one space failing doesn't cancel others.
 */
export function processSpaceMembership(
	wallet: SmartWallet,
	spaceId: string,
	requests: MembershipRequest[],
	botSpaceId: Hex,
	spaceRegistryAddress: Address,
	nowSeconds: bigint,
): Effect.Effect<MembershipOutcome[], InfraError> {
	return resolveDaoSpaceAddressWithRetry(wallet, spaceRegistryAddress, uuidToBytes16(spaceId)).pipe(
		Effect.flatMap((daoSpaceAddress) =>
			Effect.forEach(
				requests,
				(request) =>
					processMembershipRequest(
						wallet,
						request,
						daoSpaceAddress,
						botSpaceId,
						spaceRegistryAddress,
						nowSeconds,
					),
				{concurrency: 1},
			),
		),
	)
}

// ---------------------------------------------------------------------------
// Main — orchestration, error handling, flush, exit code
// ---------------------------------------------------------------------------

const runId = crypto.randomUUID()

const main = Effect.gen(function* () {
	const config = yield* parseConfig
	const db = yield* connectDb(Redacted.value(config.databaseUrl))

	// Ensure DB is disconnected when we exit (success or failure)
	yield* Effect.addFinalizer(() => disconnectDb(db).pipe(Effect.tap(() => Effect.logDebug("db_disconnected"))))

	const wallet = yield* createSmartWallet({
		privateKey: Redacted.value(config.privateKey),
		pimlicoApiKey: Redacted.value(config.pimlicoApiKey),
		executorSpaceId: config.executorSpaceId,
		spaceRegistryAddress: config.spaceRegistryAddress,
		rpcUrl: Redacted.value(config.rpcUrl),
		chainId: config.chainId,
		zerodevSponsorshipRpcUrl: Redacted.value(config.zerodevSponsorshipRpcUrl) || undefined,
	})

	yield* Effect.logInfo("wallet_ready").pipe(
		Effect.annotateLogs({identity: "executor", accountAddress: wallet.accountAddress}),
	)

	yield* verifyExecutorSetup(wallet, config.executorSpaceId, config.spaceRegistryAddress)

	const runStart = Date.now()
	const nowSeconds = Math.floor(runStart / 1000)

	// --- Execute path (executor wallet) — total: catches its own failures so it never
	// aborts the membership path. The only uncaught source below the space boundary is
	// the detection query; its InfraError is converted to a failed summary here. ---
	const runExecutePath: Effect.Effect<{
		succeeded: number
		failed: number
		skipped: number
		total: number
		spaces: number
	}> = Effect.gen(function* () {
		const proposals = yield* findExecutableProposals(db, nowSeconds).pipe(
			Effect.withSpan("proposal-executor.detect"),
		)
		const bySpace = Map.groupBy(proposals, (p) => p.spaceId)

		yield* Effect.logInfo("run_start").pipe(
			Effect.annotateLogs({
				proposalsFound: proposals.length,
				spaces: bySpace.size,
				ageCutoffTimestamp: nowSeconds - MAX_PROPOSAL_AGE,
			}),
		)

		if (proposals.length === 0) {
			return {succeeded: 0, failed: 0, skipped: 0, total: 0, spaces: 0}
		}

		// Parallel across spaces (capped at 10 concurrent RPC connections), sequential
		// within each space. Each space is independent — an InfraError in one space
		// doesn't affect others.
		const results = yield* Effect.forEach(
			[...bySpace.entries()],
			([spaceId, spaceProposals]) =>
				executeSpaceProposals(
					spaceId,
					spaceProposals,
					wallet,
					config.executorSpaceId,
					config.spaceRegistryAddress,
				).pipe(
					Effect.map(
						(outcomes) =>
							({
								status: "ok" as const,
								spaceId,
								succeeded: outcomes.filter((r) => r !== "skipped").length,
								skipped: outcomes.filter((r) => r === "skipped").length,
							}) as const,
					),
					// Catch InfraError at the space level so one space failing
					// doesn't cancel the others
					Effect.catchTag("InfraError", (e) =>
						Effect.logError("space_aborted").pipe(
							Effect.annotateLogs({spaceId, error: e.message, proposalId: e.proposalId}),
							Effect.as({status: "infraError" as const, spaceId, succeeded: 0, skipped: 0} as const),
						),
					),
				),
			{concurrency: 10},
		)

		return {
			succeeded: results.reduce((n, r) => n + r.succeeded, 0),
			failed: results.filter((r) => r.status === "infraError").length,
			skipped: results.reduce((n, r) => n + r.skipped, 0),
			total: proposals.length,
			spaces: bySpace.size,
		}
	}).pipe(
		Effect.catchTag("InfraError", (e) =>
			Effect.logError("execute_path_failed").pipe(
				Effect.annotateLogs({error: e.message, proposalId: e.proposalId}),
				Effect.as({succeeded: 0, failed: 1, skipped: 0, total: 0, spaces: 0}),
			),
		),
	)

	// --- Membership-accept path (bot wallet) — total: a detection-query failure is
	// caught here so it never aborts the execute path. Per-space infra failures are
	// caught at the space boundary; reverts are absorbed per request. Kill switch: an
	// empty allowlist short-circuits findMembershipRequests to [] (no query). ---
	const runMembershipPath: Effect.Effect<MembershipSummary> = Effect.gen(function* () {
		const allowlistUuids = config.membershipAutoacceptSpaceIds.map(bytes16ToUuid)

		// Kill switch: an empty allowlist does no work at all — no chain read, no query,
		// and no bot wallet setup. This fully disables the membership-bot dependency
		// (key, space, Pimlico/RPC), so misconfiguration there can never affect a run.
		if (allowlistUuids.length === 0) {
			yield* Effect.logInfo("membership_start").pipe(
				Effect.annotateLogs({allowlistSize: 0, requestsFound: 0, spaces: 0}),
			)
			return {admitted: 0, skipped: 0, failed: 0, total: 0, spaces: 0}
		}

		// Membership-accept bot wallet — a SECOND, distinct identity built from the
		// dedicated bot key + space. Built and verified inside this path (not main) so a
		// misconfigured bot or transient infra failure (Pimlico/bundler/RPC) is caught by
		// the membership path's own InfraError boundary and never aborts the execute path.
		// Verified each run so a misconfigured bot fails fast. It casts the membership YES
		// votes; the executor wallet never does.
		const botWallet = yield* createSmartWallet({
			privateKey: Redacted.value(config.membershipBotPrivateKey),
			pimlicoApiKey: Redacted.value(config.pimlicoApiKey),
			executorSpaceId: config.membershipBotSpaceId,
			spaceRegistryAddress: config.spaceRegistryAddress,
			rpcUrl: Redacted.value(config.rpcUrl),
			chainId: config.chainId,
			zerodevSponsorshipRpcUrl: Redacted.value(config.zerodevSponsorshipRpcUrl) || undefined,
		})

		yield* Effect.logInfo("wallet_ready").pipe(
			Effect.annotateLogs({identity: "membership-bot", accountAddress: botWallet.accountAddress}),
		)

		yield* verifyExecutorSetup(botWallet, config.membershipBotSpaceId, config.spaceRegistryAddress)

		// Read chain time once, BEFORE detection, and reuse it for both stages. Stage-1
		// detection and stage-2 eligibility must share one notion of "now" and the same
		// +CLOCK_SKEW_BUFFER window, or a request can land where stage 2 would vote but
		// stage 1 never surfaces it (the pod wall clock can drift from block.timestamp).
		const chainNow = yield* readChainTimeSeconds(botWallet)

		const requests = yield* findMembershipRequests(db, allowlistUuids, Number(chainNow))
		const bySpace = Map.groupBy(requests, (r) => r.spaceId)

		yield* Effect.logInfo("membership_start").pipe(
			Effect.annotateLogs({
				allowlistSize: allowlistUuids.length,
				requestsFound: requests.length,
				spaces: bySpace.size,
			}),
		)

		if (requests.length === 0) {
			return {admitted: 0, skipped: 0, failed: 0, total: 0, spaces: 0}
		}

		const results: MembershipSpaceResult[] = yield* Effect.forEach(
			[...bySpace.entries()],
			([spaceId, spaceRequests]) =>
				processSpaceMembership(
					botWallet,
					spaceId,
					spaceRequests,
					config.membershipBotSpaceId,
					config.spaceRegistryAddress,
					chainNow,
				).pipe(
					Effect.map((outcomes) => ({status: "ok" as const, spaceId, outcomes})),
					// Catch InfraError at the space level so one space failing
					// doesn't cancel the others.
					Effect.catchTag("InfraError", (e) =>
						Effect.logError("membership_space_aborted").pipe(
							Effect.annotateLogs({spaceId, error: e.message, proposalId: e.proposalId}),
							Effect.as({status: "infraError" as const, spaceId, outcomes: []}),
						),
					),
				),
			{concurrency: 10},
		)

		const {admitted, skipped, failed} = aggregateMembership(results)
		return {admitted, skipped, failed, total: requests.length, spaces: bySpace.size}
	}).pipe(
		Effect.catchTag("InfraError", (e) =>
			Effect.logError("membership_path_failed").pipe(
				Effect.annotateLogs({error: e.message, proposalId: e.proposalId}),
				Effect.as({admitted: 0, skipped: 0, failed: 1, total: 0, spaces: 0}),
			),
		),
	)

	// Run both paths concurrently — distinct wallets, isolated error boundaries. Each
	// is total, so Effect.all cannot interrupt one when the other settles.
	const [exec, membership] = yield* Effect.all([runExecutePath, runMembershipPath], {concurrency: 2})

	yield* Effect.logInfo("run_end").pipe(
		Effect.annotateLogs({
			succeeded: exec.succeeded,
			failed: exec.failed,
			skipped: exec.skipped,
			total: exec.total,
			spaces: exec.spaces,
			membershipAdmitted: membership.admitted,
			membershipSkipped: membership.skipped,
			membershipFailed: membership.failed,
			membershipTotal: membership.total,
			membershipSpaces: membership.spaces,
			durationMs: Date.now() - runStart,
		}),
	)

	// Fold membership outcomes into the same succeeded/failed semantics the exit code
	// uses: an admitted request counts as a success, an aborted space as a failure.
	// Partial success (anything admitted/executed) → exit 0.
	return {
		succeeded: exec.succeeded + membership.admitted,
		failed: exec.failed + membership.failed,
	}
}).pipe(
	Effect.scoped,
	// 270s top-level timeout — leaves 20s margin before K8s SIGKILL at 290s,
	// ensuring finalizers (DB disconnect) run and a structured log is emitted.
	Effect.timeoutFail({
		duration: Duration.seconds(270),
		onTimeout: () => new InfraError({message: "Run timed out after 270s", durationMs: 270_000}),
	}),
	Effect.catchTag("InfraError", (e) =>
		Effect.logError("run_failed").pipe(
			Effect.annotateLogs({error: e.message, proposalId: e.proposalId}),
			Effect.as({succeeded: 0, failed: 1}),
		),
	),
	Effect.annotateLogs({runId}),
	Effect.withSpan("proposal-executor.run"),
	Effect.catchAllDefect((defect) =>
		Effect.logFatal("fatal").pipe(
			Effect.annotateLogs({error: String(defect)}),
			Effect.as({succeeded: 0, failed: 1}),
		),
	),
	// Flush runs on every exit path — success, error, timeout, defect
	Effect.tap(() => flush),
	// Outermost: provides SentryLogger to catchTag, catchAllDefect, and flush
	Effect.provide(TelemetryLive),
)

// Total failure (all failed, none succeeded) → exit 1 so K8s marks the job as failed.
// Partial success → exit 0; the next CronJob run will retry the remaining proposals.
// Guarded by import.meta.main so importing this module (e.g. from tests to exercise
// parseConfig) does not kick off a run or call process.exit().
if (import.meta.main) {
	Effect.runPromise(main)
		.then(({succeeded, failed}) => process.exit(failed > 0 && succeeded === 0 ? 1 : 0))
		.catch((err) => {
			console.error(JSON.stringify({level: "fatal", message: "unhandled_defect", error: String(err)}))
			process.exit(1)
		})
}
