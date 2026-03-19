# Webhook Integration Guide

This guide explains how app servers (Curator, Geo, etc.) can receive governance notifications from the Geo notification service.

## How it works

```
Governance event (on-chain)
  → notification-indexer (resolves editors for the space)
  → notification_outbox (one row per editor)
  → delivery-worker (POSTs to every registered webhook)
  → Your app server
```

Each webhook call represents a notification for **one editor** about **one governance event**. Your app decides how to handle it — push notification, in-app badge, email, etc.

## 1. Register your webhook

Webhooks are registered directly in the `app_webhooks` table. Each row needs:

| Column | Description |
|---|---|
| `app_name` | Unique name for your app (e.g. `curator-ios`) |
| `url` | The HTTPS endpoint that will receive POST requests |
| `secret` | Shared secret for HMAC signature verification |

```sql
INSERT INTO app_webhooks (app_name, url, secret)
VALUES ('curator-ios', 'https://api.curator.app/webhooks/geo', 'your-secret-here');
```

Generate a strong secret (e.g. `openssl rand -hex 32`). Store it securely — you'll need it to verify signatures.

## 2. Implement the webhook endpoint

Your endpoint receives a JSON POST for each notification. It must return a 2xx status code to acknowledge receipt.

### Request format

**Headers:**

| Header | Example | Description |
|---|---|---|
| `Content-Type` | `application/json` | Always JSON |
| `X-Geo-Signature` | `sha256=a1b2c3...` | HMAC-SHA256 hex digest of the request body |

**Body:**

```json
{
  "version": 1,
  "event_type": "proposal_created",
  "space_id": "d4f5a6b7-...",
  "proposal_id": "c3e4f5a6-...",
  "user_space_id": "b2c3d4e5-...",
  "idempotency_key": "proposal_created:c3e4f5a6-...:12345:b2c3d4e5-...",
  "proposer_id": "a1b2c3d4-...",
  "block_number": 12345,
  "timestamp": 1700000000
}
```

### Event types

| Event | Description | Extra fields |
|---|---|---|
| `proposal_created` | A new proposal was created | `proposer_id` |
| `proposal_updated` | A proposal was updated | `proposer_id` |
| `proposal_voted` | A vote was cast on a proposal | `voter_id`, `vote` (`yes`/`no`/`abstain`) |
| `proposal_executed` | A passed proposal was executed | — |
| `proposal_settings_updated` | Proposal settings changed (e.g. fast→slow escalation) | — |
| `proposal_rejected` | A proposal expired without execution | `proposer_id` |

### Fields present on every event

| Field | Type | Description |
|---|---|---|
| `version` | number | Payload schema version (currently `1`). Check this field to handle future schema changes. |
| `event_type` | string | One of the event types above |
| `space_id` | UUID string | The space where the event occurred |
| `proposal_id` | UUID string | The proposal involved |
| `user_space_id` | UUID string | The editor this notification is addressed to |
| `idempotency_key` | string | Unique key for deduplication (see below) |
| `block_number` | number | Block number (absent for `proposal_rejected`) |
| `timestamp` | number | Unix timestamp in seconds |

## 3. Verify the signature

Always verify the `X-Geo-Signature` header before processing. This confirms the request came from the Geo notification service and wasn't tampered with.

The signature is computed as `sha256=` followed by the hex-encoded HMAC-SHA256 of the raw request body using your shared secret.

```typescript
import { createHmac, timingSafeEqual } from "node:crypto"

function verifySignature(body: Buffer, secret: string, signatureHeader: string): boolean {
  const prefix = "sha256="
  if (!signatureHeader.startsWith(prefix)) {
    return false
  }

  const received = signatureHeader.slice(prefix.length)
  const expected = createHmac("sha256", secret).update(body).digest("hex")

  // Use timing-safe comparison to prevent timing attacks
  if (received.length !== expected.length) {
    return false
  }
  return timingSafeEqual(Buffer.from(received), Buffer.from(expected))
}
```

## 4. Handle idempotency

The delivery worker retries failed deliveries with exponential backoff. Your endpoint may receive the same notification more than once. Use the `idempotency_key` field in the body to deduplicate:

```typescript
const processed = new Set<string>() // or use Redis/DB for persistence

app.post("/webhooks/geo", async (req, res) => {
  const event = JSON.parse(req.body.toString())

  if (processed.has(event.idempotency_key)) {
    // Already handled — return 409 to signal duplicate (treated as success)
    return res.status(409).send("duplicate")
  }

  // ... process notification ...

  processed.add(event.idempotency_key)
  res.status(200).send("ok")
})
```

Returning **409** tells the delivery worker the notification was already processed — it won't retry.

## 5. Full example (Express)

```typescript
import express from "express"
import { createHmac, timingSafeEqual } from "node:crypto"

const app = express()
const WEBHOOK_SECRET = process.env.GEO_WEBHOOK_SECRET!
const processed = new Set<string>()

// Parse raw body for signature verification
app.post("/webhooks/geo", express.raw({ type: "application/json" }), (req, res) => {
  const signature = req.headers["x-geo-signature"] as string
  const body = req.body as Buffer

  // 1. Verify signature
  if (!signature || !verifySignature(body, WEBHOOK_SECRET, signature)) {
    return res.status(401).send("invalid signature")
  }

  // 2. Parse payload
  const event = JSON.parse(body.toString())

  // 3. Check idempotency
  if (processed.has(event.idempotency_key)) {
    return res.status(409).send("duplicate")
  }

  // 4. Handle the event
  switch (event.event_type) {
    case "proposal_created":
      console.log(
        `New proposal ${event.proposal_id} in space ${event.space_id} ` +
        `for editor ${event.user_space_id}`
      )
      // Look up the user's push token by user_space_id, send push notification, etc.
      break

    case "proposal_voted":
      console.log(
        `Vote '${event.vote}' on proposal ${event.proposal_id} ` +
        `by ${event.voter_id} — notifying editor ${event.user_space_id}`
      )
      break

    case "proposal_executed":
      console.log(`Proposal ${event.proposal_id} executed`)
      break

    case "proposal_rejected":
      console.log(`Proposal ${event.proposal_id} expired (rejected)`)
      break

    // proposal_updated, proposal_settings_updated, etc.
    default:
      console.log(`Received ${event.event_type}`)
  }

  processed.add(event.idempotency_key)
  res.status(200).send("ok")
})

function verifySignature(body: Buffer, secret: string, header: string): boolean {
  const prefix = "sha256="
  if (!header.startsWith(prefix)) return false

  const received = header.slice(prefix.length)
  const expected = createHmac("sha256", secret).update(body).digest("hex")

  if (received.length !== expected.length) return false
  return timingSafeEqual(Buffer.from(received), Buffer.from(expected))
}

app.listen(3000, () => console.log("Webhook server listening on :3000"))
```

## Response codes

| Status | Meaning |
|---|---|
| **2xx** | Success — notification acknowledged |
| **409** | Duplicate — already processed (treated as success) |
| **429** | Rate limited — will retry with backoff |
| **5xx** | Server error — will retry with backoff |
| **4xx** (other) | Client error — will **not** retry, marked as failed |

## Retry behavior

Failed deliveries are retried with exponential backoff:

| Attempt | Delay |
|---|---|
| 1 | 30 seconds |
| 2 | 1 minute |
| 3 | 2 minutes |
| 4 | 4 minutes |
| 5 | 8 minutes |
| 6 | 16 minutes |
| 7 | 32 minutes |
| 8–13 | 1 hour → 34 hours |
| 14+ | 48 hours (capped) |

After **100 failed attempts**, the delivery is marked as permanently failed.
