/**
 * Smart wallet setup, encoding helpers, and on-chain proposal execution.
 *
 * Smart wallet pattern forked from geo-cli/src/wallet.ts:191-261.
 * Encoding helpers forked from geo-cli/src/governance.ts.
 *
 * See: docs/plans/2026-03-02-feat-proposal-auto-executor-plan.md §On-Chain Execution
 */

import {
	createKernelAccount,
	createKernelAccountClient,
	createZeroDevPaymasterClient,
	getUserOperationGasPrice,
} from "@zerodev/sdk"
import {getEntryPoint, KERNEL_V3_3} from "@zerodev/sdk/constants"
import {Effect} from "effect"
import {createSmartAccountClient, type SmartAccountClient} from "permissionless"
import {toSafeSmartAccount} from "permissionless/accounts"
import {createPimlicoClient} from "permissionless/clients/pimlico"
import {
	type Address,
	type Chain,
	createPublicClient,
	encodeAbiParameters,
	encodeFunctionData,
	type Hex,
	http,
	type PublicClient,
} from "viem"
import {entryPoint07Address} from "viem/account-abstraction"
import {privateKeyToAccount} from "viem/accounts"
import {
	DaoSpaceAbi,
	EMPTY_SIGNATURE,
	getChain,
	InfraError,
	PROPOSAL_EXECUTED_ACTION,
	RevertError,
	SpaceRegistryAbi,
	type SupportedChainId,
	TESTNET_SAFE_ADDRESSES,
} from "./contracts.js"
import type {Proposal} from "./detect.js"

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface SmartWallet {
	readonly smartAccountClient: SmartAccountClient
	readonly publicClient: PublicClient
	readonly chain: Chain
	readonly accountAddress: Address
}

export interface ExecutorConfig {
	privateKey: `0x${string}`
	pimlicoApiKey: string
	executorSpaceId: Hex
	spaceRegistryAddress: Address
	rpcUrl: string
	chainId: SupportedChainId
	/**
	 * ZeroDev bundler+paymaster endpoint (embeds a project ID) — required only
	 * when chainId is 55516. That chain (a Conduit rollup) has ZeroDev's Kernel
	 * v0.3.3 + EntryPoint v0.7 contracts deployed by default but no Safe infra
	 * at all, so it uses an EIP-7702 Kernel account via ZeroDev's own bundler
	 * instead of the Safe+Pimlico path used on 19411/80451 — the same pattern
	 * geogenesis's frontend (mainnet-migration-v020, @geoprotocol/geo-sdk)
	 * already uses for this chain. Unlike a Safe/classic-Kernel proxy, the
	 * resulting account address IS the raw EOA derived from `privateKey`.
	 */
	zerodevSponsorshipRpcUrl?: string
}

// ---------------------------------------------------------------------------
// Encoding helpers
// Source: geo-cli/src/governance.ts
// ---------------------------------------------------------------------------

/**
 * Convert a UUID string to a bytes16 hex value.
 * Strips dashes and prefixes with 0x.
 *
 * @example uuidToBytes16("550e8400-e29b-41d4-a716-446655440000")
 *          → "0x550e8400e29b41d4a716446655440000"
 */
export function uuidToBytes16(uuid: string): Hex {
	const stripped = uuid.replace(/-/g, "")
	if (stripped.length !== 32 || !/^[0-9a-fA-F]+$/.test(stripped)) {
		throw new Error(`Invalid UUID for bytes16 conversion: ${uuid}`)
	}
	return `0x${stripped}` as Hex
}

/**
 * Pad a bytes16 hex string (32 hex chars) to bytes32 (64 hex chars) with trailing zeros.
 * Source: geo-cli/src/governance.ts:94-112
 */
export function padBytes16ToBytes32(bytes16Hex: Hex): Hex {
	const withoutPrefix = bytes16Hex.startsWith("0x") ? bytes16Hex.slice(2) : bytes16Hex
	if (withoutPrefix.length !== 32) {
		throw new Error(`Invalid bytes16: expected 32 hex chars, got ${withoutPrefix.length}`)
	}
	return `0x${withoutPrefix}${"0".repeat(32)}` as Hex
}

