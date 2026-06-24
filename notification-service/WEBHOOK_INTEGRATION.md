# Webhook Integration Guide

This guide explains how app servers (Curator, Geo, etc.) can receive governance and bounty notifications from the Geo notification service.

## How it works

```
Governance/bounty event (on-chain)
  → notification-indexer (resolves editors for the space, enriches with names)
  → notification_outbox (one row per editor)
  → delivery-worker (POSTs to every registered webhook)
  → Your app server
```

Each webhook call represents a notification for **one editor** about **one event**. Your app decides how to handle it — push notification, in-app badge, email, etc.

## 1. Register your webhook

Webhooks are registered directly in the `app_webhooks` table. Each row needs:

| Column | Description |
|---|---|
| `app_name` | Unique name for your app (e.g. `curator-ios`) |
| `url` | The HTTPS endpoint that will receive POST requests |
| `secret` | Shared secret for HMAC signature verification |
| `notification_types` | Optional `text[]` — only deliver these notification types. `NULL`/empty = **all types**. |
| `space_ids` | Optional `uuid[]` — only deliver events in these spaces. `NULL`/empty = **all spaces**. |

```sql
INSERT INTO app_webhooks (app_name, url, secret)
VALUES ('curator-ios', 'https://api.curator.app/webhooks/geo', 'your-secret-here');
```

Generate a strong secret (e.g. `openssl rand -hex 32`). Store it securely — you'll need it to verify signatures.

### Filtering which events you receive

By default (both filter columns `NULL`), a webhook receives **every** event for
**every** space. Set either column to narrow delivery. The two dimensions are
**ANDed** — an event is delivered only if it matches *both* the type filter
*and* the space filter:

- **`notification_types`** — an event is matched if any of its notification
  tokens (see taxonomy below) is in the array. Empty/`NULL` matches all types.
- **`space_ids`** — an event is matched if its `space_id` is in the array.
  Empty/`NULL` matches all spaces.

```sql
-- Only membership changes (add_member / add_editor), and only in two spaces:
INSERT INTO app_webhooks (app_name, url, secret, notification_types, space_ids)
VALUES (
  'curator-ios',
  'https://api.curator.app/webhooks/geo',
  'your-secret-here',
  ARRAY['add_member', 'add_editor'],
  ARRAY['d4f5a6b7-...', 'e5f6a7b8-...']::uuid[]
);

-- All proposal events, any space:
UPDATE app_webhooks SET notification_types = ARRAY['proposal_created'] WHERE app_name = 'curator-ios';
```

#### Notification type taxonomy

Most events map to a single token equal to their `event_type`.
**`proposal_created` is layered**: it always emits the base token
`proposal_created`, *plus* a per-action token for membership actions. This lets
you subscribe broadly or narrowly:

- Subscribe to **`proposal_created`** → you receive **all** proposals, including
  those that add members/editors.
- Subscribe to **`add_member`** (or `add_editor`) → you receive **only**
  proposals containing that action, not other proposals.

| Token | Emitted when |
|---|---|
| `proposal_created` | Any proposal is created (always). |
| `add_member` | A `proposal_created` contains an `add_member` action (in addition to `proposal_created`). |
| `add_editor` | A `proposal_created` contains an `add_editor` action (in addition to `proposal_created`). |
| `proposal_updated` | A proposal is updated. |
| `proposal_voted` | A vote is cast. |
| `proposal_executed` | A passed proposal is executed. |
| `proposal_settings_updated` | Voting settings change. |
| `proposal_rejected` | A proposal expires unexecuted. |
| `bounty_interest` / `bounty_allocated` / `bounty_payout` / `bounty_created` | Corresponding bounty event. |
| `proposal_comment` / `comment` | Comment events. |
| `entity_votes_threshold` | Entity vote threshold reached. |

> **Unknown types are logged, not rejected.** If `notification_types` contains a
> token outside this set it simply never matches (and the indexer logs an error
> at startup naming the app and token). Adding a type to the DB before the
> indexer knows about it won't cause errors — it just won't match until the code
> recognizes it.

> **Propagation delay:** filters are cached in memory by the indexer and
> refreshed every **30 seconds**, so inserts/updates to `app_webhooks` take
> effect within ~30s — no restart needed.

## 2. Implement the webhook endpoint

Your endpoint receives a JSON POST for each notification. It must return a 2xx status code to acknowledge receipt.

### Request format

**Headers:**

| Header | Example | Description |
|---|---|---|
| `Content-Type` | `application/json` | Always JSON |
| `X-Geo-Signature` | `sha256=a1b2c3...` | HMAC-SHA256 hex digest of the request body |

