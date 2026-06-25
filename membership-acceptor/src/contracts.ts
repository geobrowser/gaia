/**
 * Contract ABI, chain definitions, governance constants, encoding helpers, and
 * revert classification for casting the membership YES vote.
 *
 * Ported from proposal-executor/src/{contracts,execute}.ts (which forked geo-cli),
 * trimmed to what the acceptor needs and de-Effect-ified (plain functions).
 *
 * Why not @geoprotocol/geo-sdk: its `daoSpace.voteProposal` hardcodes the TESTNET
 * registry address and `getSmartAccountWalletClient` hardcodes chain 19411 + a
 * testnet bundler, so it cannot drive a mainnet vote in the published version.
 * We reuse the proven viem path instead.
 */

import {type Address, type Chain, encodeAbiParameters, type Hex} from "viem"

// ---------------------------------------------------------------------------
// SpaceRegistry ABI — only enter() is needed (we cast votes, never read).
// ---------------------------------------------------------------------------

export const SpaceRegistryAbi = [
	{
		inputs: [
			{internalType: "bytes16", name: "_fromSpaceId", type: "bytes16"},
			{internalType: "bytes16", name: "_toSpaceId", type: "bytes16"},
			{internalType: "bytes32", name: "_action", type: "bytes32"},
			{internalType: "bytes32", name: "_topic", type: "bytes32"},
			{internalType: "bytes", name: "_data", type: "bytes"},
			{internalType: "bytes", name: "_signature", type: "bytes"},
		],
		name: "enter",
		outputs: [],
		stateMutability: "nonpayable",
		type: "function",
	},
] as const

// ---------------------------------------------------------------------------
// Chain definitions
// ---------------------------------------------------------------------------

export const mainnetChain: Chain = {
	id: 80451,
	name: "Geo Genesis",
	nativeCurrency: {name: "Ethereum", symbol: "ETH", decimals: 18},
	rpcUrls: {default: {http: ["https://rpc-geo-genesis-h0q2s21xx8.t.conduit.xyz"]}},
}

export const testnetChain: Chain = {
	id: 19411,
	name: "Geo Genesis Testnet",
	nativeCurrency: {name: "Ethereum", symbol: "ETH", decimals: 18},
	rpcUrls: {default: {http: ["https://rpc-geo-test-zc16z3tcvf.t.conduit.xyz"]}},
}

export type SupportedChainId = 80451 | 19411

export function getChain(chainId: SupportedChainId): Chain {
	if (chainId === 80451) return mainnetChain
	if (chainId === 19411) return testnetChain
	const _exhaustive: never = chainId
	throw new Error(`Unsupported chain ID: ${_exhaustive}`)
}

/** Safe deployment addresses for Geo Testnet (canonical addresses differ there). */
export const TESTNET_SAFE_ADDRESSES = {
	safeModuleSetupAddress: "0x2dd68b007B46fBe91B9A7c3EDa5A7a1063cB5b47" as const,
	safe4337ModuleAddress: "0x75cf11467937ce3F2f357CE24ffc3DBF8fD5c226" as const,
	safeProxyFactoryAddress: "0xd9d2Ba03a7754250FDD71333F444636471CACBC4" as const,
	safeSingletonAddress: "0x639245e8476E03e789a244f279b5843b9633b2E7" as const,
	multiSendAddress: "0x7B21BBDBdE8D01Df591fdc2dc0bE9956Dde1e16C" as const,
	multiSendCallOnlyAddress: "0x32228dDEA8b9A2bd7f2d71A958fF241D79ca5eEC" as const,
}

// ---------------------------------------------------------------------------
// Governance constants
// ---------------------------------------------------------------------------

/** keccak256('GOVERNANCE.PROPOSAL_VOTED') — action hash for casting a vote via enter() */
export const PROPOSAL_VOTED_ACTION = "0x4ebf5f29676cedf7e2e4d346a8433289278f95a9fda73691dc1ce24574d5819e" as Hex

/** IDAOSpace.VoteOption.Yes (enum: None=0, Yes=1, No=2, Abstain=3). */
export const VOTE_YES = 1

/** Empty signature — ignored when msg.sender == _fromSpace. */
export const EMPTY_SIGNATURE = "0x" as Hex

// ---------------------------------------------------------------------------
// Encoding helpers
// ---------------------------------------------------------------------------

/**
 * Convert a UUID string to a bytes16 hex value (strip dashes, prefix 0x).
 * @example uuidToBytes16("550e8400-e29b-41d4-a716-446655440000") → "0x550e8400e29b41d4a716446655440000"
 */
export function uuidToBytes16(uuid: string): Hex {
	const stripped = uuid.replace(/-/g, "")
	if (stripped.length !== 32 || !/^[0-9a-fA-F]+$/.test(stripped)) {
		throw new Error(`Invalid UUID for bytes16 conversion: ${uuid}`)
	}
	return `0x${stripped}` as Hex
}

/** Pad a bytes16 hex (32 hex chars) to bytes32 (64 hex chars) with trailing zeros. */
export function padBytes16ToBytes32(bytes16Hex: Hex): Hex {
	const withoutPrefix = bytes16Hex.startsWith("0x") ? bytes16Hex.slice(2) : bytes16Hex
	if (withoutPrefix.length !== 32) {
		throw new Error(`Invalid bytes16: expected 32 hex chars, got ${withoutPrefix.length}`)
	}
	return `0x${withoutPrefix}${"0".repeat(32)}` as Hex
}

/**
 * ABI-encode the PROPOSAL_VOTED data payload: abi.encode(bytes16 proposalId, uint8 voteOption).
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
// Revert classification
// ---------------------------------------------------------------------------

/**
 * viem/permissionless error names that indicate an on-chain revert. The chain
 * rejecting a vote (already voted / executed / window closed / not an editor) is
 * a *benign* outcome for us — there is nothing to retry — so we must distinguish
 * it from an infrastructure error (RPC/bundler hiccup) which SHOULD be retried.
 */
const REVERT_ERROR_NAMES = new Set([
	"ContractFunctionRevertedError",
	"ContractFunctionExecutionError",
	"CallExecutionError",
])

const REVERT_MESSAGE_PATTERNS = ["revert", "execution reverted", "CALL_EXCEPTION", "UserOperation reverted"]

/** True if `error` looks like an on-chain revert (vs. an infrastructure failure). */
export function isRevert(error: unknown): boolean {
	const name = error instanceof Error ? error.name : typeof error
	if (REVERT_ERROR_NAMES.has(name)) return true

	if (error instanceof Error && error.cause instanceof Error && REVERT_ERROR_NAMES.has(error.cause.name)) {
		return true
	}

	const message = String(error)
	return REVERT_MESSAGE_PATTERNS.some((p) => message.includes(p))
}

/** Never leak a bundler URL's API key into a log/error message. */
export function sanitizeError(error: unknown): string {
	return String(error).replace(/apikey=[^\s&"]+/gi, "apikey=<redacted>")
}

export type {Address}
