# Notification Service (RFC)

## Summary

A standalone notification indexer that consumes blockchain events from Kafka, determines who should be notified, and delivers notifications via webhooks to registered app servers. Each app server (Curator iOS, Geo web, etc.) owns the last mile — deciding how and where to notify users on their platform.

## Goals

- Notify users of relevant activity in the knowledge graph (proposals, edits, membership changes)
- Support multiple front-end apps via registered webhooks
- Guarantee at-least-once delivery with idempotency
- Decouple notification logic from existing indexers (kg-indexer, search-indexer)

## Non-Goals

- Managing push tokens, APNs, FCM, or device-level delivery (app servers own this)
- Real-time websocket notifications (future work)
- In-app notification UI (each front-end owns this)

---

## Architecture

```mermaid
flowchart TB
    subgraph Blockchain
        BC[Blockchain / Substreams]
    end

    subgraph Event Pipeline
        HP[hermes-pipeline]
    end

    subgraph Kafka
        SG[space.governance]
        SM[space.membership]
        KE[knowledge.edits]
        SMod[space.moderation]
    end

    subgraph Notification Service
        NI[Notification Indexer]
        DB[(Postgres)]
        DW[Delivery Worker]
    end

    subgraph App Servers
        CUR[Curator API]
        GEO[Geo Web API]
        APP3[Other App API]
    end

    BC --> HP
    HP --> SG & SM & KE & SMod
    SG & SM & KE & SMod --> NI
    NI -->|lookup subscriptions| DB
    NI -->|write outbox| DB
    DW -->|read pending| DB
    DW -->|POST webhook| CUR & GEO & APP3
```

---

## Event Flow

```mermaid
sequenceDiagram
    participant K as Kafka
    participant NI as Notification Indexer
    participant DB as Postgres
    participant DW as Delivery Worker
    participant App as App Server

    K->>NI: Consume event (e.g. ProposalCreated)
    NI->>DB: Query subscriptions for space
    DB-->>NI: List of subscribed users + app_ids
    NI->>DB: Write notification_outbox rows (one per user×app)
    NI->>K: Commit offset

    loop Delivery loop
        DW->>DB: SELECT pending notifications (batch)
        DW->>App: POST /notifications (with idempotency_key header)
        App-->>DW: 200 OK
        DW->>DB: UPDATE status = 'delivered'
    end

    Note over DW,App: If webhook returns non-2xx,<br/>row stays pending and retries<br/>with exponential backoff
```

---

## Notification Types

### Governance

| Event | Kafka Topic | Who Gets Notified | Subscription Type |
|---|---|---|---|
| Proposal created | `space.governance` | Editors of the space | Implicit (membership) |
| Proposal voted on | `space.governance` | Proposal creator | Implicit (authorship) |
| Proposal executed | `space.governance` | Proposal creator + space editors | Implicit |
| Proposal rejected/failed | `space.governance` | Proposal creator + space editors | Implicit |
| Proposal expiring soon | `space.governance` | Editors who haven't voted | Implicit (membership) |
| Voting settings changed | `space.governance` | Space editors | Implicit (membership) |

### Membership

| Event | Kafka Topic | Who Gets Notified | Subscription Type |
|---|---|---|---|
| You were added as editor/member | `space.membership` | The added user | Implicit (direct) |
| You were removed as editor/member | `space.membership` | The removed user | Implicit (direct) |
| Editor/member added to your space | `space.membership` | Existing editors of the space | Implicit (membership) |
| Editor/member removed from your space | `space.membership` | Existing editors of the space | Implicit (membership) |
| User left space | `space.membership` | Existing editors of the space | Implicit (membership) |

### Moderation

| Event | Kafka Topic | Who Gets Notified | Subscription Type |
|---|---|---|---|
| Content flagged | `space.moderation` | Space editors | Implicit (membership) |
| Content unflagged | `space.moderation` | The editor whose content was flagged | Implicit (direct) |
| Editor flagged | `space.moderation` | The flagged editor | Implicit (direct) |
| Editor unflagged | `space.moderation` | The unflagged editor | Implicit (direct) |

---

## Subscription Model

All subscriptions are **implicit** — derived from existing state. Editors and members of a space are automatically subscribed to governance, membership, and moderation events in that space. Resolved at notification time by querying the existing `editors` and `members` tables. No new subscription tables are needed.

```mermaid
erDiagram
    notification_outbox {
        uuid id PK
        text idempotency_key UK "block:sequence:event_type:user_space_id"
        text app_id FK
        text user_space_id
        text event_type
        jsonb payload
        text status "pending | delivered | failed"
        int attempts
        timestamp created_at
        timestamp delivered_at
    }

    app_webhooks {
        text app_id PK
        text webhook_url
        text secret "HMAC signing key"
        timestamp registered_at
    }

    app_webhooks ||--o{ notification_outbox : "delivers to"
```

