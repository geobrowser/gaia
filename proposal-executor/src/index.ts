/**
 * Proposal Auto-Executor — Effect-TS entry point.
 *
 * Detects slow-path governance proposals in EXECUTABLE status and calls
 * enter(PROPOSAL_EXECUTED) on the Space Registry contract via a gas-sponsored
 * Safe smart account.
 *
 * Concurrency: parallel across spaces (unbounded), sequential within each space.
 * Error handling: RevertError → skip, InfraError → retry 2x then abort space.
 *
 * See: docs/plans/2026-03-02-feat-proposal-auto-executor-plan.md
 */

import {Config, Duration, Effect, Logger, Redacted, Schedule} from "effect"
import type {Hex} from "viem"
import {type Address, getAddress} from "viem"

import {InfraError, type SupportedChainId} from "./contracts.js"
import {connectDb, disconnectDb, findExecutableProposals, type Proposal} from "./detect.js"
import {createSmartWallet, executeProposal, type SmartWallet, verifyExecutorSetup} from "./execute.js"

// Re-export tagged errors so tests and consumers have a single import
export {InfraError, RevertError} from "./contracts.js"

// ---------------------------------------------------------------------------
// Config parsing — uses Effect Config module (matches API + geo-cli patterns)
// ---------------------------------------------------------------------------

interface ExecutorEnv {
	/** Redacted — unwrap with Redacted.value() only at the point of use */
	databaseUrl: Redacted.Redacted
	privateKey: `0x${string}`
	/** Redacted — unwrap with Redacted.value() only at the point of use */
	pimlicoApiKey: Redacted.Redacted
	executorSpaceId: Hex
	spaceRegistryAddress: Address
	rpcUrl: string
	chainId: SupportedChainId
}

/** bytes16 hex: 0x-prefixed, 32 hex chars, 34 total */
const BYTES16_RE = /^0x[0-9a-fA-F]{32}$/

/** 0x-prefixed 64 hex chars (32 bytes) */
const PRIVATE_KEY_RE = /^0x[0-9a-fA-F]{64}$/

