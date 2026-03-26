# Delivery Worker

Polls the `notification_deliveries` table for pending rows and POSTs them to registered webhook endpoints with HMAC-SHA256 signatures.

## How it works

1. Queries `notification_deliveries` for rows where `status = 'pending' AND next_retry_at <= now()`
2. For each delivery, POSTs the notification payload to the webhook URL
3. Signs each request with `HMAC-SHA256(webhook_secret, body)` in the `X-Geo-Signature` header
4. On success (2xx or 409): marks delivery as `delivered`
5. On retryable failure (5xx, 429): increments attempt count and schedules retry with exponential backoff
6. On permanent failure (4xx other than 429, or max retries exceeded): marks delivery as `failed`

Uses `FOR UPDATE SKIP LOCKED` for safe horizontal scaling — multiple workers can run concurrently without processing the same delivery.

## Environment Variables

### Required

| Variable | Description |
|---|---|
| `DATABASE_URL` | PostgreSQL connection string |

### Service

| Variable | Default | Description |
|---|---|---|
| `POLL_INTERVAL_MS` | `5000` | How often to check for pending deliveries (milliseconds) |
| `MAX_RETRIES` | `100` | Maximum delivery attempts before marking as permanently failed |
| `BATCH_SIZE` | `50` | Maximum deliveries to fetch per poll cycle |
| `HEARTBEAT_INTERVAL_SECS` | `60` | How often to log heartbeat stats (seconds) |

### Telemetry (all optional)

| Variable | Description |
|---|---|
| `RUST_LOG` | Log level filter (e.g. `info,delivery_worker=debug`) |
| `SENTRY_DSN` | Sentry DSN — enables Sentry when set, falls back to console logging |
| `SENTRY_TRACES_SAMPLE_RATE` | Trace sampling rate (default `1.0`) |
| `SENTRY_SEND_DEFAULT_PII` | Send PII to Sentry (`true`/`false`) |
| `SENTRY_ENVIRONMENT` | Sentry environment tag |
| `SENTRY_RELEASE` | Sentry release tag |
| `SENTRY_DEBUG` | Enable Sentry debug mode (`true`/`false`) |

## Retry Behavior

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

After `MAX_RETRIES` failed attempts, the delivery is marked as `failed` and logged at `error` level.

## Staging vs Production Isolation

Staging and production run in **separate Kubernetes namespaces** (`notifications-staging` vs `notifications`), each with its own `scoring-service-credentials` secret pointing to a **completely separate PostgreSQL database**. The delivery worker only reads webhook URLs from the `app_webhooks` table in its own database — there is no shared state between environments.

This means staging can never call production webhooks (or vice versa), as long as each database is seeded with the correct environment's webhook URLs. When registering a new webhook, make sure you're connected to the right database for the target environment.

## Database Tables

**Reads from:**
- `notification_deliveries` — pending deliveries to process
- `notification_outbox` — payload and idempotency key for each delivery
- `app_webhooks` — webhook URL and HMAC secret for each registered app

**Writes to:**
- `notification_deliveries` — updates status, attempts, next_retry_at, delivered_at

## Running locally

```bash
DATABASE_URL=postgresql://postgres:postgres@localhost:5432/gaia \
POLL_INTERVAL_MS=5000 \
RUST_LOG=info,delivery_worker=debug \
cargo run -p delivery-worker
```