---

## Idempotency

Every blockchain event includes `BlockchainMetadata`:
- `block_number` (u64)
- `sequence` (u32 — order within block)
- `transaction_hash` (bytes)

The idempotency key is deterministically derived:

```
idempotency_key = hash(block_number + ":" + sequence + ":" + event_type + ":" + user_space_id)
```

This key is:
- Passed to the outbox as a `UNIQUE` constraint (prevents duplicate writes)
- Sent to the app server as an `X-Idempotency-Key` header (app server deduplicates on their end)

---

## Webhook Contract

### Registration

App servers register via API or config. All notifications are delivered to all registered webhooks — there is no per-app filtering of event types. Each app server is responsible for ignoring notification types it doesn't care about.

```json
{
  "app_id": "curator-ios",
  "webhook_url": "https://curator-api.example.com/geo/notifications",
  "secret": "whsec_..."
}
```

### Delivery

The delivery worker POSTs to the registered webhook:

```http
POST /geo/notifications
Content-Type: application/json
X-Idempotency-Key: abc123...
X-Signature: sha256=<HMAC of payload using secret>

{
  "event_type": "proposal_created",
  "user_space_id": "31cfe99fdf3549ef89094548f04858ff",
  "space_id": "a542cac04434987163d31071f3223af5",
  "data": {
    "proposal_id": "...",
    "proposal_name": "Add new editor",
    "created_by": "...",
    "space_name": "Crypto"
  },
  "timestamp": "2026-03-11T15:30:00Z",
  "block_number": 12345678
}
```

### Expected Response

- `200` — Notification accepted. Delivery worker marks as `delivered`.
- `409` — Duplicate (already processed). Delivery worker marks as `delivered`.
- `4xx/5xx` — Retry with exponential backoff (max 5 attempts, then `failed`).

---

## Failure Handling

```mermaid
stateDiagram-v2
    [*] --> pending: Outbox row created
    pending --> delivered: Webhook returns 2xx/409
    pending --> pending: Webhook fails, attempts < max
    pending --> failed: Webhook fails, attempts >= max
    failed --> pending: Manual retry / operator intervention
```

- **Kafka unreachable:** Indexer retries connection (standard consumer behavior)
- **Postgres unreachable:** Indexer pauses consumption until DB recovers (no offset commit)
- **Webhook unreachable:** Outbox rows stay `pending`, delivery worker retries with exponential backoff
- **Duplicate events:** `idempotency_key` UNIQUE constraint prevents duplicate outbox rows

---

## Deployment

Follows the same pattern as search-indexer and scoring-service:

```
k8s namespace: notifications (or notifications-staging)
├── notification-indexer   (Deployment — Kafka consumer, writes outbox)
└── delivery-worker        (Deployment — reads outbox, calls webhooks)
```

Both share the same Postgres database. The indexer and delivery worker are separate deployments so they scale independently — the indexer is bound by Kafka throughput, the worker by webhook latency.

---

## Open Questions

1. **User ↔ device mapping:** How does the app server know which device/user to notify? The notification payload includes `user_space_id` — the app server needs its own mapping of `user_space_id → user account → device token`. The Curator app already has user-to-email mappings that could serve as a starting point.

2. **Notification preferences:** Do users control which event types they receive? Per-space mute? This can start simple (all-or-nothing) and add granularity later.

3. **Rate limiting:** Should we batch notifications to avoid spamming? e.g., "5 proposals created in Crypto space" instead of 5 separate notifications.

---

## Example: Curator App with AWS SNS

This section illustrates how an app server (the Curator iOS app) would integrate with the notification service using AWS SNS for push delivery.

### Overview

```mermaid
flowchart LR
    DW[Delivery Worker] -->|POST webhook| CUR[Curator API]
    CUR -->|lookup device tokens| DB[(Curator DB)]
    CUR -->|publish| SNS[AWS SNS]
    SNS -->|push| APNs[APNs]
    APNs -->|push| iOS[iOS Device]
```

### 1. Register Webhook

The Curator API registers its webhook with the notification service:

```json
{
  "app_id": "curator-ios",
  "webhook_url": "https://curator-api.example.com/geo/notifications",
  "secret": "whsec_abc123..."
}
```

### 2. Map Users to Devices

The Curator app maintains its own mapping of Geo users to device tokens. When a user logs into the Curator app, the app registers their APNs device token with AWS SNS using `@aws-sdk/client-sns` and stores the endpoint ARN alongside their `user_space_id`:

