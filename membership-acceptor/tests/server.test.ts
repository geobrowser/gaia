import {describe, expect, test} from "bun:test"
import {createHmac} from "node:crypto"

import type {AppConfig} from "../src/config.js"
import {createApp} from "../src/server.js"
import type {Acceptor, VoteResult} from "../src/vote.js"

const SECRET = "test-secret-0123456789abcdef"
const ALLOWED_SPACE = "d4f5a6b7-0000-0000-0000-000000000000"

const baseConfig: AppConfig = {
	port: 8080,
	webhookSecret: SECRET,
	acceptorPrivateKey: `0x${"1".repeat(64)}`,
	acceptorSpaceId: `0x${"a".repeat(32)}`,
	spaceRegistryAddress: `0x${"b".repeat(40)}`,
	rpcUrl: "http://rpc.test",
	pimlicoApiKey: "pim_test",
	chainId: 80451,
	graphqlEndpoint: "https://api.test/graphql",
	autoacceptSpaceIds: new Set([ALLOWED_SPACE]),
}

/** A stub acceptor that records vote calls and returns configurable decisions/results. */
function stubAcceptor(opts: {allows?: (s: string) => boolean; accept?: boolean; result?: VoteResult} = {}) {
	const calls: string[] = []
	const evaluated: string[] = []
	const acceptor: Acceptor = {
		allowsSpace: opts.allows ?? ((s) => baseConfig.autoacceptSpaceIds.has(s.toLowerCase())),
		evaluate: async (req) => {
			evaluated.push(req.proposalId)
			return opts.accept === false
				? {accept: false, reason: "denied by test policy"}
				: {accept: true, reason: "ok"}
		},
		vote: async (req) => {
			calls.push(req.proposalId)
			return opts.result ?? {kind: "voted", txHash: "0xabc"}
		},
	}
	return {acceptor, calls, evaluated}
}

function sign(body: string, secret = SECRET): string {
	return "sha256=" + createHmac("sha256", secret).update(Buffer.from(body)).digest("hex")
}

function webhookRequest(body: string, signature: string | null): Request {
	const headers: Record<string, string> = {"content-type": "application/json"}
	if (signature !== null) headers["x-geo-signature"] = signature
	return new Request("http://localhost:8080/webhooks/geo", {method: "POST", headers, body})
}

const NON_MEMBERSHIP = JSON.stringify({
	version: 1,
	event_type: "proposal_created",
	category: "governance",
	space_id: ALLOWED_SPACE,
	proposal_id: "c3e4f5a6-0000-0000-0000-000000000000",
	idempotency_key: "12345:0:proposal_created:b2c3d4e5",
})

function membershipBody(proposalId = "c3e4f5a6-0000-0000-0000-000000000000", spaceId = ALLOWED_SPACE): string {
	return JSON.stringify({
		event_type: "proposal_created",
		category: "governance",
		space_id: spaceId,
		proposal_id: proposalId,
		voting_mode: "fast",
		actions: [{type: "add_member", target_address: "a1b2c3d4-0000-0000-0000-000000000000"}],
	})
}

describe("GET /health", () => {
	test("returns 200 ok", async () => {
		const {acceptor} = stubAcceptor()
		const res = await createApp(baseConfig, acceptor)(new Request("http://localhost:8080/health"))
		expect(res.status).toBe(200)
		expect(await res.json()).toEqual({status: "ok"})
	})
})

