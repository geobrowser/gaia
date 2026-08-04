/**
 * Contract ABIs, chain definitions, and governance constants for the proposal executor.
 *
 * FORKED from geo-cli (v0.1.0):
 * - ABI subset: geo-cli/src/contracts.ts (SpaceRegistryAbi — enter, addressToSpaceId;
 *   spaceIdToAddress added locally for the membership-accept path)
 * - Chain defs: geo-cli/src/network.ts (mainnetChain, testnetChain)
 * - Governance: geo-cli/src/governance.ts (GOVERNANCE_ACTIONS, encoding helpers)
 * - Safe addrs: geo-cli/src/wallet.ts:54-61 (TESTNET_SAFE_ADDRESSES)
 *
 * Long-term consolidation: extract a shared @geo/protocol package when a third consumer appears.
 */

import {Data} from "effect"
import type {Chain, Hex} from "viem"

// ---------------------------------------------------------------------------
// Tagged errors — defined here (leaf dependency) to avoid circular imports
// ---------------------------------------------------------------------------

/** On-chain revert — proposal is skipped and execution continues */
export class RevertError extends Data.TaggedError("RevertError")<{
	proposalId: string
	message: string
	expected: boolean
	durationMs: number
}> {}

/** Infrastructure failure — retried per-proposal, then space is aborted */
export class InfraError extends Data.TaggedError("InfraError")<{
	/** Omitted for run-level failures (config, timeout) that aren't tied to a proposal */
	proposalId?: string
	message: string
	durationMs: number
}> {}

// ---------------------------------------------------------------------------
// SpaceRegistry ABI — minimal subset (enter + addressToSpaceId + spaceIdToAddress)
// Source: geo-cli/src/contracts.ts (enter, addressToSpaceId);
//         spaceIdToAddress added locally to resolve a space's DAOSpace address.
// ---------------------------------------------------------------------------