```typescript
import { SNSClient, CreatePlatformEndpointCommand } from "@aws-sdk/client-sns"

const sns = new SNSClient({ region: "us-east-1" })

async function registerDevice(userSpaceId: string, apnsDeviceToken: string) {
  const { EndpointArn } = await sns.send(
    new CreatePlatformEndpointCommand({
      PlatformApplicationArn: "arn:aws:sns:us-east-1:123456789:app/APNS/curator",
      Token: apnsDeviceToken,
    })
  )

  // Store mapping in Curator's own database
  await db.query(
    `INSERT INTO curator_devices (user_space_id, device_token, sns_endpoint_arn)
     VALUES ($1, $2, $3)
     ON CONFLICT (user_space_id) DO UPDATE SET device_token = $2, sns_endpoint_arn = $3`,
    [userSpaceId, apnsDeviceToken, EndpointArn]
  )
}
```

### 3. Receive and Route Notifications

When the Curator API receives a webhook POST from the delivery worker:

```typescript
import { SNSClient, PublishCommand } from "@aws-sdk/client-sns"
import crypto from "node:crypto"

const sns = new SNSClient({ region: "us-east-1" })

app.post("/geo/notifications", async (req, res) => {
  // 1. Verify HMAC signature
  const expected = crypto
    .createHmac("sha256", WEBHOOK_SECRET)
    .update(JSON.stringify(req.body))
    .digest("hex")

  if (req.headers["x-signature"] !== `sha256=${expected}`) {
    return res.status(401).send("Invalid signature")
  }

  // 2. Deduplicate
  const idempotencyKey = req.headers["x-idempotency-key"]
  if (await db.hasProcessed(idempotencyKey)) {
    return res.status(409).send("Already processed")
  }

  // 3. Look up user's SNS endpoint
  const { event_type, user_space_id, data } = req.body
  const device = await db.query(
    "SELECT sns_endpoint_arn FROM curator_devices WHERE user_space_id = $1",
    [user_space_id]
  )

  if (!device) return res.status(200).send("No device registered")

  // 4. Publish push notification via AWS SNS
  await sns.send(
    new PublishCommand({
      TargetArn: device.sns_endpoint_arn,
      MessageStructure: "json",
      Message: JSON.stringify({
        APNS: JSON.stringify({
          aps: {
            alert: {
              title: `New ${event_type} in ${data.space_name}`,
              body: data.proposal_name,
            },
            sound: "default",
          },
        }),
      }),
    })
  )

  await db.markProcessed(idempotencyKey)
  return res.status(200).send("OK")
})
```

### 4. What the Curator App Owns

The notification service is only responsible for delivering the webhook. Everything else is the Curator app's responsibility:

- User ↔ device token mapping and SNS endpoint registration
- AWS SNS platform application setup
- Push notification formatting and localization
- Notification preferences and muting
- Badge counts and notification history

---

## Future Work

Potential notification types that could be added later:

### Trust & Topology
- **Space verified/related** — Space editors notified when another space extends trust to theirs
- **Subtopic added** — Space editors notified when their space is declared a subtopic of another
- **New subspace created** — Parent space editors notified when a child space is created under theirs

### Explicit Subscriptions (User Opt-In)

Users subscribe to specific entities or spaces to get notified when proposals or edits affect them. Requires a new `notification_subscriptions` table and a subscription management API. Alternatively, this could be implemented entirely in the app layer — each app server manages its own subscriptions and filters the notifications it receives via webhook.

```mermaid
erDiagram
    notification_subscriptions {
        uuid id PK
        text user_space_id
        text target_type "space | entity"
        text target_id
        text[] event_types "proposal_created, entity_edited, etc."
        timestamp created_at
    }

    notification_outbox {
        uuid id PK
        text idempotency_key UK
        text app_id FK
        text user_space_id
        text event_type
        jsonb payload
        text status
        int attempts
        timestamp created_at
        timestamp delivered_at
    }

    app_webhooks {
        text app_id PK
        text webhook_url
        text secret
        timestamp registered_at
    }

    notification_subscriptions ||--o{ notification_outbox : "triggers"
    app_webhooks ||--o{ notification_outbox : "delivers to"
```

Notification types enabled by explicit subscriptions:

| Event | Kafka Topic | Who Gets Notified |
|---|---|---|
| Proposal on watched entity | `space.governance` | Entity subscribers |
| Entity edited | `knowledge.edits` | Entity subscribers |

### Curation
- **Vote cast on your entity** — Entity creator notified of upvotes/downvotes (from `curation.votes` topic)
- **Score milestone** — Notify when an entity crosses a score threshold (e.g., top 10% in its space)

### Webhook Filtering
- **Event type filter on webhook registration** — Add an `event_types` column to `app_webhooks` so apps only receive notification types they care about

### Engagement
- **Notification batching/digests** — Aggregate multiple notifications into a single summary (e.g., daily digest of activity in your spaces)
- **Real-time websocket channel** — Push notifications to connected clients without polling
- **Notification preferences UI** — Per-space and per-event-type mute/unmute controls
