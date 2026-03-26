# Notification Indexer

Consumes governance events from the `space.governance` Kafka topic and writes per-editor notifications to the `notification_outbox` table. Also runs a periodic rejection poller that detects proposals that expired without execution.

## How it works

1. **Kafka consumer** subscribes to `space.governance` and processes all governance event types (`PROPOSAL_CREATED`, `PROPOSAL_UPDATED`, `PROPOSAL_VOTED`, `PROPOSAL_EXECUTED`, `PROPOSAL_SETTINGS_UPDATED`)
2. For each event, looks up all editors in the proposal's space via the `editors` table
3. Creates one outbox row per editor with `user_space_id` in the payload
4. The delivery-worker (separate service) picks up outbox rows and delivers them to webhooks

**Rejection poller**: Runs every `REJECTION_POLL_INTERVAL_SECS` seconds. Finds proposals where `end_time < now()` and `executed_at IS NULL` that haven't been notified yet, and writes `proposal_rejected` notifications for each editor.

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
- `editors` — looks up editors per space for fan-out
- `proposals` — rejection poller queries for expired proposals

**Writes to:**
- `notification_outbox` — one row per editor per event
- `notification_deliveries` — one row per outbox entry per registered webhook

## Running locally

```bash
DATABASE_URL=postgresql://postgres:postgres@localhost:5432/gaia \
KAFKA_BROKER=localhost:9092 \
ENVIRONMENT=production \
RUST_LOG=info,notification_indexer=debug \
cargo run -p notification-indexer
```
