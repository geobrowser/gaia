# Proposal Auto-Executor — Runbook

## What This Service Does

Detects slow-path governance proposals in EXECUTABLE status and calls `enter(PROPOSAL_EXECUTED)` on the Space Registry contract to execute them on-chain. Runs as a K8s CronJob every 5 minutes.

- **Fast-path proposals** don't need this — they auto-execute in the smart contract when the decisive YES vote lands
- **Slow-path proposals** require an explicit on-chain call after voting ends and quorum + threshold conditions are met — that's what this service does

The executor uses a Safe smart account with Pimlico gas sponsorship, so it needs **no ETH balance**.

## Architecture

```
K8s CronJob (every 5 min)
  → Read PostgreSQL for EXECUTABLE slow-path proposals
  → Group by space
  → Execute in parallel across spaces, sequential within each space
  → Call enter(PROPOSAL_EXECUTED) on Space Registry via Safe + Pimlico
  → Exit
```

Parallel across spaces, sequential within each. The bottleneck is the slowest single space's sequential chain, not the total proposal count.

## Environment Variables

| Variable | Sensitive | Description |
|---|---|---|
| `DATABASE_URL` | Yes | PostgreSQL connection string. **Must use a read-only user.** |
| `EXECUTOR_PRIVATE_KEY` | Yes | 0x-prefixed, 64 hex chars. Private key for the EOA that owns the Safe. |
| `PIMLICO_API_KEY` | Yes | Pimlico bundler/paymaster API key. |
| `EXECUTOR_SPACE_ID` | No | bytes16, 0x-prefixed. The executor's personal space ID (public on-chain data). |
| `SPACE_REGISTRY_ADDRESS` | No | Space Registry contract address. |
| `RPC_URL` | Yes | Chain RPC endpoint (must be `http://` or `https://`). May contain API keys in the path. |
| `CHAIN_ID` | No | `19411` (testnet). Mainnet (`80451`) is not yet deployed. |

> **Sentry** is optional. When `SENTRY_DSN` is set, ERROR/FATAL logs create Sentry issues and all logs become breadcrumbs. OTel spans are routed through Sentry for tracing. When not set, falls back to structured JSON console logging only.

Sensitive values come from K8s Secrets (`proposal-executor-credentials`) in the `knowledge` namespace. Non-sensitive values are plain in `cronjob.yaml`.

Optional Sentry env vars (also from K8s Secret, all `optional: true`):

| Variable | Description |
|---|---|
| `SENTRY_DSN` | Sentry DSN. If not set, telemetry falls back to console-only. |
| `SENTRY_ENVIRONMENT` | e.g. `production`, `staging`. Defaults to `production`. |
| `SENTRY_RELEASE` | Release version tag. |
| `SENTRY_TRACES_SAMPLE_RATE` | Trace sampling rate (0.0–1.0). Defaults to `1.0`. |
| `SENTRY_DEBUG` | Set to `"true"` for Sentry debug logging. |

## First-Time Setup

### 1. Create the executor's personal space

The `enter()` function requires the caller to have a registered personal space. The Safe smart account address is deterministic from the private key, so:

1. Generate a private key (or reuse an existing EOA)
2. Derive the Safe smart account address — run the service locally with just `EXECUTOR_PRIVATE_KEY`, `PIMLICO_API_KEY`, `RPC_URL`, and `CHAIN_ID` set. It will log the Safe address at `wallet_ready` before failing on the missing space.
3. Register a personal space for that Safe address using `geo space create` (via geo-cli)
4. Set the resulting space ID as `EXECUTOR_SPACE_ID`

**The service verifies this on startup** — it calls `addressToSpaceId(safeAddress)` and fails fast if the space isn't registered or doesn't match `EXECUTOR_SPACE_ID`.

### 2. Edit deployment values

Before applying, edit the environment-specific `deployment/<env>/cronjob.yaml` and fill in the `EXECUTOR_SPACE_ID` placeholder:

- `EXECUTOR_SPACE_ID` — from step 1.4 above

`SPACE_REGISTRY_ADDRESS`, `RPC_URL`, and `CHAIN_ID` are pre-filled per environment. `EXECUTOR_SPACE_ID` is the only value you must set — it's empty by default and the service will crash without it.

### 3. Create K8s resources

```bash
# Both environments deploy into the existing 'knowledge' namespace.

# For staging (testnet):
kubectl apply -f deployment/staging/cronjob.yaml

# For production (mainnet):
kubectl apply -f deployment/production/cronjob.yaml
```

