import {describe, expect, test} from "bun:test"

import type {MembershipRequest} from "../src/detect.js"
import type {GraphQLClient} from "../src/graphql.js"
import {composePolicies, editorPolicy, type Policy, type PolicyContext} from "../src/policy.js"

const REQUEST: MembershipRequest = {
	proposalId: "c3e4f5a6-0000-0000-0000-000000000000",
	spaceId: "a19c345a-b986-6679-b001-d7d2138d88a1",
	requesterSpaceId: "1310f810-454c-d482-e35c-e81cb86ca383",
}
const ACCEPTOR_SPACE_ID = `0x${"a".repeat(32)}`

/** A GraphQL client whose `query` is stubbed. */
function graphqlReturning(impl: (query: string) => unknown): GraphQLClient {
	return {query: async (query: string) => impl(query) as never}
}

function ctxWith(graphql: GraphQLClient): PolicyContext {
	return {graphql, acceptorSpaceId: ACCEPTOR_SPACE_ID}
}

describe("editorPolicy", () => {
	test("accepts when the editor query returns a record", async () => {
		const graphql = graphqlReturning(() => ({editor: {memberSpaceId: "a".repeat(32)}}))
		const decision = await editorPolicy(REQUEST, ctxWith(graphql))
		expect(decision.accept).toBe(true)
	})

	test("denies when the editor query returns null", async () => {
		const graphql = graphqlReturning(() => ({editor: null}))
		const decision = await editorPolicy(REQUEST, ctxWith(graphql))
		expect(decision.accept).toBe(false)
	})

	test("inlines spaceId as-is and the acceptor id with 0x stripped", async () => {
		let sentQuery = ""
		const graphql = graphqlReturning((q) => {
			sentQuery = q
			return {editor: {memberSpaceId: "x"}}
		})
		await editorPolicy(REQUEST, ctxWith(graphql))
		expect(sentQuery).toContain('spaceId: "a19c345a-b986-6679-b001-d7d2138d88a1"') // dashed, as-is
		expect(sentQuery).toContain(`memberSpaceId: "${"a".repeat(32)}"`) // 0x stripped
	})

	test("fails OPEN when the query throws (API error)", async () => {
		const graphql = graphqlReturning(() => {
			throw new Error("api down")
		})
		const decision = await editorPolicy(REQUEST, ctxWith(graphql))
		expect(decision.accept).toBe(true)
		expect(decision.reason).toContain("editor check skipped")
	})
})

describe("composePolicies", () => {
	const accept: Policy = async () => ({accept: true, reason: "yes"})

	test("accepts only when every policy accepts", async () => {
		const decision = await composePolicies(accept, accept)(REQUEST, ctxWith(graphqlReturning(() => ({}))))
		expect(decision.accept).toBe(true)
	})

	test("short-circuits on the first denial", async () => {
		let secondRan = false
		const trackingDeny: Policy = async () => {
			return {accept: false, reason: "first"}
		}
		const tracking: Policy = async () => {
			secondRan = true
			return {accept: true, reason: "second"}
		}
		const decision = await composePolicies(trackingDeny, tracking)(REQUEST, ctxWith(graphqlReturning(() => ({}))))
		expect(decision).toEqual({accept: false, reason: "first"})
		expect(secondRan).toBe(false)
	})
})
