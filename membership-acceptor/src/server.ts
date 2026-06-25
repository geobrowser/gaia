/**
 * HTTP app — routes and request handling.
 *
 * `createApp` returns a `fetch`-style handler `(Request) => Promise<Response>` so
 * it can be driven directly in unit tests (no bound port) and handed to
 * `Bun.serve` in production.
 *
 */

import type {AppConfig} from "./config.js"
import {detectMembershipRequest, SeenProposals} from "./detect.js"
import {verifySignature} from "./signature.js"
import {log} from "./telemetry.js"
import type {Acceptor} from "./vote.js"

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

async function handleWebhook(
	req: Request,
	config: AppConfig,
	acceptor: Acceptor,
	seen: SeenProposals,
): Promise<Response> {
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

	// Every authenticated delivery is logged at debug (the firehose is large);
	// membership requests are surfaced at info below.
	log.debug("webhook received", summarize(payload))

	const request = detectMembershipRequest(payload)
	if (!request) {
		return json({status: "ok"}, 200)
	}

	// Policy gate: only act on spaces this acceptor serves.
	if (!acceptor.allowsSpace(request.spaceId)) {
		log.debug("membership request for an unserved space — ignored", {
			proposal_id: request.proposalId,
			space_id: request.spaceId,
		})
		return json({status: "ok"}, 200)
	}

	// Dedupe on proposal_id, marking BEFORE we evaluate/vote so the concurrent
	// fan-out copies (one per editor) collapse to a single attempt — and so we
	// only hit the policy's API once per proposal.
	if (seen.seen(request.proposalId)) {
		log.debug("membership request already seen — deduped", {
			proposal_id: request.proposalId,
			space_id: request.spaceId,
		})
		return json({status: "ok"}, 200)
	}

	// Policy seam: API-backed rules (editor check, and any space-defined policies).
	const decision = await acceptor.evaluate(request)
	if (!decision.accept) {
		log.info("membership request denied by policy", {
			proposal_id: request.proposalId,
			space_id: request.spaceId,
			reason: decision.reason,
		})
		return json({status: "ok"}, 200)
	}

	const result = await acceptor.vote(request)
	switch (result.kind) {
		case "voted":
			log.info("membership request accepted — vote cast", {
				proposal_id: request.proposalId,
				space_id: request.spaceId,
				requester_space_id: request.requesterSpaceId,
				tx_hash: result.txHash,
			})
			return json({status: "ok"}, 200)

		case "benign":
			// The chain rejected the vote. Nothing to retry — ack so delivery stops.
			log.warn("membership vote rejected on-chain — not retrying", {
				proposal_id: request.proposalId,
				space_id: request.spaceId,
				reason: result.message,
			})
			return json({status: "ok"}, 200)

		default: {
			// Infrastructure failure — roll back the dedupe mark so the
			// delivery-worker's retry isn't silently swallowed, and 5xx to retry.
			seen.unmark(request.proposalId)
			log.error("membership vote failed (infrastructure) — will retry", {
				proposal_id: request.proposalId,
				space_id: request.spaceId,
				error: result.message,
			})
			return json({error: "vote failed"}, 503)
		}
	}
}

/**
 * Build the request handler for the given config.
 *
 * Routes:
 *  - `GET /health`        — liveness/readiness probe
 *  - `POST /webhooks/geo` — notification webhook sink (HMAC-verified)
 */
export function createApp(config: AppConfig, acceptor: Acceptor): (req: Request) => Promise<Response> {
	// Dedupe state lives for the lifetime of the process (see SeenProposals).
	const seen = new SeenProposals()

	return async (req: Request): Promise<Response> => {
		const {pathname} = new URL(req.url)

		if (req.method === "GET" && pathname === "/health") {
			return json({status: "ok"}, 200)
		}

		if (req.method === "POST" && pathname === "/webhooks/geo") {
			try {
				return await handleWebhook(req, config, acceptor, seen)
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