Secrets are created separately (see secrets.yaml.example for the template).

### 4. Verify

```bash
# Trigger a manual run
kubectl create job --from=cronjob/proposal-executor test-run -n knowledge

# Watch logs
kubectl logs -f job/test-run -n knowledge
```

Look for:
- `wallet_ready` with the correct `safeAddress`
- `run_start` with `proposalsFound` count
- `run_end` with succeeded/failed/skipped counts
- Exit code 0

## How to Read the Logs

All logs are structured JSON with a `runId` UUID that correlates events within a single CronJob invocation.

> **`proposal_skip_expected` is normal and harmless — ignore it.** It means a proposal was already executed on-chain but the database hasn't caught up yet. See "Expected Reverts" below.

### Key Events

| Event | Level | Meaning |
|---|---|---|
| `wallet_ready` | INFO | Smart wallet initialized. Check `safeAddress`. |
| `run_start` | INFO | Detection complete. `proposalsFound` and `spaces` show scope. |
| `proposal_executed` | INFO | Success. Contains `txHash`, `proposalId`, `spaceId`. |
| `proposal_skip_expected` | INFO | Expected revert (e.g., already executed). **Not an error.** |
| `proposal_reverted` | INFO | Unexpected revert. Proposal skipped, execution continues. |
| `space_aborted` | ERROR | InfraError exhausted retries for a space. Other spaces continue. |
| `run_end` | INFO | Summary: `succeeded`, `failed` (spaces aborted, not proposals), `skipped`, `total`, `durationMs`. |
| `run_failed` | ERROR | Top-level failure (config error, DB unreachable, timeout). |
| `fatal` | ERROR | Unhandled defect (bug). May lack `runId` — occurs outside Effect runtime. |
| `db_disconnected` | DEBUG | Finalizer ran, DB connection closed. |

> **`failed` in `run_end` counts spaces that aborted, not individual proposals.** Proposals in aborted spaces were never attempted and will retry next cycle.

### Expected Reverts

When the executor submits a transaction, the proposal's `executed_at` stays NULL in the database until the kg-indexer processes the on-chain event (seconds to minutes of lag). During this window, the next CronJob run sees the same proposal as EXECUTABLE and re-submits. The contract reverts ("already executed"), and the executor logs it as `proposal_skip_expected`.

**This is by design.** Gas is sponsored so reverts are free. There's no on-chain view function to check execution state.

## Timeouts and Deadlines

| Timeout | Value | Purpose |
|---|---|---|
| K8s `activeDeadlineSeconds` | 290s | Hard kill — 10s gap before next 5-min schedule |
| Effect top-level timeout | 270s | Graceful shutdown — 20s margin for finalizers (DB disconnect) before K8s kills |
| Per-proposal execution timeout | 30s | Prevents a single hung UserOperation from blocking the pipeline |
| DB `connectionTimeoutMillis` | 5s | Fail fast if DB/PgBouncer unreachable |
| DB `statement_timeout` | 30s | Cap on detection query (normally <100ms) |

### Performance

A single proposal takes **~3–8 seconds** end-to-end (gas estimation → paymaster → bundler → confirmation). With retries, worst case per-proposal is ~90s (3 attempts × 30s timeout).

If a single space has **>25 proposals** queued simultaneously, the run may hit the 270s timeout. This is unlikely — proposals have staggered voting periods — but if it happens, unprocessed proposals are picked up in the next 5-minute cycle. Only worry if the same proposals are stuck across multiple cycles (see "Proposals stuck in EXECUTABLE" below).

## Error Handling

| Type | Behavior |
|---|---|
| On-chain revert | Skip proposal, continue. Never retried. |
| Infrastructure failure (Pimlico, RPC) | Retry per-proposal: 2 retries with exponential backoff (0s, 1s, 2s). Only infra errors retry — reverts never retry. If retries exhausted, abort remaining proposals in that space. Other spaces continue independently. |

If Pimlico is fully down, every space independently exhausts retries on its first proposal and aborts — a natural kill switch.

To add new revert patterns to the classification, update `REVERT_ERROR_NAMES` or `REVERT_MESSAGE_PATTERNS` in `execute.ts`.

## Exit Codes

| Code | Meaning |
|---|---|
| `0` | At least one proposal succeeded, OR no proposals found, OR partial success |
| `1` | Every attempt failed (systemic issue — all spaces aborted) |

Partial success exits `0` because individual proposal failures don't indicate systemic problems.