/**
 * Encode the data payload for a PROPOSAL_EXECUTED action.
 * Source: geo-cli/src/governance.ts:177-179
 */
export function encodeProposalExecutedData(proposalIdHex: Hex): Hex {
	return encodeAbiParameters([{name: "proposalId", type: "bytes16"}], [proposalIdHex])
}

// ---------------------------------------------------------------------------
// Smart wallet creation
// Source: geo-cli/src/wallet.ts:191-261
// ---------------------------------------------------------------------------

/**
 * Create a gas-sponsored smart account. Called once per CronJob run.
 *
 * Chain 55516 (testnet v2) has no Safe infra deployed, so it uses an
 * EIP-7702 Kernel account via ZeroDev's bundler instead of the Safe+Pimlico
 * path used everywhere else. See the `zerodevSponsorshipRpcUrl` doc comment
 * on `ExecutorConfig` for why.
 */
export function createSmartWallet(config: ExecutorConfig): Effect.Effect<SmartWallet, InfraError> {
	return Effect.tryPromise({
		try: async () => {
			const chain = getChain(config.chainId)

			const publicClient = createPublicClient({
				chain,
				transport: http(config.rpcUrl),
			})

			const owner = privateKeyToAccount(config.privateKey)

			if (config.chainId === 55516) {
				if (!config.zerodevSponsorshipRpcUrl) {
					throw new Error("zerodevSponsorshipRpcUrl is required when chainId is 55516")
				}

				const entryPoint = getEntryPoint("0.7")
				const kernelAccount = await createKernelAccount(publicClient, {
					eip7702Account: owner,
					entryPoint,
					kernelVersion: KERNEL_V3_3,
				})

				const bundlerTransport = http(config.zerodevSponsorshipRpcUrl)
				const paymasterClient = createZeroDevPaymasterClient({
					chain,
					transport: bundlerTransport,
				})

				const smartAccountClient = createKernelAccountClient({
					account: kernelAccount,
					chain,
					client: publicClient,
					bundlerTransport,
					paymaster: {
						getPaymasterStubData: (userOperation) =>
							paymasterClient.sponsorUserOperation({userOperation, shouldConsume: false}),
						getPaymasterData: (userOperation) => paymasterClient.sponsorUserOperation({userOperation}),
					},
					userOperation: {
						estimateFeesPerGas: async ({bundlerClient}) => getUserOperationGasPrice(bundlerClient),
					},
				})

				return {
					// KernelAccountClient and permissionless's SmartAccountClient both
					// expose the `.account`/`.sendTransaction(...)` shape executeProposal
					// and castMembershipVote actually use — that's the only shape SmartWallet
					// depends on across the codebase (verified: no other member is accessed).
					smartAccountClient: smartAccountClient as unknown as SmartAccountClient,
					publicClient,
					chain,
					accountAddress: kernelAccount.address,
				} satisfies SmartWallet
			}

			const bundlerUrl = `https://api.pimlico.io/v2/${config.chainId}/rpc?apikey=${config.pimlicoApiKey}`

			const safeAccount = await toSafeSmartAccount({
				client: publicClient,
				owners: [owner],
				entryPoint: {
					address: entryPoint07Address,
					version: "0.7" as const,
				},
				version: "1.4.1" as const,
				...(config.chainId === 19411 ? TESTNET_SAFE_ADDRESSES : {}),
			})

			const bundlerTransport = http(bundlerUrl)
			const paymasterClient = createPimlicoClient({
				transport: bundlerTransport,
				chain,
				entryPoint: {
					address: entryPoint07Address,
					version: "0.7",
				},
			})

			const smartAccountClient = createSmartAccountClient({
				chain,
				account: safeAccount,
				paymaster: paymasterClient,
				bundlerTransport,
				userOperation: {
					estimateFeesPerGas: async () => {
						return (await paymasterClient.getUserOperationGasPrice()).fast
					},
				},
			})

			return {
				smartAccountClient,
				publicClient,
				chain,
				accountAddress: safeAccount.address,
			} satisfies SmartWallet
		},
		catch: (error) => new InfraError({message: `Smart wallet creation failed: ${error}`, durationMs: 0}),
	})
}