### Payload structure

All events share common fields, with event-specific data flattened into the same object.

**Common fields (present on every event):**

| Field | Type | Description |
|---|---|---|
| `version` | number | Payload schema version (currently `1`). Check this field to handle future schema changes. |
| `event_type` | string | One of the event types below |
| `category` | string | `"governance"` or `"bounty"` — use for routing |
| `space_id` | UUID string | The space where the event occurred |
| `space_name` | string or null | Human-readable space name (best-effort) |
| `user_space_id` | UUID string | The editor this notification is addressed to |
| `idempotency_key` | string | Unique key for deduplication (see below) |
| `block_number` | number or null | Block number (absent for `proposal_rejected`) |
| `timestamp` | number or null | Unix timestamp in seconds |

## 3. Event types

### Governance events

#### `proposal_created`

A new proposal was created in the space.

```json
{
  "version": 1,
  "event_type": "proposal_created",
  "category": "governance",
  "space_id": "d4f5a6b7-...",
  "space_name": "Geo Genesis",
  "user_space_id": "b2c3d4e5-...",
  "idempotency_key": "12345:0:proposal_created:b2c3d4e5-...",
  "block_number": 12345,
  "timestamp": 1700000000,
  "proposal_id": "c3e4f5a6-...",
  "proposal_name": "Add new editor to space",
  "proposer_id": "a1b2c3d4-...",
  "proposer_name": "Alice",
  "voting_mode": "slow",
  "actions": [
    {
      "type": "add_member",
      "target_address": "0x1234..."
    }
  ],
  "settings": {
    "start_date": 1700000000,
    "end_date": 1700086400,
    "voting_mode": "slow",
    "quorum": 51,
    "flat_threshold": 0,
    "percentage_threshold": 51
  }
}
```

**Governance-specific fields:**

| Field | Type | Present on | Description |
|---|---|---|---|
| `proposal_id` | UUID string | all governance | The proposal involved |
| `proposal_name` | string or null | all governance | Human-readable proposal name (best-effort) |
| `proposer_id` | UUID string or null | `created`, `updated`, `rejected` | Who created the proposal |
| `proposer_name` | string or null | `created`, `updated`, `rejected` | Human-readable proposer name (best-effort) |
| `voter_id` | UUID string or null | `voted` | Who cast the vote |
| `voter_name` | string or null | `voted` | Human-readable voter name (best-effort) |
| `vote` | string or null | `voted` | `"yes"`, `"no"`, or `"abstain"` |
| `voting_mode` | string or null | `created`, `updated` | `"slow"` or `"fast"` |
| `actions` | array or null | `created`, `updated` | List of proposal actions (see below) |
| `settings` | object or null | `created`, `settings_updated` | Proposal voting settings |
| `yes_count` | number or null | `voted` | Current yes vote tally |
| `no_count` | number or null | `voted` | Current no vote tally |
| `abstain_count` | number or null | `voted` | Current abstain vote tally |

#### `proposal_updated`

A proposal was updated with new actions or metadata.

Same fields as `proposal_created`.

#### `proposal_voted`

A vote was cast on a proposal.

```json
{
  "version": 1,
  "event_type": "proposal_voted",
  "category": "governance",
  "space_id": "d4f5a6b7-...",
  "space_name": "Geo Genesis",
  "user_space_id": "b2c3d4e5-...",
  "idempotency_key": "12345:1:proposal_voted:b2c3d4e5-...",
  "block_number": 12345,
  "timestamp": 1700000000,
  "proposal_id": "c3e4f5a6-...",
  "proposal_name": "Add new editor to space",
  "voter_id": "e5f6a7b8-...",
  "voter_name": "Bob",
  "vote": "yes",
  "yes_count": 5,
  "no_count": 1,
  "abstain_count": 0
}
```

#### `proposal_executed`

A passed proposal was executed on-chain.

```json
{
  "version": 1,
  "event_type": "proposal_executed",
  "category": "governance",
  "space_id": "d4f5a6b7-...",
  "space_name": "Geo Genesis",
  "user_space_id": "b2c3d4e5-...",
  "idempotency_key": "12345:2:proposal_executed:b2c3d4e5-...",
  "block_number": 12345,
  "timestamp": 1700000000,
  "proposal_id": "c3e4f5a6-...",
  "proposal_name": "Add new editor to space"
}
```

#### `proposal_settings_updated`

Proposal voting settings were changed (e.g. quorum, thresholds, duration).