## K8s Configuration

```
schedule: */5 * * * *
concurrencyPolicy: Forbid         # No overlapping runs
restartPolicy: Never               # Retry via next scheduled run
backoffLimit: 1                    # K8s creates one retry pod if first pod fails
activeDeadlineSeconds: 290         # Hard kill before next invocation
runAsNonRoot: true (UID 1001)
allowPrivilegeEscalation: false
readOnlyRootFilesystem: true
capabilities: drop ALL
```

> **`backoffLimit: 1` note:** With `restartPolicy: Never`, this creates a second _pod_ (not a container restart) if the first pod fails. Two failed pods per Job during systemic failures (e.g., DB down) is expected behavior.

**Resources:** 256Mi–512Mi memory, 250m–500m CPU.

## Operational Procedures

### Suspend / Resume

During incidents (Pimlico outage, contract upgrade, DB maintenance):

```bash
# Suspend — stops scheduling new runs
kubectl patch cronjob proposal-executor -n knowledge -p '{"spec":{"suspend":true}}'

# Resume
kubectl patch cronjob proposal-executor -n knowledge -p '{"spec":{"suspend":false}}'

# Verify
kubectl get cronjob proposal-executor -n knowledge -o jsonpath='{.spec.suspend}'
```

### Key Rotation / Compromise Response

Rotating the private key changes the Safe address, which invalidates the personal space. Follow this order:

**If compromised — do step 0 first:**

0. **Suspend the CronJob immediately** (see above)

**Rotation steps:**

1. Generate a new private key
2. Derive the new Safe smart account address (run the service locally with the new key — see First-Time Setup step 2)
3. Register a new personal space for the new Safe address using `geo space create`
4. Update `EXECUTOR_SPACE_ID` in `cronjob.yaml` with the new space ID
5. Update `EXECUTOR_PRIVATE_KEY` in the K8s Secret
6. Apply updated resources: `kubectl apply -f deployment/cronjob.yaml` and update the secret
7. Resume the CronJob (if suspended)
8. Trigger a manual run to verify (see First-Time Setup step 4)

### Manual Proposal Execution

If a proposal is stuck and the automated service can't handle it (e.g., persistent unexpected revert), use `geo-cli` to call `enter(PROPOSAL_EXECUTED)` directly.

## Edge Cases

### Clock Skew

The detection SQL adds a **60-second buffer** after `end_time` before considering a proposal executable. This guards against clock skew between the CronJob pod's wall clock and on-chain `block.timestamp`.

### Ordering Within a Space

Proposals in the same space execute sequentially in FIFO order. If an earlier proposal's execution fails (after retries), remaining proposals in that space are aborted — they'll retry next cycle.

### Detection SQL Drift

The executable-proposal detection logic exists in three places:
1. `api/src/proposals/status.ts` — `computeProposalStatus()` (pure TypeScript)
2. `api/src/proposals/queries.ts` — `sqlIsExecutable()` (SQL fragments)
3. `proposal-executor/src/detect.ts` — Detection SQL

The executor's copy is intentionally stricter (60s clock-skew buffer). If the threshold/quorum logic changes in the API, **the executor's SQL must be updated too**. The test suite cross-validates `RATIO_BASE` against the API's constant, but the formula shape and `voting_mode` enum values are not automatically verified — check manually when the API changes.

Run `bun test` in `proposal-executor/` to verify cross-validation passes.

### Proposals With Reverting Actions (`ActionReverted`)

Some proposals pass all governance checks (quorum met, threshold met, voting ended) but contain **embedded actions that revert** when executed. For example, a proposal to `addMember` where the member was already added by another proposal, or `removeEditor` when the editor already left.

On-chain, the DAOSpace contract's `_executeProposal` executes each action sequentially and reverts with `ActionReverted()` (selector `0x24c05f9a`) if any action's `.call()` fails. Because the entire transaction reverts, `proposal.executed` is never set to `true` on-chain, and `executed_at` stays `NULL` in the database.

**Mitigation:** The detection SQL includes a **7-day age cutoff** (`MAX_PROPOSAL_AGE`). Proposals older than 7 days from creation are excluded from detection, so stuck proposals age out naturally without any per-proposal state tracking. This is well beyond the typical 1-day voting period.

**During the 7-day window:** Stuck proposals will be retried every CronJob cycle until they age out. This is harmless — gas is sponsored and reverts complete in <500ms — but creates some log noise.

