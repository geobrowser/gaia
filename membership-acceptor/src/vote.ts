/**
 * Casting the membership YES vote.
 *
 * Build the enter(PROPOSAL_VOTED, Yes) calldata and send it.
 * The contract is the authority — a duplicate / closed /
 * already-executed / unauthorized vote simply reverts, which we classify as a
 * *benign* outcome (nothing to retry). Only genuine infrastructure failures are
 * retryable.
 *
 * Because fast-path spaces run with flatThreshold = 1, the single YES executes the
 * AddMember in the same transaction — admitting the member.
 */

import {type Address, encodeFunctionData} from "viem"

import {
	EMPTY_SIGNATURE,
	encodeVoteData,
	isRevert,
	PROPOSAL_VOTED_ACTION,
	padBytes16ToBytes32,
	SpaceRegistryAbi,
	sanitizeError,
	uuidToBytes16,
	VOTE_YES,
} from "./contracts.js"
import type {MembershipRequest} from "./detect.js"
import type {GraphQLClient} from "./graphql.js"
import type {Policy, PolicyDecision} from "./policy.js"
import type {SmartWallet} from "./wallet.js"

/**
 * Outcome of attempting a vote.
 * - `voted`  — the YES vote landed (member admitted). txHash for tracing.
 * - `benign` — the chain rejected it (already voted / executed / closed / not an
 *              editor). Nothing to retry; ack the webhook so it stops retrying.
 * - `infra`  — an infrastructure failure (RPC/bundler). The webhook should retry.
 */
export type VoteResult =
	| {kind: "voted"; txHash: string}
	| {kind: "benign"; message: string}
	| {kind: "infra"; message: string}

/**
 * The acceptor's view used by the HTTP layer: which spaces it serves, and how to
 * vote. Injected into `createApp` so tests can stub it without a real wallet.
 */
export interface Acceptor {
	/** Cheap config gate: true if this acceptor serves `spaceId` (the whitelist). */
	allowsSpace(spaceId: string): boolean
	/** Run the policy seam (API-backed rules, e.g. the editor check) for a request. */
	evaluate(request: MembershipRequest): Promise<PolicyDecision>
	/** Cast the YES vote for a detected, allowed, accepted, not-yet-seen request. */
	vote(request: MembershipRequest): Promise<VoteResult>
}

export interface AcceptorConfig {
	wallet: SmartWallet
	/** The acceptor's personal-space id (bytes16 hex) — the enter() `_fromSpaceId`. */
	acceptorSpaceId: `0x${string}`
	spaceRegistryAddress: Address
	/** Spaces (UUIDs, lowercased) this acceptor auto-accepts. Empty ⇒ accept none. */
	allowlist: ReadonlySet<string>
	/** The (possibly composed) policy run before voting. */
	policy: Policy
	/** GraphQL client passed to policies for API-backed decisions. */
	graphql: GraphQLClient
}

/** Build the real, wallet-backed acceptor. */
export function createAcceptor(config: AcceptorConfig): Acceptor {
	return {
		allowsSpace(spaceId: string): boolean {
			return config.allowlist.has(spaceId.toLowerCase())
		},

		evaluate(request: MembershipRequest): Promise<PolicyDecision> {
			return config.policy(request, {graphql: config.graphql, acceptorSpaceId: config.acceptorSpaceId})
		},

		async vote(request: MembershipRequest): Promise<VoteResult> {
			try {
				const daoSpaceId = uuidToBytes16(request.spaceId)
				const proposalIdHex = uuidToBytes16(request.proposalId)

				const calldata = encodeFunctionData({
					abi: SpaceRegistryAbi,
					functionName: "enter",
					args: [
						config.acceptorSpaceId, // _fromSpaceId: acceptor's personal space
						daoSpaceId, // _toSpaceId: DAO space being joined
						PROPOSAL_VOTED_ACTION, // _action
						padBytes16ToBytes32(proposalIdHex), // _topic: bytes32(proposalId)
						encodeVoteData(proposalIdHex, VOTE_YES), // _data: abi.encode(proposalId, Yes)
						EMPTY_SIGNATURE, // _signature: unused when msg.sender == _fromSpace
					],
				})

				const account = config.wallet.smartAccountClient.account
				if (!account) {
					// Misconfigured wallet — retrying won't help, but it's not a chain
					// revert either; surface as infra so it's loud and retried.
					return {kind: "infra", message: "smart account client has no account"}
				}

				const txHash = await config.wallet.smartAccountClient.sendTransaction({
					account,
					chain: config.wallet.chain,
					to: config.spaceRegistryAddress,
					data: calldata,
				})
				return {kind: "voted", txHash}
			} catch (error) {
				const message = sanitizeError(error)
				return isRevert(error) ? {kind: "benign", message} : {kind: "infra", message}
			}
		},
	}
}
