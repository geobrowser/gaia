import {describe, expect, test} from "bun:test"
import {createHmac} from "node:crypto"

import {verifySignature} from "../src/signature.js"

const SECRET = "test-secret-0123456789abcdef"

function sign(body: string, secret = SECRET): string {
	return "sha256=" + createHmac("sha256", secret).update(Buffer.from(body)).digest("hex")
}

describe("verifySignature", () => {
	test("accepts a correctly signed body", () => {
		const body = JSON.stringify({event_type: "proposal_created"})
		expect(verifySignature(Buffer.from(body), SECRET, sign(body))).toBe(true)
	})

	test("rejects a tampered body", () => {
		const body = JSON.stringify({event_type: "proposal_created"})
		const signature = sign(body)
		const tampered = JSON.stringify({event_type: "proposal_executed"})
		expect(verifySignature(Buffer.from(tampered), SECRET, signature)).toBe(false)
	})

	test("rejects a signature made with the wrong secret", () => {
		const body = "payload"
		expect(verifySignature(Buffer.from(body), SECRET, sign(body, "wrong-secret"))).toBe(false)
	})

	test("rejects a missing signature header", () => {
		expect(verifySignature(Buffer.from("payload"), SECRET, null)).toBe(false)
	})

	test("rejects a header without the sha256= prefix", () => {
		const raw = createHmac("sha256", SECRET).update(Buffer.from("payload")).digest("hex")
		expect(verifySignature(Buffer.from("payload"), SECRET, raw)).toBe(false)
	})

	test("rejects a malformed/short signature without throwing", () => {
		expect(verifySignature(Buffer.from("payload"), SECRET, "sha256=deadbeef")).toBe(false)
	})

	test("verifies over raw bytes, not re-serialized JSON (key order preserved)", () => {
		// Two JSON encodings of the same object with different key order produce
		// different signatures — the worker signs exact bytes.
		const sent = '{"b":2,"a":1}'
		const signature = sign(sent)
		expect(verifySignature(Buffer.from(sent), SECRET, signature)).toBe(true)
		expect(verifySignature(Buffer.from('{"a":1,"b":2}'), SECRET, signature)).toBe(false)
	})
})