```json
{
  "version": 1,
  "event_type": "proposal_settings_updated",
  "category": "governance",
  "space_id": "d4f5a6b7-...",
  "space_name": "Geo Genesis",
  "user_space_id": "b2c3d4e5-...",
  "idempotency_key": "12345:3:proposal_settings_updated:b2c3d4e5-...",
  "block_number": 12345,
  "timestamp": 1700000000,
  "proposal_id": "c3e4f5a6-...",
  "voting_mode": "slow",
  "settings": {
    "start_date": 1700000000,
    "end_date": 1700086400,
    "voting_mode": "slow",
    "quorum": 51,
    "flat_threshold": 0,
    "percentage_threshold": 51
  }
}
```

#### `proposal_rejected`

A proposal expired without being executed (rejection poller detected it).

```json
{
  "version": 1,
  "event_type": "proposal_rejected",
  "category": "governance",
  "space_id": "d4f5a6b7-...",
  "space_name": "Geo Genesis",
  "user_space_id": "b2c3d4e5-...",
  "idempotency_key": "rejection:c3e4f5a6-...:b2c3d4e5-...",
  "timestamp": 1700000000,
  "proposal_id": "c3e4f5a6-...",
  "proposer_id": "a1b2c3d4-...",
  "proposer_name": "Alice"
}
```

Note: `block_number` is absent for rejection events since they are detected by polling, not from a block.

### Bounty events

#### `bounty_interest`

A user expressed interest in a bounty.

```json
{
  "version": 1,
  "event_type": "bounty_interest",
  "category": "bounty",
  "space_id": "d4f5a6b7-...",
  "space_name": "Geo Genesis",
  "user_space_id": "b2c3d4e5-...",
  "idempotency_key": "12345:4:bounty_interest:b2c3d4e5-...",
  "block_number": 12345,
  "timestamp": 1700000000,
  "bounty_entity_id": "f6a7b8c9-...",
  "bounty_name": "Improve search ranking",
  "relation_id": "a7b8c9d0-...",
  "curator_space_id": "c8d9e0f1-...",
  "curator_name": "Curator DAO",
  "bounty_space_id": "d9e0f1a2-...",
  "interested_user_space_id": "e0f1a2b3-..."
}
```

**Bounty-specific fields:**

| Field | Type | Present on | Description |
|---|---|---|---|
| `bounty_entity_id` | UUID string | all bounty | The bounty entity |
| `bounty_name` | string or null | all bounty | Human-readable bounty name (best-effort) |
| `relation_id` | UUID string | all bounty | The relation that triggered the event |
| `curator_space_id` | UUID string | all bounty | The curator's space |
| `curator_name` | string or null | all bounty | Human-readable curator name (best-effort) |
| `bounty_space_id` | UUID string | all bounty | The space the bounty belongs to |
| `proposal_id` | UUID string or null | `allocated`, `payout` | Associated proposal |
| `interested_user_space_id` | UUID string or null | `interest` | The user who expressed interest |

#### `bounty_allocated`

A bounty was allocated to a user via proposal.

```json
{
  "version": 1,
  "event_type": "bounty_allocated",
  "category": "bounty",
  "space_id": "d4f5a6b7-...",
  "space_name": "Geo Genesis",
  "user_space_id": "b2c3d4e5-...",
  "idempotency_key": "12345:5:bounty_allocated:b2c3d4e5-...",
  "block_number": 12345,
  "timestamp": 1700000000,
  "bounty_entity_id": "f6a7b8c9-...",
  "bounty_name": "Improve search ranking",
  "relation_id": "a7b8c9d0-...",
  "curator_space_id": "c8d9e0f1-...",
  "curator_name": "Curator DAO",
  "bounty_space_id": "d9e0f1a2-...",
  "proposal_id": "b8c9d0e1-..."
}
```

#### `bounty_payout`

A bounty payout was completed.

Same structure as `bounty_allocated`.

### Action types (in `actions` array)

**Membership actions:**

| Action type | Fields | Description |
|---|---|---|
| `add_member` | `target_address` | Add a member to the space |
| `remove_member` | `target_address` | Remove a member |
| `add_editor` | `target_address` | Add an editor |
| `remove_editor` | `target_address` | Remove an editor |
| `unflag_editor` | `target_address` | Unflag a flagged editor |

**Content actions:**

| Action type | Fields | Description |
|---|---|---|
| `publish` | `content_uri`, `name` | Publish an edit |
| `flag` | `target_address` (content ID) | Flag content |
| `unflag` | `target_address` (content ID) | Unflag content |