// ---------------------------------------------------------------------------
// Executor space verification
// ---------------------------------------------------------------------------

/**
 * Verify the executor's personal space is registered on-chain.
 * Calls addressToSpaceId(accountAddress) — if it returns zero bytes, the
 * personal space hasn't been created yet and all executions will revert.
 */
export function verifyExecutorSetup(
	wallet: SmartWallet,
	executorSpaceId: Hex,
	spaceRegistryAddress: Address,
): Effect.Effect<void, InfraError> {
	return Effect.tryPromise({
		try: async () => {
			const spaceId = await wallet.publicClient.readContract({
				address: spaceRegistryAddress,
				abi: SpaceRegistryAbi,
				functionName: "addressToSpaceId",
				args: [wallet.accountAddress],
			})

			const ZERO_BYTES16 = "0x00000000000000000000000000000000"
			if (spaceId === ZERO_BYTES16) {
				throw new Error(
					`Executor account ${wallet.accountAddress} has no registered personal space. ` +
						"Register one with 'geo space create' before running the executor.",
				)
			}

			// Verify it matches the configured EXECUTOR_SPACE_ID
			if (spaceId.toLowerCase() !== executorSpaceId.toLowerCase()) {
				throw new Error(
					`On-chain space ID ${spaceId} for account ${wallet.accountAddress} ` +
						`does not match configured EXECUTOR_SPACE_ID ${executorSpaceId}`,
				)
			}
		},
		catch: (error) => new InfraError({message: `Executor setup verification failed: ${error}`, durationMs: 0}),
	})
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/**
 * Known viem/permissionless error names that indicate an on-chain revert.
 * Checked first (structured), before falling back to string matching (pragmatic).
 *
 * Error chain for UserOp reverts: Pimlico RPC → permissionless →
 * UserOperationExecutionError (cause: ContractFunctionRevertedError).
 * We check both the outer and inner error names.
 */
const REVERT_ERROR_NAMES = new Set([
	"ContractFunctionRevertedError",
	"ContractFunctionExecutionError",
	"CallExecutionError",
])

/** Fallback string patterns when structured error types aren't available */
const REVERT_MESSAGE_PATTERNS = ["revert", "execution reverted", "CALL_EXCEPTION", "UserOperation reverted"]

export function classifyAsRevert(error: unknown, errorName: string, message: string): boolean {
	// 1. Check structured error name (most reliable)
	if (REVERT_ERROR_NAMES.has(errorName)) return true

	// 2. Walk the cause chain — UserOperationExecutionError wraps the revert as .cause
	if (error instanceof Error && "cause" in error && error.cause instanceof Error) {
		if (REVERT_ERROR_NAMES.has(error.cause.name)) return true
	}

	// 3. Fallback: string matching on the message (covers raw bundler RPC errors)
	return REVERT_MESSAGE_PATTERNS.some((p) => message.includes(p))
}

// ---------------------------------------------------------------------------
// Proposal execution
// ---------------------------------------------------------------------------

/**
 * Execute a single proposal on-chain by calling enter(PROPOSAL_EXECUTED).
 *
 * Returns the transaction hash on success. Classifies errors:
 * - On-chain reverts → RevertError (with expected=true for "already executed")
 * - Infrastructure failures → InfraError
 *
 * Retry and timeout are composed externally in index.ts.
 */
/**
 * Raised by the pre-flight check when the DAO reports the proposal cannot
 * execute. A named class rather than a message match, so classification does
 * not depend on error text we do not control.
 */
class NotExecutableError extends Error {
	constructor() {
		super("canExecuteProposal returned false — skipped before simulation")
		this.name = "NotExecutableError"
	}
}

export function executeProposal(
	wallet: SmartWallet,
	proposal: Proposal,
	executorSpaceId: Hex,
	spaceRegistryAddress: Address,
): Effect.Effect<string, RevertError | InfraError> {
	// Effect.suspend ensures `start` is captured fresh on each retry attempt,
	// so durationMs reflects only the current attempt's wall time.
	return Effect.suspend(() => {
		const start = Date.now()

		return Effect.tryPromise({
			try: async () => {
				const daoSpaceId = uuidToBytes16(proposal.spaceId)
				const proposalIdHex = uuidToBytes16(proposal.id)

				// Ask the DAO whether this can execute before paying to find out.
				//
				// `_executeProposal` reverts with CanNotExecute() (0xdf322356) when
				// `canExecuteProposal` is false. Reaching that revert costs a sponsored
				// UserOperation simulation and logs `proposal_reverted`, which reads as
				// a fault. Checking first is a plain eth_call and costs nothing.
				//
				// This is not a race guard — a proposal can still be executed by someone
				// else between the check and the send, which the RevertError path below
				// still handles. It exists because the migration left proposals in the
				// database that were never created on chain 55516, so they can NEVER
				// execute: `creator == bytes16(0)`. Without this, each one burns a
				// sponsored simulation every run, forever. See gaia notes on
				// v2-proposals-absent-on-chain.
				const daoAddress = await wallet.publicClient.readContract({
					address: spaceRegistryAddress,
					abi: SpaceRegistryAbi,
					functionName: "spaceIdToAddress",
					args: [daoSpaceId],
				})

				const canExecute = await wallet.publicClient.readContract({
					address: daoAddress,
					abi: DaoSpaceAbi,
					functionName: "canExecuteProposal",
					args: [proposalIdHex],
				})

				if (!canExecute) {
					// Deliberately a RevertError with expected: true — this is a skip, not
					// a failure, and the caller already treats expected reverts as such.
					throw new NotExecutableError()
				}

				const calldata = encodeFunctionData({
					abi: SpaceRegistryAbi,
					functionName: "enter",
					args: [
						executorSpaceId, // bytes16: executor's personal space ID
						daoSpaceId, // bytes16: DAO space ID
						PROPOSAL_EXECUTED_ACTION, // bytes32: action hash
						padBytes16ToBytes32(proposalIdHex), // bytes32: proposalId padded
						encodeProposalExecutedData(proposalIdHex), // bytes: ABI-encoded proposalId
						EMPTY_SIGNATURE, // bytes: "0x" (ignored when msg.sender == _fromSpace)
					],
				})

				const account = wallet.smartAccountClient.account
				if (!account) {
					throw new Error("Smart account client has no account — wallet was not initialized correctly")
				}

				const hash = await wallet.smartAccountClient.sendTransaction({
					account,
					chain: wallet.chain,
					to: spaceRegistryAddress,
					data: calldata,
				})

				return hash
			},
			catch: (error) => {
				const durationMs = Date.now() - start
				// Sanitize: strip Pimlico API key from bundler URL if present in error message.
				// Defense in depth — viem may not include URLs in errors today, but we don't
				// control their error formatting across versions.
				const message = String(error).replace(/apikey=[^\s&"]+/gi, "apikey=<redacted>")
				// Capture error type for observability — helps tighten classification over time
				const errorName = error instanceof Error ? error.name : typeof error

				const isRevert = error instanceof NotExecutableError || classifyAsRevert(error, errorName, message)

				if (isRevert) {
					// Expected reverts: proposal already executed (race condition window)
					const isExpected =
						error instanceof NotExecutableError ||
						message.includes("already executed") ||
						message.includes("ProposalAlreadyExecuted") ||
						message.includes("InvalidAction")
					return new RevertError({
						proposalId: proposal.id,
						message: `[${errorName}] ${message}`,
						expected: isExpected,
						durationMs,
					})
				}

				return new InfraError({
					proposalId: proposal.id,
					message: `[${errorName}] ${message}`,
					durationMs,
				})
			},
		})
	})
}
