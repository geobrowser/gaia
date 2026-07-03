/**
 * Policy seam — the extension point for membership acceptors.
 *
 * A {@link Policy} decides whether a detected, allowlisted membership request
 * should be accepted (voted YES). Policies receive a {@link PolicyContext} with a
 * GraphQL client, so a space owner can express arbitrary rules backed by API data
 * (editor status, reputation, payment, external auth, …) without touching the
 * webhook/voting plumbing.
 *
 * The reference policy shipped here is {@link editorPolicy}: it confirms the
 * acceptor is actually an editor of the target space (otherwise the vote would
 * just revert on-chain). It fails OPEN — an API error never suppresses a vote;
 * the chain remains the final authority.
 */

import {sanitizeError} from "./contracts.js"
import type {MembershipRequest} from "./detect.js"
import type {GraphQLClient} from "./graphql.js"

export interface PolicyContext {
	graphql: GraphQLClient
	/** The acceptor's personal-space id (bytes16 hex, 0x-prefixed). */
	acceptorSpaceId: string
}

export interface PolicyDecision {
	accept: boolean
	/** Human-readable explanation, logged on both accept and deny. */
	reason: string
}

export type Policy = (request: MembershipRequest, ctx: PolicyContext) => Promise<PolicyDecision>

/**
 * Compose policies with AND semantics: every policy must accept. The first denial
 * short-circuits and is returned (later policies are not run).
 */
export function composePolicies(...policies: Policy[]): Policy {
	return async (request, ctx) => {
		for (const policy of policies) {
			const decision = await policy(request, ctx)
			if (!decision.accept) return decision
		}
		return {accept: true, reason: "all policies passed"}
	}
}

interface EditorQueryResult {
	editor: {memberSpaceId: string} | null
}

/**
 * Accept iff the acceptor's space is an editor of the request's DAO space.
 *
 * Queries `editor(spaceId, memberSpaceId)` — a non-null result means the acceptor
 * can vote. The API accepts ids dashed or dashless, so `request.spaceId` is passed
 * as-is (and it is already an allowlisted, operator-controlled value by this point,
 * so it is safe to inline); only the `0x` prefix is stripped off the acceptor's
 * bytes16 space id, which the query does not expect.
 *
 * Fails OPEN: if the query errors, we accept and let the on-chain vote be the
 * authority (a genuine non-editor simply reverts).
 */
export const editorPolicy: Policy = async (request, ctx) => {
	const spaceId = request.spaceId
	const memberSpaceId = ctx.acceptorSpaceId.replace(/^0x/i, "")

	const query = `query IsEditor { editor(spaceId: "${spaceId}", memberSpaceId: "${memberSpaceId}") { memberSpaceId } }`

	try {
		const data = await ctx.graphql.query<EditorQueryResult>(query)
		if (data.editor) {
			return {accept: true, reason: "acceptor is an editor of the space"}
		}
		return {accept: false, reason: `acceptor space ${memberSpaceId} is not an editor of ${spaceId}`}
	} catch (err) {
		// Fail open — never let API trouble block a legitimate vote.
		return {accept: true, reason: `editor check skipped (api error): ${sanitizeError(err)}`}
	}
}
