# Notification Indexer

Consumes blockchain events from Kafka, resolves who should be notified, and writes one row per recipient to the `notification_outbox` table. The separate **delivery-worker** service signs (HMAC-SHA256) and delivers outbox rows to registered webhooks.

## How it works

Four sources feed the outbox:

1. **Governance consumer** — subscribes to `space.governance` and handles `PROPOSAL_CREATED`, `PROPOSAL_UPDATED`, `PROPOSAL_VOTED`, `PROPOSAL_EXECUTED`, `PROPOSAL_SETTINGS_UPDATED`.
2. **Knowledge-edits consumer** — subscribes to `knowledge.edits`, decodes the GRC-20 payload, and detects bounty and comment activity from `CreateRelation` ops.
3. **Rejection poller** — runs every `REJECTION_POLL_INTERVAL_SECS`; finds proposals where `end_time < now()` and `executed_at IS NULL` that haven't been notified, and emits `proposal_rejected`.
4. **Vote poller** — runs every `VOTE_POLL_INTERVAL_SECS`; polls `votes_count` for entities whose upvotes reached `VOTE_NOTIFICATION_THRESHOLD` and emits `entity_votes_threshold` to each entity's creator. Uses a persisted keyset cursor on `(updated_at, id)` (in `notification_poll_cursors`) so each poll only scans rows whose counts changed since the last poll. Disabled when the threshold is `<= 0`.

For each detected event the indexer resolves a recipient set (see the table below), then writes one outbox row **per recipient** with that recipient's `user_space_id` stamped into the payload. The indexer delivers the relevant *superset*; precise per-user filtering (mute/snooze/preferences) is done **app-side**.

### Idempotency

Each outbox row's unique key is `{block}:{sequence}:{event_type}:{instance_id}` hashed together with the recipient's `user_space_id` (SHA-256), under `ON CONFLICT (idempotency_key) DO NOTHING`. The `{instance_id}` is the per-event subject — `bounty_entity_id`, `comment_entity_id`, or the bounty `relation_id` — so a single edit emitting several events of the same type does not collide. Consequence: **one comment/entity touched repeatedly within one edit yields one notification per recipient**; each distinct bounty relation (e.g. two users expressing interest) is its own notification.

## Notifications