const parseConfig: Effect.Effect<ExecutorEnv, InfraError> = Effect.gen(function* () {
	// Sensitive — wrapped in Redacted to prevent accidental logging/serialization
	const databaseUrl = yield* Config.redacted("DATABASE_URL")
	const rawPrivateKey = yield* Config.redacted("EXECUTOR_PRIVATE_KEY")
	const pimlicoApiKey = yield* Config.redacted("PIMLICO_API_KEY")

	// Non-sensitive
	const rawExecutorSpaceId = yield* Config.string("EXECUTOR_SPACE_ID")
	const rawSpaceRegistryAddress = yield* Config.string("SPACE_REGISTRY_ADDRESS")
	const rpcUrl = yield* Config.string("RPC_URL")
	const chainId = yield* Config.integer("CHAIN_ID")

	// --- Validate private key ---
	let privateKey = Redacted.value(rawPrivateKey)
	if (!privateKey.startsWith("0x")) {
		privateKey = `0x${privateKey}`
	}
	if (!PRIVATE_KEY_RE.test(privateKey)) {
		return yield* Effect.fail(
			new InfraError({
				proposalId: "N/A",
				message: "Invalid EXECUTOR_PRIVATE_KEY: expected a 32-byte hex-encoded key with 0x prefix",
				durationMs: 0,
			}),
		)
	}

	// --- Validate executor space ID (bytes16) ---
	if (!BYTES16_RE.test(rawExecutorSpaceId)) {
		return yield* Effect.fail(
			new InfraError({
				proposalId: "N/A",
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
				proposalId: "N/A",
				message: `Invalid SPACE_REGISTRY_ADDRESS: "${rawSpaceRegistryAddress}" is not a valid Ethereum address`,
				durationMs: 0,
			}),
		)
	}

	// --- Validate chain ID ---
	if (chainId !== 80451 && chainId !== 19411) {
		return yield* Effect.fail(
			new InfraError({
				proposalId: "N/A",
				message: `Invalid CHAIN_ID: ${chainId}. Expected 80451 (mainnet) or 19411 (testnet).`,
				durationMs: 0,
			}),
		)
	}

	return {
		databaseUrl,
		privateKey: privateKey as `0x${string}`,
		pimlicoApiKey,
		executorSpaceId: rawExecutorSpaceId as Hex,
		spaceRegistryAddress,
		rpcUrl,
		chainId: chainId as SupportedChainId,
	}
}).pipe(
	Effect.catchTag("ConfigError", (e) =>
		Effect.fail(new InfraError({proposalId: "N/A", message: `Config error: ${e.message}`, durationMs: 0})),
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
) {
	return executeProposal(wallet, proposal, executorSpaceId, spaceRegistryAddress).pipe(
		Effect.timeoutFail({
			duration: Duration.seconds(30),
			onTimeout: () =>
				new InfraError({proposalId: proposal.id, message: "Execution timed out after 30s", durationMs: 30_000}),
		}),
		Effect.retry({schedule: infraRetryPolicy, while: (e) => e._tag === "InfraError"}),
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
) {
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
// Main orchestration
// ---------------------------------------------------------------------------

const program = Effect.gen(function* () {
	const config = yield* parseConfig
	const db = yield* connectDb(Redacted.value(config.databaseUrl))

	// Ensure DB is disconnected when we exit (success or failure)
	yield* Effect.addFinalizer(() => disconnectDb(db).pipe(Effect.tap(() => Effect.logDebug("db_disconnected"))))

	const wallet = yield* createSmartWallet({
		privateKey: config.privateKey,
		pimlicoApiKey: Redacted.value(config.pimlicoApiKey),
		executorSpaceId: config.executorSpaceId,
		spaceRegistryAddress: config.spaceRegistryAddress,
		rpcUrl: config.rpcUrl,
		chainId: config.chainId,
	})

	yield* Effect.logInfo("wallet_ready").pipe(Effect.annotateLogs({safeAddress: wallet.safeAddress}))

	yield* verifyExecutorSetup(wallet, config.executorSpaceId, config.spaceRegistryAddress)

	const runStart = Date.now()
	const nowSeconds = Math.floor(runStart / 1000)
	const proposals = yield* findExecutableProposals(db, nowSeconds)
	const bySpace = Map.groupBy(proposals, (p) => p.spaceId)

	yield* Effect.logInfo("run_start").pipe(
		Effect.annotateLogs({proposalsFound: proposals.length, spaces: bySpace.size}),
	)

	if (proposals.length === 0) {
		yield* Effect.logInfo("run_end").pipe(
			Effect.annotateLogs({succeeded: 0, failed: 0, skipped: 0, total: 0, durationMs: Date.now() - runStart}),
		)
		return {succeeded: 0, failed: 0}
	}

	// Execute spaces in parallel (unbounded — one fiber per space); sequential
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
				Effect.map((outcomes) => ({
					spaceId,
					succeeded: outcomes.filter((r) => r !== "skipped").length,
					skipped: outcomes.filter((r) => r === "skipped").length,
				})),
				// Catch InfraError at the space level so one space failing
				// doesn't cancel the others
				Effect.catchTag("InfraError", (e) =>
					Effect.logError("space_aborted").pipe(
						Effect.annotateLogs({spaceId, error: e.message, proposalId: e.proposalId}),
						Effect.as({spaceId, succeeded: 0, skipped: 0, infraError: true as const}),
					),
				),
			),
		{concurrency: "unbounded"},
	)

	const succeeded = results.reduce((n, r) => n + r.succeeded, 0)
	const failed = results.filter((r) => "infraError" in r).length
	const skipped = results.reduce((n, r) => n + r.skipped, 0)

	yield* Effect.logInfo("run_end").pipe(
		Effect.annotateLogs({
			succeeded,
			failed,
			skipped,
			total: proposals.length,
			spaces: bySpace.size,
			durationMs: Date.now() - runStart,
		}),
	)

	return {succeeded, failed}
})

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

const main = Effect.gen(function* () {
	const runId = crypto.randomUUID()

	return yield* Effect.scoped(program).pipe(
		// 270s top-level timeout — leaves 20s margin before K8s SIGKILL at 290s,
		// ensuring finalizers (DB disconnect) run and a structured log is emitted.
		Effect.timeoutFail({
			duration: Duration.seconds(270),
			onTimeout: () =>
				new InfraError({proposalId: "N/A", message: "Run timed out after 270s", durationMs: 270_000}),
		}),
		Effect.catchTag("InfraError", (e) => {
			console.error(JSON.stringify({event: "run_failed", runId, error: e.message, proposalId: e.proposalId}))
			return Effect.succeed({succeeded: 0, failed: 1})
		}),
		Effect.annotateLogs({runId}),
	)
}).pipe(
	Effect.provide(Logger.json),
	Effect.catchAllDefect((defect) => {
		console.error(JSON.stringify({event: "fatal", error: String(defect)}))
		return Effect.succeed({succeeded: 0, failed: 1})
	}),
)

Effect.runPromise(main).then(({succeeded, failed}) => {
	process.exit(failed > 0 && succeeded === 0 ? 1 : 0)
})