describe("POST /webhooks/geo — signature & parsing", () => {
	test("rejects an invalid signature with 401", async () => {
		const {acceptor} = stubAcceptor()
		const res = await createApp(baseConfig, acceptor)(webhookRequest(NON_MEMBERSHIP, sign(NON_MEMBERSHIP, "wrong")))
		expect(res.status).toBe(401)
	})

	test("rejects a missing signature with 401", async () => {
		const {acceptor} = stubAcceptor()
		const res = await createApp(baseConfig, acceptor)(webhookRequest(NON_MEMBERSHIP, null))
		expect(res.status).toBe(401)
	})

	test("rejects a signed-but-non-JSON body with 400", async () => {
		const {acceptor} = stubAcceptor()
		const res = await createApp(baseConfig, acceptor)(webhookRequest("not json", sign("not json")))
		expect(res.status).toBe(400)
	})

	test("acks a non-membership event with 200 and does not vote", async () => {
		const {acceptor, calls} = stubAcceptor()
		const res = await createApp(baseConfig, acceptor)(webhookRequest(NON_MEMBERSHIP, sign(NON_MEMBERSHIP)))
		expect(res.status).toBe(200)
		expect(calls).toHaveLength(0)
	})
})

describe("POST /webhooks/geo — membership voting", () => {
	test("votes once on a valid membership request and acks 200", async () => {
		const {acceptor, calls} = stubAcceptor()
		const body = membershipBody()
		const res = await createApp(baseConfig, acceptor)(webhookRequest(body, sign(body)))
		expect(res.status).toBe(200)
		expect(calls).toEqual(["c3e4f5a6-0000-0000-0000-000000000000"])
	})

	test("ignores a request for a space not in the allowlist (no vote)", async () => {
		const {acceptor, calls} = stubAcceptor()
		const body = membershipBody("c3e4f5a6-0000-0000-0000-000000000000", "ffffffff-0000-0000-0000-000000000000")
		const res = await createApp(baseConfig, acceptor)(webhookRequest(body, sign(body)))
		expect(res.status).toBe(200)
		expect(calls).toHaveLength(0)
	})

	test("dedupes duplicate deliveries — votes once across N copies", async () => {
		const {acceptor, calls} = stubAcceptor()
		const app = createApp(baseConfig, acceptor)
		const body = membershipBody()
		const sig = sign(body)
		await app(webhookRequest(body, sig))
		await app(webhookRequest(body, sig))
		await app(webhookRequest(body, sig))
		expect(calls).toHaveLength(1)
	})

	test("policy denial acks 200 and does not vote", async () => {
		const {acceptor, calls, evaluated} = stubAcceptor({accept: false})
		const body = membershipBody()
		const res = await createApp(baseConfig, acceptor)(webhookRequest(body, sign(body)))
		expect(res.status).toBe(200)
		expect(evaluated).toHaveLength(1) // policy was consulted
		expect(calls).toHaveLength(0) // but no vote
	})

	test("benign on-chain rejection acks 200 (no retry)", async () => {
		const {acceptor} = stubAcceptor({result: {kind: "benign", message: "already voted"}})
		const body = membershipBody()
		const res = await createApp(baseConfig, acceptor)(webhookRequest(body, sign(body)))
		expect(res.status).toBe(200)
	})

	test("infra failure returns 5xx and a retry re-votes (dedupe rolled back)", async () => {
		const {acceptor, calls} = stubAcceptor({result: {kind: "infra", message: "rpc down"}})
		const app = createApp(baseConfig, acceptor)
		const body = membershipBody()
		const sig = sign(body)
		const first = await app(webhookRequest(body, sig))
		const second = await app(webhookRequest(body, sig))
		expect(first.status).toBe(503)
		expect(second.status).toBe(503)
		// Both attempts actually reached the vote (the failed mark was rolled back).
		expect(calls).toHaveLength(2)
	})
})

describe("unknown routes", () => {
	test("GET / returns 404", async () => {
		const {acceptor} = stubAcceptor()
		const res = await createApp(baseConfig, acceptor)(new Request("http://localhost:8080/"))
		expect(res.status).toBe(404)
	})

	test("wrong method on /webhooks/geo returns 404", async () => {
		const {acceptor} = stubAcceptor()
		const res = await createApp(
			baseConfig,
			acceptor,
		)(new Request("http://localhost:8080/webhooks/geo", {method: "GET"}))
		expect(res.status).toBe(404)
	})
})
