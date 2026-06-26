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

## Status — Milestone 3 (on-chain vote)

The service:

- exposes `POST /webhooks/geo` and verifies the `X-Geo-Signature` HMAC-SHA256
  header against `GEO_WEBHOOK_SECRET` (rejects unsigned/tampered requests with 401);
- exposes `GET /health` for k8s probes;
- **detects membership requests** in the firehose — a `proposal_created` event
  with `voting_mode == "fast"` and exactly one `add_member` action — gates them by
  the `MEMBERSHIP_AUTOACCEPT_SPACE_IDS` allowlist, and **de-duplicates** by
  `proposal_id` (the fan-out delivers one copy per editor of the space);
- runs a **policy** (API-backed rules — see below) before voting;
- **casts the YES vote** for each unique allowed+accepted request via the acceptor's Safe
  smart account (`SpaceRegistry.enter(PROPOSAL_VOTED, Yes)`, gas-sponsored by
  Pimlico). Fast-path threshold = 1, so the YES executes the AddMember in the same
  transaction — admitting the member.

### The "simple" model (no on-chain pre-reads)

The contract is the source of truth, so there is **no** read-before-vote. We mark
the proposal seen, attempt the vote, and classify the result:

| Outcome | HTTP | Behavior |
|---|---|---|
| `voted` | `200` | YES landed; member admitted. |
| `benign` (on-chain revert: already voted/executed/closed, or not an editor) | `200` | Nothing to retry — ack so the delivery-worker stops. |
| `infra` (RPC/bundler failure) | `503` | Roll back the dedupe mark and signal a retry. |

This eliminates the check-then-act race a read-before-vote design would have: a
duplicate that slips past in-memory dedupe (restart, 2nd replica) simply reverts
on-chain → no double-admit, just a wasted tx. **Keep `replicas: 1`** — dedupe is
per-process.

> **Prerequisite:** the acceptor's Safe smart account (`ACCEPTOR_PRIVATE_KEY` /
> `ACCEPTOR_SPACE_ID`) must be an **editor** of every space in
> `MEMBERSHIP_AUTOACCEPT_SPACE_IDS`, or its votes revert (`benign`, logged loudly).

### Policies (the BYO extension point)

Requests pass through a cheap→expensive funnel:

```
detect → WHITELIST (config Set, no I/O) → dedupe → POLICY (GraphQL) → vote
```

A `Policy` is `(request, ctx) => Promise<{accept, reason}>`; `ctx` carries a
`GraphQLClient` so a space can express rules backed by API data (reputation,
payment, external auth, …) without touching the webhook/voting plumbing. Compose
several with `composePolicies(...)` (AND semantics, first denial wins).

The shipped reference policy is **`editorPolicy`**: it confirms the acceptor is an
editor of the target space via the `editor(spaceId, memberSpaceId)` GraphQL query.
It **fails open** — an API error/timeout never suppresses a vote; the chain stays
the final authority (a genuine non-editor just reverts). Configure the endpoint
with `GRAPHQL_ENDPOINT`. Add your own policies in `src/index.ts`:
`composePolicies(editorPolicy, myReputationPolicy)`.

> Implementation note: unlike `proposal-executor` (an Effect-TS CronJob), this is
> a plain `Bun.serve` HTTP server. The on-chain pieces (ABI, `enter()` encoding,
> Safe smart wallet) are ported from `proposal-executor` because the published
> `@geoprotocol/geo-sdk` voting helpers are hardcoded to testnet in the current
> version and cannot drive a mainnet vote.

## Layout

| Path | Purpose |
|---|---|
| `src/index.ts` | Entry point — parse config, start server, graceful shutdown |
| `src/server.ts` | Routes + request handling (`createApp` returns a testable fetch handler) |
| `src/detect.ts` | Membership-request detection + `proposal_id` de-duplication |
| `src/vote.ts` | The acceptor: whitelist + run policy + cast the YES vote, classify the result |
| `src/policy.ts` | Policy seam + `composePolicies` + the reference `editorPolicy` |
| `src/graphql.ts` | Minimal GraphQL client used by policies |
| `src/wallet.ts` | Safe smart-account setup (Pimlico-sponsored) |
| `src/contracts.ts` | SpaceRegistry ABI, chains, `enter()` encoding, revert classification |
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
