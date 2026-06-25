/**
 * Webhook signature verification.
 *
 * The notification delivery-worker signs every request with
 * `X-Geo-Signature: sha256=<hex>`, where `<hex>` is the HMAC-SHA256 of the raw
 * request body keyed by the shared secret. See notification-service/WEBHOOK_INTEGRATION.md.
 */

import {createHmac, timingSafeEqual} from "node:crypto"

const PREFIX = "sha256="

/**
 * Verify the `X-Geo-Signature` header against the raw request body.
 *
 * @param body - the raw request body bytes (NOT re-serialized JSON — the HMAC is
 *   computed over the exact bytes the worker sent)
 * @param secret - the shared secret for this webhook
 * @param signatureHeader - the value of the `X-Geo-Signature` header (may be null)
 * @returns true iff the header is present, well-formed, and matches
 */
export function verifySignature(body: Uint8Array, secret: string, signatureHeader: string | null): boolean {
	if (!signatureHeader?.startsWith(PREFIX)) {
		return false
	}

	const received = signatureHeader.slice(PREFIX.length)
	const expected = createHmac("sha256", secret).update(body).digest("hex")

	// Length check first: timingSafeEqual throws if the buffers differ in length.
	if (received.length !== expected.length) {
		return false
	}
	return timingSafeEqual(Buffer.from(received), Buffer.from(expected))
}
