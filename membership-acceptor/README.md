# membership-acceptor

Real-time membership auto-accept for Geo spaces.

This is the event-driven successor to the `proposal-executor` cron's
membership-accept path. Instead of polling every 5 minutes, it receives
notification **webhooks** from the Geo notification service and reacts in
near real time.

```
on-chain PROPOSAL_CREATED
  → hermes-pipeline → Kafka space.governance
  → notification-indexer → delivery-worker → HMAC-signed webhook
  → membership-acceptor  ← this service
```

It is structured so it can later be extracted into its own repo that space
admins deploy themselves, each running a sovereign acceptor with its own wallet
and membership policy (allow-all, allow-verified, reputation, payment, …).

## Status — Milestone 1 (webhook connectivity)

This milestone proves the pipe end to end. The service:

- exposes `POST /webhooks/geo` and verifies the `X-Geo-Signature` HMAC-SHA256
  header against `GEO_WEBHOOK_SECRET` (rejects unsigned/tampered requests with 401);
- exposes `GET /health` for k8s probes;
- logs a structured summary of every delivery it receives.

It does **not** yet detect membership requests (M2) or cast votes (M3). Every
authenticated webhook is acknowledged with `200`.

> Implementation note: unlike `proposal-executor` (an Effect-TS CronJob), this is
> a plain `Bun.serve` HTTP server. The request path is straightforward async
> code; we keep the same Sentry/structured-logging conventions (see
> `src/telemetry.ts`) so logs read consistently across the two services.

## Layout

| Path | Purpose |
|---|---|
| `src/index.ts` | Entry point — parse config, start server, graceful shutdown |
| `src/server.ts` | Routes + request handling (`createApp` returns a testable fetch handler) |
| `src/signature.ts` | `X-Geo-Signature` HMAC verification |
| `src/config.ts` | Env parsing + validation (fail-fast) |
| `src/telemetry.ts` | Sentry init + structured logger + flush |
| `deployment/production/` | k8s `Deployment` + `Service` + secret template |

## Develop

```bash
bun install
bun test          # unit tests
bun run typecheck # tsc --noEmit
bun run lint      # biome
GEO_WEBHOOK_SECRET=dev-secret bun run start   # serves on :8080
```

Smoke test locally:

```bash
SECRET=dev-secret
BODY='{"event_type":"proposal_created","category":"governance"}'
SIG="sha256=$(printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$SECRET" | awk '{print $2}')"
curl -s -XPOST localhost:8080/webhooks/geo \
  -H "content-type: application/json" -H "x-geo-signature: $SIG" -d "$BODY"
# → {"status":"ok"}
```

## Registering the webhook

The service receives deliveries only after a row is added to the notification
service's `app_webhooks` table pointing at it, with a `secret` equal to this
service's `GEO_WEBHOOK_SECRET`. See
`notification-service/WEBHOOK_INTEGRATION.md`. In-cluster URL:

```
http://membership-acceptor.<namespace>.svc.cluster.local/webhooks/geo
```

## Configuration

| Variable | Required | Default | Description |
|---|---|---|---|
| `GEO_WEBHOOK_SECRET` | yes | — | Shared secret; must equal `app_webhooks.secret` |
| `PORT` | no | `8080` | HTTP listen port |
| `SENTRY_DSN` | no | — | Enables Sentry; console-only when unset |
| `SENTRY_ENVIRONMENT` / `SENTRY_RELEASE` / `SENTRY_TRACES_SAMPLE_RATE` / `SENTRY_DEBUG` | no | — | Sentry tuning |