**How to identify them:** Look for `proposal_reverted` logs where the error contains `0x24c05f9a` or `ActionReverted`. Unlike transient infra errors, these will repeat with the same proposal IDs across runs until the 7-day cutoff elapses.

**Manual resolution:** If a stuck proposal's embedded actions can be fixed (e.g., the proposal creator updates the proposal via `PROPOSAL_UPDATED`), it will succeed on the next cycle — as long as it's still within the 7-day window.

### Invalid UUIDs in Database

If the proposals table contains a malformed proposal or space ID, `uuidToBytes16` throws, which is classified as an `InfraError`. This triggers retries and then aborts the space. Look for `space_aborted` with "Invalid UUID for bytes16 conversion" — this is a data issue, not an infra issue.

## Troubleshooting

### Service fails on startup

| Symptom | Likely Cause | Fix |
|---|---|---|
| `Config error: ...` | Missing or malformed env var | Check K8s Secret and cronjob.yaml values |
| `Invalid EXECUTOR_SPACE_ID` | Not 0x-prefixed bytes16 (34 chars) | UUID → strip dashes → prefix 0x |
| `Invalid EXECUTOR_PRIVATE_KEY` | Not 0x-prefixed 64 hex chars | Check key format, add 0x prefix if missing |
| `Invalid SPACE_REGISTRY_ADDRESS` | Not a valid checksummed address | Use `viem.getAddress()` to checksum |
| `Invalid CHAIN_ID` | Not 80451 or 19411 | Only testnet (19411) is currently deployed. Mainnet (80451) not yet available. |
| `Invalid RPC_URL` | Empty or not a URL | Must start with `http://` or `https://` |
| `DB connect failed: [ECONNREFUSED]` | DB unreachable | Check DATABASE_URL, network policies, PgBouncer |
| `Executor Safe ... has no registered personal space` | Personal space not created | Run `geo space create` for the Safe address |
| `On-chain space ID ... does not match configured EXECUTOR_SPACE_ID` | Wrong EXECUTOR_SPACE_ID | Verify against on-chain `addressToSpaceId(safeAddress)` |

### All proposals revert

| Symptom | Likely Cause | Fix |
|---|---|---|
| All `proposal_reverted` | Wrong `SPACE_REGISTRY_ADDRESS` or ABI change | Verify address matches deployed contract |
| All "already executed" | kg-indexer caught up between runs | Normal — these are `proposal_skip_expected` |
| Same proposals revert with `0x24c05f9a` every cycle | Embedded actions revert (`ActionReverted`) | Proposals are permanently stuck — see "Proposals With Reverting Actions" in Edge Cases |
| `Smart wallet creation failed` | Pimlico API key invalid or budget exhausted | Check Pimlico dashboard |

### CronJob not running

```bash
# Check CronJob status
kubectl get cronjob proposal-executor -n knowledge

# Check recent jobs
kubectl get jobs -n knowledge --sort-by=.metadata.creationTimestamp

# Check for suspended CronJob
kubectl get cronjob proposal-executor -n knowledge -o jsonpath='{.spec.suspend}'
```

### Proposals stuck in EXECUTABLE

1. Check logs — is the service running and finding them?
2. If `proposalsFound: 0` — the 60s clock-skew buffer may not have elapsed yet. Wait and check again.
3. If proposals are found but all revert with `0x24c05f9a` — these have reverting embedded actions. See "Proposals With Reverting Actions" in Edge Cases.
4. If proposals revert with other errors — check contract state, ABI compatibility.
5. If the service isn't running — check CronJob schedule and namespace.
6. If the same proposals are stuck across 3+ cycles and the revert is *not* `ActionReverted` — investigate manually (see "Manual Proposal Execution" above).

## Monitoring Checklist

| Alert | Status | Notes |
|---|---|---|
| **Sentry error tracking** | Wired | `space_aborted`, `run_failed`, `fatal` events create Sentry issues when `SENTRY_DSN` is set. |
| **OTel tracing** | Wired | Spans for `proposal-executor.run`, `.detect`, `.execute-proposal` routed through Sentry. |
| **Pimlico budget** | TODO: Not yet configured | Monitor sponsorship balance. Budget exhaustion stops all executions. |
| **CronJob failures** | TODO: Not yet configured | Alert on repeated exit code 1 (systemic failure). |
| **Proposals stuck >15 min** | TODO: Not yet configured | Alert if proposals stay EXECUTABLE beyond 3 CronJob cycles. |
| **kg-indexer lag** | Informational | Long lag increases expected reverts. Not harmful, but noisy. |