**Subspace actions:**

| Action type | Fields | Description |
|---|---|---|
| `subspace_verified` | `target_space_id` | Mark a subspace as verified |
| `subspace_unverified` | `target_space_id` | Mark a subspace as unverified |
| `subspace_related` | `target_space_id` | Mark a subspace as related |
| `subspace_unrelated` | `target_space_id` | Mark a subspace as unrelated |
| `subspace_topic_declared` | `target_topic_id` | Declare a topic on a subspace |
| `subspace_topic_removed` | `target_topic_id` | Remove a topic from a subspace |
| `set_topic` | `target_topic_id` | Set a topic |
| `unset_topic` | _(none)_ | Unset a topic |

**Settings actions:**

| Action type | Fields | Description |
|---|---|---|
| `update_voting_settings` | `voting_settings` | Update voting configuration |

### Settings object

| Field | Type | Description |
|---|---|---|
| `start_date` | number | Voting start (unix timestamp) |
| `end_date` | number | Voting end (unix timestamp) |
| `voting_mode` | string | `"slow"` or `"fast"` |
| `quorum` | number | Required quorum percentage |
| `flat_threshold` | number | Flat vote threshold |
| `percentage_threshold` | number | Percentage vote threshold |

### Voting settings update object (in actions)

| Field | Type | Description |
|---|---|---|
| `quorum` | number | New quorum value |
| `fast_threshold` | number | New fast threshold |
| `slow_threshold` | number | New slow threshold |
| `duration` | number | New voting duration in seconds |

## 4. Verify the signature

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

## 5. Handle idempotency

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

> **Important:** The idempotency set must be backed by persistent storage (e.g. a database table or Redis) so that already-processed keys survive server restarts. An in-memory `Set` will be lost on restart, causing duplicate notifications to be re-processed.

Returning **409** tells the delivery worker the notification was already processed — it won't retry.

## 6. Full example (Express)

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

  // 4. Route by category and event type
  switch (event.category) {
    case "governance":
      handleGovernanceEvent(event)
      break
    case "bounty":
      handleBountyEvent(event)
      break
    default:
      console.log(`Unknown category: ${event.category}`)
  }

  processed.add(event.idempotency_key)
  res.status(200).send("ok")
})

function handleGovernanceEvent(event: any) {
  switch (event.event_type) {
    case "proposal_created":
      console.log(
        `New proposal "${event.proposal_name}" by ${event.proposer_name} ` +
        `in ${event.space_name} for editor ${event.user_space_id}`
      )
      break
    case "proposal_voted":
      console.log(
        `${event.voter_name} voted '${event.vote}' on "${event.proposal_name}" ` +
        `(yes: ${event.yes_count}, no: ${event.no_count}, abstain: ${event.abstain_count})`
      )
      break
    case "proposal_executed":
      console.log(`Proposal "${event.proposal_name}" executed`)
      break
    case "proposal_rejected":
      console.log(`Proposal "${event.proposal_name}" expired (rejected)`)
      break
    case "proposal_settings_updated":
      console.log(`Voting settings updated for "${event.proposal_name}"`)
      break
    default:
      console.log(`Governance event: ${event.event_type}`)
  }
}

function handleBountyEvent(event: any) {
  switch (event.event_type) {
    case "bounty_interest":
      console.log(`Interest in bounty "${event.bounty_name}" from ${event.interested_user_space_id}`)
      break
    case "bounty_allocated":
      console.log(`Bounty "${event.bounty_name}" allocated via proposal ${event.proposal_id}`)
      break
    case "bounty_payout":
      console.log(`Bounty "${event.bounty_name}" payout completed`)
      break
    default:
      console.log(`Bounty event: ${event.event_type}`)
  }
}

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
| **Any other** | Error — will retry with exponential backoff |

## Retry behavior

All non-success responses are retried with exponential backoff:

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

## Enriched fields

The notification-indexer enriches payloads with human-readable names from the knowledge graph on a best-effort basis. These fields may be `null` if the data hasn't been indexed yet:

- `space_name` — resolved from the KG values table
- `proposal_name` — resolved from the proposals table
- `proposer_name`, `voter_name` — resolved from the KG values table
- `bounty_name`, `curator_name` — resolved from the KG values table
- `yes_count`, `no_count`, `abstain_count` — resolved from the proposals table (on `proposal_voted` only)

The indexer waits up to 30 seconds for the kg-indexer to catch up to the event's block before enriching, to maximize the chance of names being available.