export const SpaceRegistryAbi = [
	{
		inputs: [{internalType: "address", name: "_account", type: "address"}],
		name: "addressToSpaceId",
		outputs: [{internalType: "bytes16", name: "_spaceId", type: "bytes16"}],
		stateMutability: "view",
		type: "function",
	},
	{
		inputs: [{internalType: "bytes16", name: "_spaceId", type: "bytes16"}],
		name: "spaceIdToAddress",
		outputs: [{internalType: "address", name: "_account", type: "address"}],
		stateMutability: "view",
		type: "function",
	},
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
// DAOSpace ABI — minimal subset (only getLatestProposalInformation)
// Source: geo-contracts-foundry/src/interfaces/IDAOSpace.sol
//
// The vote-tally getters live on the per-space DAOSpace contract (resolved via
// SpaceRegistry.spaceIdToAddress), NOT on the SpaceRegistry. The membership
// path reads this for the stage-2 authoritative "untouched" check.
// ---------------------------------------------------------------------------

export const DAOSpaceAbi = [
	{
		inputs: [{internalType: "bytes16", name: "_proposalId", type: "bytes16"}],
		name: "getLatestProposalInformation",
		outputs: [
			{internalType: "bool", name: "_executed", type: "bool"},
			{internalType: "bytes16", name: "_creator", type: "bytes16"},
			{
				components: [
					{internalType: "enum IDAOSpace.VotingMode", name: "votingMode", type: "uint8"},
					{internalType: "uint256", name: "supportThreshold", type: "uint256"},
					{internalType: "uint256", name: "quorum", type: "uint256"},
					{internalType: "uint256", name: "startDate", type: "uint256"},
					{internalType: "uint256", name: "lastDate", type: "uint256"},
				],
				internalType: "struct IDAOSpace.ProposalParameters",
				name: "_parameters",
				type: "tuple",
			},
			{
				components: [
					{internalType: "uint256", name: "yes", type: "uint256"},
					{internalType: "uint256", name: "no", type: "uint256"},
					{internalType: "uint256", name: "abstain", type: "uint256"},
				],
				internalType: "struct IDAOSpace.Tally",
				name: "_tally",
				type: "tuple",
			},
			{
				components: [
					{internalType: "address", name: "toAddress", type: "address"},
					{internalType: "bytes16", name: "toSpaceId", type: "bytes16"},
					{internalType: "uint256", name: "value", type: "uint256"},
					{internalType: "bytes", name: "data", type: "bytes"},
				],
				internalType: "struct IDAOSpace.Action[]",
				name: "_actions",
				type: "tuple[]",
			},
		],
		stateMutability: "view",
		type: "function",
	},
] as const

// ---------------------------------------------------------------------------
// Chain definitions
// Source: geo-cli/src/network.ts
// ---------------------------------------------------------------------------

export const mainnetChain: Chain = {
	id: 80451,
	name: "Geo Genesis",
	nativeCurrency: {name: "Ethereum", symbol: "ETH", decimals: 18},
	rpcUrls: {
		default: {http: ["https://rpc-geo-genesis-h0q2s21xx8.t.conduit.xyz"]},
	},
}

export const testnetChain: Chain = {
	id: 19411,
	name: "Geo Genesis Testnet",
	nativeCurrency: {name: "Ethereum", symbol: "ETH", decimals: 18},
	rpcUrls: {
		default: {http: ["https://rpc-geo-test-zc16z3tcvf.t.conduit.xyz"]},
	},
}

export const testnetV2Chain: Chain = {
	id: 55516,
	name: "Geo Testnet",
	nativeCurrency: {name: "Ethereum", symbol: "ETH", decimals: 18},
	rpcUrls: {
		default: {http: ["https://rpc-geo-testnet-irdc0cgb0w.t.conduit.xyz"]},
	},
}

export type SupportedChainId = 80451 | 19411 | 55516

export function getChain(chainId: SupportedChainId): Chain {
	if (chainId === 80451) return mainnetChain
	if (chainId === 19411) return testnetChain
	if (chainId === 55516) return testnetV2Chain
	// Exhaustiveness check — TypeScript will error here if a new chain ID is added
	// to SupportedChainId without a corresponding branch above.
	const _exhaustive: never = chainId
	throw new Error(`Unsupported chain ID: ${_exhaustive}`)
}

// ---------------------------------------------------------------------------
// Testnet Safe deployment addresses
// Source: geo-cli/src/wallet.ts:54-61
// ---------------------------------------------------------------------------

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
// Source: geo-cli/src/governance.ts
// ---------------------------------------------------------------------------

/** keccak256('GOVERNANCE.PROPOSAL_EXECUTED') — action hash for enter() */
/**
 * DAOSpace ABI — the single view we need to avoid wasting sponsored simulations.
 *
 * `canExecuteProposal` is the exact predicate `_executeProposal` checks before
 * it reverts with `CanNotExecute()` (0xdf322356). Calling it first is a free
 * `eth_call`; letting it fail instead costs a sponsored UserOperation
 * simulation and produces a `proposal_reverted` log that reads like a fault.
 *
 * See geo-migration `contracts/src/contracts/DAOSpace.sol`.
 */
export const DaoSpaceAbi = [
	{
		inputs: [{internalType: "bytes16", name: "_proposalId", type: "bytes16"}],
		name: "canExecuteProposal",
		outputs: [{internalType: "bool", name: "_canExecuteProposal", type: "bool"}],
		stateMutability: "view",
		type: "function",
	},
] as const

export const PROPOSAL_EXECUTED_ACTION = "0x62a60c0a9681612871e0dafa0f24bb0c83cbdde8be5a6299979c88d382369e96" as Hex

/** keccak256('GOVERNANCE.PROPOSAL_VOTED') — action hash for casting a vote via enter() */
export const PROPOSAL_VOTED_ACTION = "0x4ebf5f29676cedf7e2e4d346a8433289278f95a9fda73691dc1ce24574d5819e" as Hex

/**
 * IDAOSpace.VoteOption.Yes — the on-chain enum value for a YES vote.
 * Authoritative enum (geo-contracts-foundry IDAOSpace): None=0, Yes=1, No=2, Abstain=3.
 */
export const VOTE_YES = 1

/** Empty signature — ignored when msg.sender == _fromSpace */
export const EMPTY_SIGNATURE = "0x" as Hex

/**
 * RATIO_BASE from the governance contract. Used in threshold calculations.
 * Source: api/src/proposals/types.ts — RATIO_BASE = 10_000_000n
 * Inlined as a number for SQL literal comparison.
 */
export const RATIO_BASE = 10_000_000
