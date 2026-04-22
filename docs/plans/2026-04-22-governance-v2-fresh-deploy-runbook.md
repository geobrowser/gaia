# Governance V2 Fresh-Deploy Runbook

**Status:** Draft — needs review.
**Ticket:** [GEO-520](https://linear.app/defi-wonderland/issue/GEO-520).
**Scope:** Bring up the Governance V2 stack on a fresh chain with empty state. Not a migration from V1.

---

## Purpose

The V2 stack deploys onto a new chain with new contracts, a new PostgreSQL database, and fresh Kafka topics. There is no V1 data to migrate and no backfill to run. This runbook is the bring-up order, per-layer health gate, end-to-end smoke test, and "wipe + retry" recovery procedure for that deployment.

Anything describing a V1 → V2 rolling migration is out of scope — there is no V1 in prod.

---

## Pre-deployment checklist

Before touching any service, confirm the following:

| Item | Signal / command |
|---|---|
| Chain online + RPC reachable | `cast chain-id --rpc-url $RPC_URL` returns `19411` (testnet) or `80451` (mainnet) |
| V2 contracts deployed | `SpaceRegistry` address captured; matches `SPACE_REGISTRY_ADDRESS` env in `proposal-executor/deployment/<env>/cronjob.yaml:72` |
| Executor space created + funded | `EXECUTOR_SPACE_ID` exists on-chain; the space's Safe smart account has ETH for gas (Pimlico sponsors user-ops but each space still needs minimal ETH for Safe init) |
| PostgreSQL provisioned, empty | `psql $DATABASE_URL -c "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public'"` → 0 |
| Drizzle migrations ready | `ls api/drizzle/*.sql | wc -l` matches the latest `api/drizzle/meta/_journal.json` entry count |
| Kafka cluster reachable | `kafka-topics --bootstrap-server $KAFKA_BROKER --list` returns (can be empty; topics auto-create on first produce per cluster config) |
| Kafka consumer groups clean | `kafka-consumer-groups --bootstrap-server $KAFKA_BROKER --list | grep kg-indexer` returns nothing — otherwise reset before bring-up (see [Recovery](#failure-recovery-during-bring-up)) |
| Substream endpoint + token | `SUBSTREAMS_ENDPOINT` reachable (default `geotest.substreams.pinax.network:443`); `SUBSTREAMS_API_TOKEN` valid |
| Starting block captured | `hermes-substream/substreams.yaml:21` → `initialBlock: 81809`. Override per deploy with `SUBSTREAMS_START_BLOCK` if pointing at a different fresh-chain genesis |
| All K8s secrets populated | `DATABASE_URL`, `DATABASE_URL_DIRECT`, `KAFKA_PASSWORD`, `KAFKA_SSL_CA_PEM`, `EXECUTOR_PRIVATE_KEY`, `PIMLICO_API_KEY`, `SENTRY_DSN` |
| Sentry project configured | `SENTRY_DSN` set per service (`api`, `kg-indexer`, `hermes-pipeline`, `proposal-executor`); `SENTRY_ENVIRONMENT` matches target env |

> TODO — confirm before deploy: the exact chain (mainnet = 80451 or testnet = 19411), V2 `SPACE_REGISTRY_ADDRESS`, and `EXECUTOR_SPACE_ID` values for this rollout.

---

## Bring-up sequence

Deploy in the listed order. Do not proceed to the next step until the health gate for the current step is green.

### 1 — PostgreSQL

Apply migrations. Nothing else reads/writes yet.

```
bun --cwd api run db:migrate
```

The API `Deployment` manifest runs this automatically via an `initContainer` (`api/k8s/staging/api.yaml:52-72`) but for a fresh cluster you often want to apply migrations manually first so the API doesn't restart-loop on migration failure.

**Gate:**
- `SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public'` > 0.
- Highest `idx` in `api/drizzle/meta/_journal.json` matches `SELECT MAX(CAST(version AS int)) FROM drizzle.__drizzle_migrations` (or equivalent — drizzle-kit's bookkeeping table).

### 2 — Kafka topics + consumer group

Either rely on `auto.create.topics.enable=true`, or pre-create the topics hermes-pipeline produces to (`space.creations`, `space.membership`, `space.trust.extensions`, `space.topics`, `space.governance`, `space.moderation`, `knowledge.edits`, `curation.votes`, `hermes.blocks`).

For a fresh deploy, the consumer group `KAFKA_GROUP_ID` on kg-indexer should not exist yet — if it does (from a prior failed attempt), reset it to earliest before starting:

```
kafka-consumer-groups --bootstrap-server $KAFKA_BROKER \
  --group $KAFKA_GROUP_ID --reset-offsets --to-earliest --all-topics --execute
```

**Gate:**
- `kafka-topics --list` shows the expected topics.
- `kafka-consumer-groups --describe --group $KAFKA_GROUP_ID` either returns "Group not found" or shows offsets at earliest.

### 3 — hermes-pipeline

Deploy `hermes/k8s/<env>/hermes-pipeline.yaml`. It connects to Substreams (gRPC) and produces to Kafka. Stateless — no DB, no cursor table; Substreams tracks the cursor internally.

**Env vars (see manifest):** `SUBSTREAMS_ENDPOINT`, `SUBSTREAMS_API_TOKEN`, `KAFKA_BROKER`, `KAFKA_USERNAME`, `KAFKA_PASSWORD`, `KAFKA_SSL_CA_PEM`, `SENTRY_DSN`, `ENVIRONMENT`, optional `SUBSTREAMS_START_BLOCK` / `SUBSTREAMS_END_BLOCK`.

**Gate:**
- Pod log contains `Connected to Kafka broker` (`hermes-pipeline/src/main.rs:859`).
- Pod log contains a Substreams block stream start line (no connection errors in the first 60s).
- `kafka-consumer-groups --describe --group <any-consumer>` on `space.creations` or similar shows the topic has messages (log-end-offset advancing).
- Sentry: no new issues for the service in the first 5 minutes after start.

> **Gap — no HTTP health endpoint.** hermes-pipeline has no `/health` probe. Treat "process alive + Kafka offsets advancing + Sentry clean" as the readiness signal.

### 4 — kg-indexer

Deploy `kg-indexer/k8s/<env>/kg-indexer.yaml`. Subscribes to the topics listed in `kg-indexer/src/main.rs` (`knowledge.edits`, `space.creations`, `space.membership`, `space.trust.extensions`, `space.topics`, `space.governance`) plus `hermes.blocks` for batch-close signals. Writes to Postgres.

**Env vars:** `DATABASE_URL`, `KAFKA_BROKER`, `KAFKA_USERNAME`, `KAFKA_PASSWORD`, `KAFKA_SSL_CA_PEM`, `KAFKA_GROUP_ID`, `BLOCK_STALE_TIMEOUT_MS` (default 1000), `TALLY_WORKER_INTERVAL_MS`, `RUST_LOG`, `SENTRY_DSN`.

**Gate:**
- Pod log emits `kg_indexer.batch_end` lines — each block the indexer processes produces one. Missing = pipeline not flowing.
- `SELECT MAX(block_number) FROM atlas_checkpoints WHERE indexer_id = 'kg_indexer'` advances (query every ~30s during bring-up).
- Consumer lag bounded: `kafka-consumer-groups --describe --group $KAFKA_GROUP_ID` shows `LAG` column decreasing (or near-zero once caught up).
- Sentry: no new issues for the service in the first 5 minutes.

> **Gap — no K8s probes on kg-indexer.** The manifest (`kg-indexer/k8s/staging/kg-indexer.yaml`) defines neither liveness nor readiness. Catching up is inferred from logs + DB + consumer lag, not from a probe.

### 5 — API

Deploy `api/k8s/<env>/api.yaml`. Serves REST + GraphQL from Postgres. Init-container sequence: `setup-certs` → `migrate` → main container.

**Env vars:** `DATABASE_URL`, `DATABASE_URL_DIRECT`, `PG_CONNECTION_TIMEOUT_MS`, `PG_POOL_PRESSURE_*`, `OPENSEARCH_URL`, `TOPOLOGY_SERVICE_URL`, `VALKEY_URL`, `SENTRY_DSN`, `ENVIRONMENT`.

**Gate:**
- `curl $API_URL/health/liveness` → 200.
- `curl $API_URL/health/readiness` → 200 (requires DB reachable; 1s `SELECT 1` timeout per `api/src/health.ts:150-215`).
- `curl $API_URL/health/detailed` → 200 with `utilizationPercent < 50` on both pools.
- `curl $API_URL/proposals/space/<test-space-id>/status` returns a well-formed response (even if the proposals array is empty; it exercises the V2 `proposals_current` path).

### 6 — proposal-executor

Deploy `proposal-executor/deployment/<env>/cronjob.yaml`. CronJob, runs every 5 minutes (`*/5 * * * *`) with `activeDeadlineSeconds: 290`. `concurrencyPolicy: Forbid` prevents overlap.

**Env vars:** `DATABASE_URL`, `EXECUTOR_PRIVATE_KEY`, `PIMLICO_API_KEY`, `EXECUTOR_SPACE_ID`, `SPACE_REGISTRY_ADDRESS`, `RPC_URL`, `CHAIN_ID`, `SENTRY_DSN`.

**Gate:**
- Trigger a one-off run: `kubectl create job --from=cronjob/proposal-executor exec-bringup -n <ns>`.
- Pod log emits `wallet_ready` then `run_start` then `run_end`.
- Exit code 0 (even "no proposals found" is a success).
- Sentry: no issues from the trigger run.

See `proposal-executor/RUNBOOK.md` for deeper operational procedures (suspend/resume, key rotation, error classification).

---

## End-to-end smoke test

Only run after every layer above is green.

1. Submit a test proposal on-chain — fastest path is a `SUBSPACE_VERIFIED` proposal in the executor space, since it requires no content CID upload. (`docs/plans/subspace-verified-proposal.md` has the encoding; there is also a related wiki page.)
2. Observe it flow:
    - Substream → hermes-pipeline produces to `space.governance` (kafka topic offset advances).
    - kg-indexer logs `kg_indexer.batch_end` for the block, `event_count >= 1`.
    - `SELECT id, current_version FROM proposals WHERE space_id = '<test-space>' ORDER BY created_at DESC LIMIT 1` returns the new row.
    - `SELECT executed_at FROM proposals WHERE id = '<proposal-id>'` returns NULL while voting (identity-level executed_at from GEO-531).
3. Cast a YES vote that meets `flat_support_threshold` → triggers V2 inline Fast-path auto-exec on-chain. Expected signals:
    - `PROPOSAL_EXECUTED` event flows through hermes-pipeline.
    - kg-indexer stamps `executed_at` on the proposal row (`update_proposal_executed` via `KgMessage::ProposalExecuted`).
    - `SELECT executed_at FROM proposals WHERE id = '<proposal-id>'` returns the execution timestamp.
    - API: `GET /proposals/space/<space>/status` returns the proposal with `status: "ACCEPTED"`.
4. Separately, create a Slow proposal that passes threshold + quorum after voting ends. Within ~5 minutes (CronJob cadence):
    - proposal-executor's next run picks it up via `detect.ts` → calls `enter(PROPOSAL_EXECUTED)`.
    - Log `proposal_executed` emitted.
    - Same `executed_at` stamping path as Fast-path; API reflects `ACCEPTED`.

---

## Go-live criteria

Declare the system live only when **all** of:

- [ ] All bring-up layer gates above have been green for ≥ 30 minutes.
- [ ] End-to-end smoke test completed for both Fast-path and Slow-path proposals.
- [ ] Sentry: zero new issues across `hermes-pipeline`, `kg-indexer`, `api`, `proposal-executor` for 1 hour.
- [ ] Grafana: API Ingress Observability dashboard shows normal request latency; Atlas Overview shows no stuck blocks. (`docs/observability.md:64-69`.)
- [ ] `SELECT MAX(block_number) FROM atlas_checkpoints` is within one block of `cast block-number --rpc-url $RPC_URL`.
- [ ] Kafka consumer lag on `kg-indexer` group is under the `BLOCK_STALE_TIMEOUT_MS` window.
- [ ] Teammate sign-off (see [Communication](#communication)).

---

## Failure recovery during bring-up

The V2 stack has no production data to preserve during bring-up. The canonical recovery path is **wipe + retry**. Pre-prod window — no user impact.

### Wipe-and-retry procedure

1. **Pause writers.** Scale `hermes-pipeline` and `kg-indexer` deployments to 0 replicas. Suspend the `proposal-executor` CronJob (`kubectl patch cronjob proposal-executor -p '{"spec":{"suspend":true}}'`).
2. **Reset Kafka.** Either delete topics and let them auto-recreate, or reset the consumer group to earliest:
    ```
    kafka-consumer-groups --bootstrap-server $KAFKA_BROKER \
      --group $KAFKA_GROUP_ID --reset-offsets --to-earliest --all-topics --execute
    ```
3. **Wipe Postgres.** `DROP SCHEMA public CASCADE; CREATE SCHEMA public;` then `bun --cwd api run db:migrate`.
4. **Fix the code** that caused the failure; redeploy the affected service.
5. **Restart the bring-up** from step 1 (Postgres is already done) through step 6. Smoke test again.

### Re-index timing estimate

> **Gap — not yet measured.** The full re-index walltime from `initialBlock: 81809` to current chain head isn't documented. Recommend a dry run on staging before go-live to establish the baseline. The tight path is likely Substreams endpoint throughput × kg-indexer transaction commit rate; Kafka partition parallelism is currently single-consumer on the `kg-indexer` group.
>
> Rough order of magnitude for planning: if staging indexes at ~100 blocks/s and the chain has ~1M blocks since fork, that's ~3 hours for a full re-index. Measure before relying on this estimate.

### When to wipe vs. when to forward-fix

Wipe:
- Schema migration ordering broke the DB.
- Indexing logic produced corrupt state (negative counts, missing rows, wrong `executed_at`).
- Consumer group lag is so high re-processing is faster than catch-up.

Forward-fix:
- Transient infra issue (Kafka broker restart, RPC blip). Pods will reconnect; check Sentry for the next 10 minutes.
- One-off decode error on a specific block that we've patched. Deploy the fix and let the event re-flow.

---

## Communication

**Slack channels:**
- `#infra-alerts` — Alertmanager (Prometheus) + Sentry notifications. (`docs/observability.md:75-80`.)

**During bring-up:**
- Post start / gate-pass / go-live milestones to the governance team channel (TODO — name the channel).
- If a gate fails, say so in channel with the log excerpt before starting the wipe.

**If governance is degraded after go-live:**
- TODO — fill in the on-call rotation or primary points-of-contact for each layer. No on-call rotation is documented in the repo today.
- Short-term: page the teammate listed in the relevant README — `api/`, `kg-indexer/`, `proposal-executor/` all have READMEs.

---

## Known gaps

Items this doc intentionally leaves open, to be resolved before deploy:

1. **Target chain decision** — mainnet (80451) vs. testnet (19411). The runbook's `CHAIN_ID`, `SPACE_REGISTRY_ADDRESS`, `EXECUTOR_SPACE_ID`, and starting block are env-specific.
2. **hermes-pipeline readiness signal** — no HTTP health endpoint; consider adding one before the next fresh deploy.
3. **kg-indexer K8s probes** — the manifest has neither liveness nor readiness. Fine for now (startup = event-loop assumption), but worth adding an atlas-checkpoint-progress-based readiness probe.
4. **Re-index walltime baseline** — measure on staging.
5. **On-call rotation** — no rotation documented for governance V2. Name primary / secondary contacts per layer.
6. **Governance Slack channel** — name the channel for deploy-milestone communication.

---

## References

- `docs/plans/2026-03-23-feat-geo-governance-mainnet-indexing-plan.md` — original V2 plan.
- `docs/plans/2026-04-01-feat-governance-v2-contract-migration-plan.md` — contract migration plan.
- `docs/observability.md` — tracing / metrics / alert channels (verified 2026-03-25).
- `proposal-executor/RUNBOOK.md` — executor-specific operational procedures.
- K8s manifests:
  - `api/k8s/<env>/api.yaml`
  - `hermes/k8s/<env>/hermes-pipeline.yaml`
  - `kg-indexer/k8s/<env>/kg-indexer.yaml`
  - `proposal-executor/deployment/<env>/cronjob.yaml`
- Epic: [GEO-469](https://linear.app/defi-wonderland/issue/GEO-469).
