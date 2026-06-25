import {describe, expect, test} from "bun:test"
import {createHmac} from "node:crypto"

import type {AcceptorConfig} from "../src/config.js"
import {createApp} from "../src/server.js"

const config: AcceptorConfig = {port: 8080, webhookSecret: "test-secret-0123456789abcdef"}
const app = createApp(config)

function sign(body: string, secret = config.webhookSecret): string {
	return "sha256=" + createHmac("sha256", secret).update(Buffer.from(body)).digest("hex")
}

function webhookRequest(body: string, signature: string | null): Request {
	const headers: Record<string, string> = {"content-type": "application/json"}
	if (signature !== null) headers["x-geo-signature"] = signature
	return new Request("http://localhost:8080/webhooks/geo", {method: "POST", headers, body})
}

const SAMPLE = JSON.stringify({
	version: 1,
	event_type: "proposal_created",
	category: "governance",
	space_id: "d4f5a6b7-0000-0000-0000-000000000000",
	proposal_id: "c3e4f5a6-0000-0000-0000-000000000000",
	idempotency_key: "12345:0:proposal_created:b2c3d4e5",
})

describe("GET /health", () => {
	test("returns 200 ok", async () => {
		const res = await app(new Request("http://localhost:8080/health"))
		expect(res.status).toBe(200)
		expect(await res.json()).toEqual({status: "ok"})
	})
})

describe("POST /webhooks/geo", () => {
	test("accepts a correctly signed payload", async () => {
		const res = await app(webhookRequest(SAMPLE, sign(SAMPLE)))
		expect(res.status).toBe(200)
		expect(await res.json()).toEqual({status: "ok"})
	})

	test("rejects an invalid signature with 401", async () => {
		const res = await app(webhookRequest(SAMPLE, sign(SAMPLE, "wrong-secret")))
		expect(res.status).toBe(401)
	})

	test("rejects a missing signature with 401", async () => {
		const res = await app(webhookRequest(SAMPLE, null))
		expect(res.status).toBe(401)
	})

	test("rejects a signed-but-non-JSON body with 400", async () => {
		const body = "not json"
		const res = await app(webhookRequest(body, sign(body)))
		expect(res.status).toBe(400)
	})

	test("rejects a tampered body with 401 (signature no longer matches)", async () => {
		const signature = sign(SAMPLE)
		const tampered = SAMPLE.replace("proposal_created", "proposal_executed")
		const res = await app(webhookRequest(tampered, signature))
		expect(res.status).toBe(401)
	})

	test("accepts and acks a membership-request payload (M2 detection path)", async () => {
		const membership = JSON.stringify({
			version: 1,
			event_type: "proposal_created",
			category: "governance",
			space_id: "d4f5a6b7-0000-0000-0000-000000000000",
			proposal_id: "c3e4f5a6-0000-0000-0000-000000000000",
			voting_mode: "fast",
			actions: [{type: "add_member", target_address: "0x1234"}],
		})
		const res = await app(webhookRequest(membership, sign(membership)))
		expect(res.status).toBe(200)
		expect(await res.json()).toEqual({status: "ok"})
	})

	test("acks a duplicate membership delivery with 200 (deduped, no error)", async () => {
		const membership = JSON.stringify({
			event_type: "proposal_created",
			space_id: "d4f5a6b7-0000-0000-0000-000000000000",
			proposal_id: "dupe-proposal-id",
			voting_mode: "fast",
			actions: [{type: "add_member", target_address: "0x1234"}],
		})
		const sig = sign(membership)
		const first = await app(webhookRequest(membership, sig))
		const second = await app(webhookRequest(membership, sig))
		expect(first.status).toBe(200)
		expect(second.status).toBe(200)
	})
})

describe("unknown routes", () => {
	test("GET / returns 404", async () => {
		const res = await app(new Request("http://localhost:8080/"))
		expect(res.status).toBe(404)
	})

	test("wrong method on /webhooks/geo returns 404", async () => {
		const res = await app(new Request("http://localhost:8080/webhooks/geo", {method: "GET"}))
		expect(res.status).toBe(404)
	})
})
