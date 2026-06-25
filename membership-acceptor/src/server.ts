/**
 * HTTP app — routes and request handling.
 *
 * `createApp` returns a `fetch`-style handler `(Request) => Promise<Response>` so
 * it can be driven directly in unit tests (no bound port) and handed to
 * `Bun.serve` in production.
 *
 * Milestone 1 scope: prove connectivity. We verify the HMAC signature and log a
 * structured summary of every delivery. Detection (M2) and voting (M3) are not
 * here yet — every authenticated webhook is acknowledged with 200.
 */

import type {AcceptorConfig} from "./config.js"
import {verifySignature} from "./signature.js"
import {log} from "./telemetry.js"

const SIGNATURE_HEADER = "x-geo-signature"
const SIGNATURE_PREFIX = "sha256="

/** Best-effort summary of a notification payload, for logging only. */
function summarize(payload: unknown): Record<string, unknown> {
	if (!payload || typeof payload !== "object") {
		return {payload_type: typeof payload}
	}
	const p = payload as Record<string, unknown>
	return {
		event_type: p.event_type,
		category: p.category,
		space_id: p.space_id,
		proposal_id: p.proposal_id,
		idempotency_key: p.idempotency_key,
	}
}

function json(body: unknown, status: number): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: {"content-type": "application/json"},
	})
}

async function handleWebhook(req: Request, config: AcceptorConfig): Promise<Response> {
	const signature = req.headers.get(SIGNATURE_HEADER)

	// A missing or malformed signature header can never pass the HMAC check, so
	// reject it up front. The full HMAC verification still runs below.
	if (signature === null || !signature.startsWith(SIGNATURE_PREFIX)) {
		log.warn("webhook signature verification failed", {
			has_signature: signature !== null,
		})
		return json({error: "invalid signature"}, 401)
	}

	// Read the raw bytes — the HMAC is over exactly what was sent, so we must not
	// round-trip through JSON before verifying.
	const body = new Uint8Array(await req.arrayBuffer())

	if (!verifySignature(body, config.webhookSecret, signature)) {
		log.warn("webhook signature verification failed", {
			has_signature: signature !== null,
			body_bytes: body.length,
		})
		return json({error: "invalid signature"}, 401)
	}

	let payload: unknown
	try {
		payload = JSON.parse(Buffer.from(body).toString("utf8"))
	} catch {
		log.warn("webhook body is not valid JSON", {body_bytes: body.length})
		return json({error: "invalid JSON body"}, 400)
	}

	// M1: just observe. M2 will filter to membership requests; M3 will vote.
	log.info("webhook received", summarize(payload))
	return json({status: "ok"}, 200)
}

/**
 * Build the request handler for the given config.
 *
 * Routes:
 *  - `GET /health`        — liveness/readiness probe
 *  - `POST /webhooks/geo` — notification webhook sink (HMAC-verified)
 */
export function createApp(config: AcceptorConfig): (req: Request) => Promise<Response> {
	return async (req: Request): Promise<Response> => {
		const {pathname} = new URL(req.url)

		if (req.method === "GET" && pathname === "/health") {
			return json({status: "ok"}, 200)
		}

		if (req.method === "POST" && pathname === "/webhooks/geo") {
			try {
				return await handleWebhook(req, config)
			} catch (err) {
				// Unexpected failure — 500 makes the delivery-worker retry rather than
				// silently dropping the event.
				log.error("unhandled error processing webhook", {error: err})
				return json({error: "internal error"}, 500)
			}
		}

		return json({error: "not found"}, 404)
	}
}