Every notification type the indexer currently creates. "Recipients" are resolved to `user_space_id`s (each recipient's personal-space UUID).

| `event_type` | Category | Source / trigger | Who is notified | Notes & edge cases |
|---|---|---|---|---|
| `proposal_created` | governance | `space.governance` | Editors of the proposal's space | — |
| `proposal_updated` | governance | `space.governance` | Editors **+ prior voters** of the proposal (`proposal_votes.voter_id`) | "A new version of a proposal you voted on was submitted." Voter lookup is best-effort: a miss still notifies editors. |
| `proposal_voted` | governance | `space.governance` | Editors **+ the proposer** (`proposals.proposed_by`) | "Your proposal was voted on." Proposer is usually also an editor (deduped). |
| `proposal_executed` | governance | `space.governance` | Editors **+ the proposer** | "Your proposal was approved/executed." |
| `proposal_settings_updated` | governance | `space.governance` | Editors of the proposal's space | — |
| `proposal_rejected` | governance | **Rejection poller** (not a Kafka event) | Editors **+ the proposer** | Emitted when a proposal expires (`end_time < now()`) unexecuted. Idempotency key is `{proposal_id}:proposal_rejected` so it fires at most once per proposal. |
| `bounty_interest` | bounty | `knowledge.edits` (Interest relation) | Editors of the **bounty's** space | Bounty space resolved via the bounty's `Types → Bounty` relation; if it can't be resolved, the event is skipped. No editors → no notifications. |
| `bounty_allocated` | bounty | `knowledge.edits` (Allocated relation) | **The curator** only (single recipient) | Not a fan-out. Curator's space resolved from the relation's `to_space`, or looked up if absent; unresolvable → skipped. |
| `bounty_payout` | bounty | `knowledge.edits` (Payout relation) | **The curator** only (single recipient) | Same resolution/skip rules as `bounty_allocated`. |
| `bounty_created` | bounty | `knowledge.edits` (`Types → Bounty`) | Editors of the space the bounty was **created in** (the edit's space) | Multiple bounties in one edit each notify independently. No editors → no notifications. |
| `proposal_comment` | comment | `knowledge.edits` (`Comment` entity with `Reply to` → a proposal) | **The proposal's proposer**, gated on the commenter being a **member or editor** of the proposal's space | If the parent isn't a proposal it's routed to `comment` (below). **If the commenter is not a member/editor, no notification is created** (filtered out). |
| `comment` | comment | `knowledge.edits` (`Comment` with `Reply to` → a non-proposal parent) | **All thread participants ∪ the thread root's creator**, excluding the comment's own author | Participants = the `space_id` of every `Reply to` relation in the thread (each comment author's personal space). Root creator is exact for proposals (`proposed_by`) and a best-effort "home space" otherwise (entities have no `created_by`). Thread root resolved by walking `Reply to` upward (depth-bounded). |
| `entity_votes_threshold` | votes | **Vote poller** (not a Kafka event) | **The entity's creator** (best-effort home space) | Fires when an entity's `upvotes` in a space reaches `VOTE_NOTIFICATION_THRESHOLD`. Idempotency key is `{entity_id}:{vote_space_id}:entity_votes_threshold:{threshold}`, so it fires at most once per entity per space per threshold value — raising the threshold (a new env value) re-arms it at the new level. Skipped if the creator can't be resolved. |

Recipient resolution is intentionally a **superset** and **best-effort**: a failed lookup of a *targeted extra* (proposer/voters) only drops that extra — editors still receive the base event. The knowledge-edits consumer also waits (bounded) for the kg-indexer to catch up to the event's block before resolving, so spaces/relations/names are populated.

## Environment Variables

### Required

| Variable | Description |
|---|---|
| `DATABASE_URL` | PostgreSQL connection string |
| `ENVIRONMENT` | `production` or `staging` — controls Kafka topic prefix (via `hermes-kafka`) |

### Kafka

| Variable | Default | Description |
|---|---|---|
| `KAFKA_BROKER` | `localhost:9092` | Kafka broker address |
| `KAFKA_GROUP_ID` | `notification-indexer` | Consumer group ID |
| `KAFKA_USERNAME` | *(none)* | SASL username — enables SASL/SSL when set |
| `KAFKA_PASSWORD` | *(none)* | SASL password — required when `KAFKA_USERNAME` is set |
| `KAFKA_SSL_CA_PEM` | *(none)* | Custom CA certificate in PEM format |

### Service

| Variable | Default | Description |
|---|---|---|
| `REJECTION_POLL_INTERVAL_SECS` | `60` | How often to check for expired proposals (seconds) |
| `VOTE_NOTIFICATION_THRESHOLD` | `10` | Upvote count at which an entity's creator is notified (`entity_votes_threshold`). Set to `0` to disable the vote poller. |
| `VOTE_POLL_INTERVAL_SECS` | `60` | How often the vote poller scans `votes_count` (seconds) |
| `HEARTBEAT_INTERVAL_SECS` | `60` | How often to log heartbeat stats (seconds) |

### Telemetry (all optional)

| Variable | Description |
|---|---|
| `RUST_LOG` | Log level filter (e.g. `info,notification_indexer=debug`) |
| `SENTRY_DSN` | Sentry DSN — enables Sentry when set, falls back to console logging |
| `SENTRY_TRACES_SAMPLE_RATE` | Trace sampling rate (default `1.0`) |
| `SENTRY_SEND_DEFAULT_PII` | Send PII to Sentry (`true`/`false`) |
| `SENTRY_ENVIRONMENT` | Sentry environment tag |
| `SENTRY_RELEASE` | Sentry release tag |
| `SENTRY_DEBUG` | Enable Sentry debug mode (`true`/`false`) |

## Database Tables

**Reads from:**
- `editors` — per-space fan-out (also the membership check, with `members`)
- `members` — `proposal_comment` member/editor gate
- `proposals` — proposer/space resolution, name/tally enrichment, and the rejection poller's expired-proposal query
- `proposal_votes` — prior voters for `proposal_updated`
- `relations` — bounty/comment detection support: bounty space, entity home space, and thread-root/participant resolution
- `values` — entity/proposal name enrichment
- `spaces` — personal-space filtering during entity→space resolution
- `votes_count` — the vote poller's threshold scan (keyset on `updated_at, id`, `object_type = 0`)

**Writes to:**
- `notification_outbox` — one row per recipient per event
- `notification_deliveries` — one row per outbox entry per registered webhook
- `notification_poll_cursors` — persisted high-water cursor for the vote poller

## Running locally

```bash
DATABASE_URL=postgresql://postgres:postgres@localhost:5432/gaia \
KAFKA_BROKER=localhost:9092 \
ENVIRONMENT=production \
RUST_LOG=info,notification_indexer=debug \
cargo run -p notification-indexer
```
