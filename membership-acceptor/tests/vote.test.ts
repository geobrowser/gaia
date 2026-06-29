import {describe, expect, test} from "bun:test"

import {toBytes16Hex} from "../src/contracts.js"
import type {MembershipRequest} from "../src/detect.js"
import type {GraphQLClient} from "../src/graphql.js"
import type {Policy} from "../src/policy.js"
import {createAcceptor} from "../src/vote.js"
import type {SmartWallet} from "../src/wallet.js"

const noopGraphql: GraphQLClient = {query: async () => ({}) as never}
const acceptAll: Policy = async () => ({accept: true, reason: "ok"})

const REQUEST: MembershipRequest = {
	proposalId: "c3e4f5a6-0000-0000-0000-000000000000",
	spaceId: "d4f5a6b7-0000-0000-0000-000000000000",
	requesterSpaceId: "a1b2c3d4-0000-0000-0000-000000000000",
}

/** Build a SmartWallet whose sendTransaction is stubbed by `send`. */
function fakeWallet(send: () => Promise<string>): SmartWallet {
	return {
		// biome-ignore lint/suspicious/noExplicitAny: minimal stub of the smart account client
		smartAccountClient: {account: {address: "0xacct"}, sendTransaction: send} as any,
		// biome-ignore lint/suspicious/noExplicitAny: chain shape is irrelevant to the test
		chain: {id: 80451} as any,
		safeAddress: "0xsafe",
	}
}

function acceptorWith(send: () => Promise<string>, allowlist = [REQUEST.spaceId], policy: Policy = acceptAll) {
	return createAcceptor({
		wallet: fakeWallet(send),
		acceptorSpaceId: `0x${"a".repeat(32)}`,
		spaceRegistryAddress: `0x${"b".repeat(40)}`,
		allowlist: new Set(allowlist.map(toBytes16Hex)),
		policy,
		graphql: noopGraphql,
	})
}

describe("allowsSpace", () => {
	test("matches allowlisted spaces case-insensitively", () => {
		const a = acceptorWith(async () => "0x", [REQUEST.spaceId.toUpperCase()])
		expect(a.allowsSpace(REQUEST.spaceId)).toBe(true)
		expect(a.allowsSpace("ffffffff-0000-0000-0000-000000000000")).toBe(false)
	})

	test("empty allowlist allows nothing", () => {
		const a = acceptorWith(async () => "0x", [])
		expect(a.allowsSpace(REQUEST.spaceId)).toBe(false)
	})
})

describe("evaluate", () => {
	test("delegates to the configured policy with the acceptor's space id", async () => {
		let seenMember = ""
		const policy: Policy = async (_req, ctx) => {
			seenMember = ctx.acceptorSpaceId
			return {accept: false, reason: "nope"}
		}
		const a = acceptorWith(async () => "0x", [REQUEST.spaceId], policy)
		const decision = await a.evaluate(REQUEST)
		expect(decision).toEqual({accept: false, reason: "nope"})
		expect(seenMember).toBe(`0x${"a".repeat(32)}`)
	})
})

describe("vote", () => {
	test("returns voted with the tx hash on success", async () => {
		const a = acceptorWith(async () => "0xdeadbeef")
		const result = await a.vote(REQUEST)
		expect(result).toEqual({kind: "voted", txHash: "0xdeadbeef"})
	})

	test("classifies an on-chain revert as benign", async () => {
		const a = acceptorWith(async () => {
			const e = new Error("execution reverted: CanNotVote")
			e.name = "ContractFunctionRevertedError"
			throw e
		})
		const result = await a.vote(REQUEST)
		expect(result.kind).toBe("benign")
	})

	test("classifies an RPC/bundler failure as infra", async () => {
		const a = acceptorWith(async () => {
			throw new Error("fetch failed: ECONNREFUSED")
		})
		const result = await a.vote(REQUEST)
		expect(result.kind).toBe("infra")
	})

	test("redacts an apikey leaked in an error message", async () => {
		const a = acceptorWith(async () => {
			throw new Error("https://api.pimlico.io/v2/80451/rpc?apikey=pim_secret down")
		})
		const result = await a.vote(REQUEST)
		expect(result.kind).toBe("infra")
		if (result.kind === "infra") {
			expect(result.message).toContain("apikey=<redacted>")
			expect(result.message).not.toContain("pim_secret")
		}
	})
})
