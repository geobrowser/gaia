---
title: "feat: Auto-execute EXECUTABLE slow-path governance proposals"
type: feat
date: 2026-03-02
---

# feat: Auto-execute EXECUTABLE slow-path governance proposals

## Overview

A standalone TypeScript/Bun K8s CronJob that detects slow-path governance proposals in EXECUTABLE status and automatically executes them on-chain by calling `enter(PROPOSAL_EXECUTED)` on the Space Registry contract. Runs every 5 minutes, processes proposals in FIFO order, and uses a Safe smart account with Pimlico gas sponsorship (no ETH balance needed).

**Brainstorm:** `docs/brainstorms/2026-03-02-proposal-auto-executor-brainstorm.md`

## Problem Statement

Slow-path proposals require an explicit on-chain `enter(PROPOSAL_EXECUTED)` call after voting ends and the threshold + quorum conditions are met. Today, nobody triggers this automatically — proposals sit in EXECUTABLE state indefinitely until a user manually executes them. This degrades the governance experience (users expect proposals to "just work") and creates reliability risk (execution depends on someone remembering to act).

Fast-path proposals don't have this problem — the smart contract auto-executes them inline when the decisive YES vote arrives.

## Proposed Solution

A K8s CronJob that runs a TypeScript/Bun script every 5 minutes:

1. **Detect** — Query PostgreSQL for slow-path proposals where `executed_at IS NULL`, voting has ended, quorum is met, and threshold is reached
2. **Execute** — For each proposal (oldest first), submit `enter(PROPOSAL_EXECUTED)` via Safe smart account with Pimlico gas sponsorship
3. **Skip on failure** — If a tx reverts, log it, skip the proposal, continue with the next
4. **Exit** — Process completes and the container exits

## Technical Approach

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│ K8s CronJob (every 5 min)                               │
│                                                         │
│  ┌──────────┐    ┌──────────┐    ┌────────────────────┐ │
│  │ Detect   │───▶│ Execute  │───▶│ Log & Exit         │ │
│  │ (SQL)    │    │ (on-chain│    │                    │ │
│  │          │    │  via Safe │    │                    │ │
│  └──────────┘    │ +Pimlico) │    └────────────────────┘ │
│       │          └──────────┘                            │
│       ▼                │                                 │
│  PostgreSQL       Space Registry                         │
│  (read)           (write)                                │
└─────────────────────────────────────────────────────────┘
```

### Detection Query

Reuses the slow-path branch of `sqlIsExecutable()` from `api/src/proposals/queries.ts`:

```sql
SELECT p.id, p.space_id, s.address AS space_address
FROM proposals p
JOIN spaces s ON s.id = p.space_id
WHERE p.executed_at IS NULL
  AND p.voting_mode = 'Slow'
  AND $1::bigint > p.end_time + 60       -- 60s buffer for clock skew
  AND (p.yes_count + p.no_count + p.abstain_count) >= p.quorum
  -- RATIO_BASE = 10_000_000 (protocol constant from api/src/proposals/types.ts)
  AND (10000000 - p.threshold::numeric) * p.yes_count::numeric
      > p.threshold::numeric * p.no_count::numeric
ORDER BY p.created_at::bigint ASC        -- cast to bigint for safe numeric ordering
```

**Notes:**
- **No `LIMIT`** — Fetch all EXECUTABLE proposals in one query. The result set is bounded by reality (proposals that are simultaneously EXECUTABLE), and the DB read is fast. Throughput is bounded by on-chain submission, not the query. Results are grouped by `space_id` in the orchestration layer for parallel-by-space execution.
- `$1::bigint` — The `nowSeconds` parameter must be explicitly cast. Raw `pg` doesn't natively support JS BigInt, so pass `Number(nowSeconds)` and cast in SQL. All Unix timestamps fit safely in JS number precision.
- `+ 60` adds a 1-minute buffer for clock skew between the CronJob pod's wall clock and on-chain `block.timestamp`. Without this, proposals near their end time could cause systematic reverts. Hardcoded constant (not configurable).
- `RATIO_BASE` (10,000,000) is the protocol constant from `api/src/proposals/types.ts`. Inlined as a SQL literal with a comment referencing the source.
- `ORDER BY p.created_at::bigint ASC` — `created_at` is a Unix timestamp stored as `text`. The `::bigint` cast ensures correct numeric ordering regardless of string length variations.

### On-Chain Execution

Follows the exact pattern from `geo-cli/src/cli.ts:2235-2254`:

```typescript
import {encodeFunctionData, encodeAbiParameters} from "viem"

const calldata = encodeFunctionData({
  abi: SpaceRegistryAbi,
  functionName: "enter",
  args: [
    executorSpaceId,                              // bytes16: executor's personal space ID
    daoSpaceId,                                   // bytes16: DAO space ID (UUID → bytes16)
    GOVERNANCE_ACTIONS.PROPOSAL_EXECUTED,          // bytes32: action hash
    padBytes16ToBytes32(proposalId),               // bytes32: proposalId padded
    encodeProposalExecutedData(proposalIdHex),     // bytes: ABI-encoded proposalId
    EMPTY_SIGNATURE,                               // bytes: "0x"
  ],
})

const hash = await smartAccountClient.sendTransaction({
  account: safeAccount,
  chain,
  to: spaceRegistryAddress,
  data: calldata,
})
```

**Key encoding details:**
- UUID → bytes16: strip dashes, prefix with `0x` (e.g., `550e8400-e29b-41d4-a716-446655440000` → `0x550e8400e29b41d4a716446655440000`)
- `padBytes16ToBytes32`: append 32 zero hex chars (16 zero bytes)
- `encodeProposalExecutedData`: `encodeAbiParameters([{name: "proposalId", type: "bytes16"}], [proposalIdHex])`
- `GOVERNANCE_ACTIONS.PROPOSAL_EXECUTED` = `"0x62a60c0a9681612871e0dafa0f24bb0c83cbdde8be5a6299979c88d382369e96"`
- `EMPTY_SIGNATURE` = `"0x"` — signature is ignored when `msg.sender == _fromSpace` (see below)

### Smart Wallet Setup

Same stack as `geo-cli/src/wallet.ts:191-261`:

```
Private Key (env var) → EOA → Safe Smart Account → Smart Account Client (with Pimlico paymaster)
```

**Dependencies:** `permissionless` ^0.3.2, `viem` ^2.43.5, `effect` ^3.19.14

The Safe smart account address is deterministic from the owner EOA, so the same private key always produces the same smart account address.

### Executor Personal Space Prerequisite

The `enter()` function requires `_fromSpaceId` — the executor's registered personal space ID. The contract resolves the space ID and checks if `msg.sender == _fromSpace` (the registered address for that space). Since the Safe smart account IS the `msg.sender`, and the personal space is created for the Safe's address, `msg.sender == _fromSpace` holds and the `signature` parameter is ignored.

**One-time setup (before first deployment):**
1. Register a personal space for the executor's Safe smart account address using `geo space create` (or equivalent contract call)
2. Store the resulting space ID as the `EXECUTOR_SPACE_ID` environment variable
3. The service verifies this on startup by calling `addressToSpaceId(safeAddress)` on the Space Registry — if it returns zero bytes, fail fast with a clear error

### Race Condition: Double Execution Window

After the executor submits `enter(PROPOSAL_EXECUTED)` on-chain, the proposal's `executed_at` remains NULL in the database until the kg-indexer processes the resulting on-chain event (seconds to minutes of lag). During this window, the next CronJob run will see the same proposal as EXECUTABLE and attempt to re-execute it. The contract will revert (proposal already executed), and the executor will skip it via the normal skip-on-failure path.

**Mitigation:** Let it revert. The revert costs nothing (gas is sponsored) and `RevertError` with `expected: true` is logged at INFO, not ERROR.

**Alternatives considered:**
- **Check on-chain before executing** — The governance plugin contract doesn't expose a view function to read proposal execution state. We'd need to know the storage layout, which is fragile and undocumented.
- **Local tracking column** (`execution_submitted_at`) — Eliminates the race window but adds a DB write, breaking the service's read-only boundary. Also introduces new failure modes (tx submitted but failed on-chain → stale flag → proposal never retried). Over-engineered for a problem that costs nothing.
- **Check our API/subgraph** — Same data source as the DB, same kg-indexer lag. Doesn't help.

If volume grows and revert noise becomes a problem, the best future option is the local tracking column with a TTL (auto-clear after 10 minutes). But this is YAGNI for now.

### Error Classification

Not all failures are equal. The executor uses Effect-TS tagged errors for type-safe classification and composable retry/skip behavior:

| Error Type | Tagged Error | Behavior | Example |
|---|---|---|---|
| **On-chain revert** | `RevertError` | Caught via `catchTag` in caller, skip proposal, continue | Proposal already executed, voting not ended on-chain |
| **Infrastructure error** | `InfraError` | Retried per-proposal via `Effect.retry` with exponential backoff (up to 2 retries). If retries exhausted, propagates up — `Effect.forEach` fail-fast aborts remaining proposals in that space. | Pimlico 429, RPC timeout |
| **Timeout** | `RevertError` | `Effect.timeout(30s)` wraps each proposal, treated as skip | Single UserOperation hangs >30s |

**Retry + fail-fast replaces the manual error budget counter.** Instead of a `Ref` counting infra errors across all spaces:
- Each proposal gets up to 2 retries with `Schedule.exponential("1 second")` composed with `Schedule.recurs(2)`, gated on `InfraError._tag` only (reverts are never retried).
- If retries are exhausted, the `InfraError` propagates. `Effect.forEach` (which is fail-fast by default) aborts the remaining proposals **in that space**.
- Other spaces are independent fibers — they continue unless they also hit unrecoverable infra errors.
- If Pimlico is truly down, every space will independently exhaust retries on its first proposal and abort. The net effect is the same as a global kill switch, but compositional and without shared mutable state.

Expected reverts (e.g., "already executed" due to the race condition window) should be logged at INFO level with an `expected: true` flag, not ERROR. This keeps logs clean and distinguishes known race conditions from genuine failures.

## Implementation Phases

### Phase 1: Project Scaffold

Create the service directory and configuration:

```
proposal-executor/
├── .env.example
├── Dockerfile
├── package.json
├── tsconfig.json
├── biome.json
├── src/
│   ├── index.ts           # Effect program: config → detect → execute (parallel/space) → exit
│   ├── detect.ts           # DB connection + SQL query, returns Effect<Proposal[]>
│   ├── execute.ts          # Smart wallet, encoding, enter() call — tagged errors (RevertError, InfraError)
│   └── contracts.ts        # SpaceRegistryAbi subset (enter, addressToSpaceId) + chain defs
├── deployment/
│   ├── cronjob.yaml        # Single manifest — env-specific values come from K8s secrets
│   ├── namespace.yaml
│   └── secrets.yaml.example  # Template only — real secrets managed externally
└── tests/
    ├── detect.test.ts      # Detection query correctness
    ├── execute.test.ts     # Error classification + encoding correctness
    └── index.test.ts       # Orchestration logic: consecutive errors, exit codes, skip-on-failure
```

**Design rationale (4 source files, not 8):**
- `config.ts` → folded into `index.ts` as a `parseConfig()` function (~15 lines)
- `encoding.ts` → folded into `execute.ts` (only called there, ~20 lines of pure functions)
- `chain.ts` → folded into `contracts.ts` (one chain definition object literal)
- `db.ts` → folded into `detect.ts` (5 lines of `pg.Client` connect/query/end)
- Each remaining file has enough substance (~30-80 lines) to justify its existence

**Tasks:**
- [ ] Initialize `package.json` with `bun` runtime, deps: `effect` ^3.19.14, `viem` ^2.43.5, `permissionless` ^0.3.2, `pg`; dev deps: `vitest`, `@types/pg`
- [ ] Create `tsconfig.json` matching API conventions
- [ ] Create `biome.json` matching repo linting config
- [ ] Create `.env.example` with all required env vars documented (mark sensitive vs non-sensitive)
- [ ] Create `Dockerfile` (pinned `oven/bun:1.3.9`, non-root user, `NODE_ENV=production`)

### Phase 2: Detection

- [ ] `detect.ts` — Contains DB connection + query logic. All functions return `Effect` values:
  - `connectDb(databaseUrl)` → `Effect<pg.Client, InfraError>` — Single `pg.Client` connection (not a pool — this is a short-lived CronJob). Configured with comprehensive timeout and keepalive settings appropriate for a batch process with `activeDeadlineSeconds: 290`:
    - `connectionTimeoutMillis: 5000` — fail fast if DB/PgBouncer is unreachable (API uses 3s; slightly more generous for batch)
    - `statement_timeout: 30000` — 30s cap on the detection query (normally <100ms for an indexed scan)
    - `idle_in_transaction_session_timeout: 60000` — safety net against leaked transactions holding locks
    - `lock_timeout: 5000` — only reads, but prevents hangs on metadata locks
    - `keepAlive: true` + `keepAliveInitialDelayMillis: 10000` — detect broken TCP connections quickly (short-lived process)
    - `application_name: "proposal-executor"` — visible in `pg_stat_activity` for observability and debugging
  - `findExecutableProposals(client, nowSeconds)` → `Effect<Proposal[], InfraError>` where `Proposal = {id: string, spaceId: string, spaceAddress: string}`. Uses the detection SQL above. `CLOCK_SKEW_BUFFER` (60) is a hardcoded named constant. No artificial `LIMIT` — result set is bounded by reality.
- [ ] `detect.test.ts` — Test the SQL logic against known proposal states using a test database or by verifying the query structure.

### Phase 3: Execution

- [ ] `contracts.ts` — Minimal ABI subset (exactly 2 functions: `enter`, `addressToSpaceId`) + chain definitions (Geo Genesis mainnet/testnet, from `geo-cli/src/network.ts`) + governance constants (`PROPOSAL_EXECUTED` action hash, `EMPTY_SIGNATURE`). Note: these are **forked copies** from geo-cli — add comments referencing the source files and versions. Long-term consolidation path: extract a shared `@geo/protocol` package when a third consumer appears.
- [ ] `execute.ts` — Smart wallet setup, encoding helpers, and transaction submission. All functions return `Effect` values with tagged errors:
  - `createSmartWallet(config)` → `Effect<SmartWallet, InfraError>` — Creates Safe smart account + Pimlico client. Called once per run.
  - `verifyExecutorSetup(wallet)` → `Effect<void, InfraError>` — Calls `addressToSpaceId(safeAddress)` to confirm the executor's personal space exists. Fails with `InfraError` if not registered.
  - `executeProposal(proposal)` → `Effect<string, RevertError | InfraError>` — Constructs and sends the `enter(PROPOSAL_EXECUTED)` tx. Converts `proposal.spaceId` (UUID) to `daoSpaceId` (bytes16) via `uuidToBytes16()`, and `proposal.id` (UUID) to `proposalIdHex` (bytes16) the same way. Returns the tx hash on success. Classifies errors: on-chain reverts become `RevertError` (with `expected: true` for "already executed"), infrastructure failures become `InfraError`. Retry and timeout are composed externally in `index.ts` via `executeWithRetry()`: `Effect.timeout(30s)` + `Effect.retry({ schedule: exponential(1s) · recurs(2), while: InfraError })`. This keeps `executeProposal` pure and testable — retry policy is separate from execution logic.
  - Encoding helpers (pure functions): `uuidToBytes16(uuid)`, `padBytes16ToBytes32(bytes16)`, `encodeProposalExecutedData(proposalIdHex)`. These are critical for correctness — test against known on-chain values from successfully executed proposals.
- [ ] `execute.test.ts` — Tests for:
  - Encoding correctness against known values from geo-cli (use real proposal IDs, not synthetic)
  - Error classification (mock viem responses: revert vs. 429/timeout vs. success)

**Environment variables (all phases):**

| Variable | Required | Sensitive | Description |
|---|---|---|---|
| `DATABASE_URL` | Yes | Yes | PostgreSQL connection string (use read-only user) |
| `EXECUTOR_PRIVATE_KEY` | Yes | Yes | Private key for the EOA that owns the Safe smart account |
| `EXECUTOR_SPACE_ID` | Yes | No | Personal space ID of the executor (bytes16, 0x-prefixed — public on-chain data) |
| `PIMLICO_API_KEY` | Yes | Yes | Pimlico bundler/paymaster API key |
| `SPACE_REGISTRY_ADDRESS` | Yes | No | Space Registry contract address (0x-prefixed) |
| `RPC_URL` | Yes | No | Chain RPC endpoint |
| `CHAIN_ID` | Yes | No | Chain ID (80451 for mainnet, 19411 for testnet) |
| `SENTRY_DSN` | No | Yes | Optional Sentry error reporting DSN |

### Phase 4: Orchestration

- [ ] `index.ts` — Main entry point using Effect-TS for orchestration, concurrency, error handling, and structured logging:

```typescript
import { Effect, Data, Duration, Logger, Schedule } from "effect"

// No artificial concurrency limit — one fiber per space. Each UserOperation
// is an independent pipeline (gas estimate → paymaster → bundler → confirm)
// so N concurrent spaces ≠ N simultaneous RPC calls. Pimlico handles this.

// --- Tagged errors for type-safe error classification ---
class RevertError extends Data.TaggedError("RevertError")<{
  proposalId: string; message: string; expected: boolean; durationMs: number
}> {}

class InfraError extends Data.TaggedError("InfraError")<{
  proposalId: string; message: string; durationMs: number
}> {}

// --- Retry policy: exponential backoff, only for InfraError ---
const infraRetryPolicy = Schedule.compose(
  Schedule.exponential(Duration.seconds(1)),
  Schedule.recurs(2),
)

// --- Per-proposal execution with retry + timeout ---
const executeWithRetry = (proposal: Proposal) =>
  executeProposal(proposal).pipe(
    Effect.timeout(Duration.seconds(30)),
    Effect.retry({ schedule: infraRetryPolicy, while: (e) => e._tag === "InfraError" }),
  )

// --- Per-space: sequential execution, skip reverts, propagate infra errors ---
const executeSpaceProposals = (spaceId: string, proposals: Proposal[]) =>
  Effect.forEach(proposals, (proposal) =>
    executeWithRetry(proposal).pipe(
      Effect.tap((txHash) =>
        Effect.logInfo("proposal_executed").pipe(
          Effect.annotateLogs({ proposalId: proposal.id, spaceId, txHash })
        )
      ),
      Effect.catchTag("RevertError", (e) =>
        Effect.logInfo(e.expected ? "proposal_skip_expected" : "proposal_reverted").pipe(
          Effect.annotateLogs({ proposalId: proposal.id, spaceId, error: e.message, expected: e.expected, durationMs: e.durationMs }),
          Effect.as("skipped" as const),
        )
      ),
      // InfraError propagates — fail-fast aborts remaining proposals in this space
    ),
    { concurrency: 1 },
  )

// --- Main orchestration ---
const program = Effect.gen(function* () {
  const config = yield* parseConfig
  const db = yield* connectDb(config.databaseUrl)
  const wallet = yield* createSmartWallet(config)
  yield* verifyExecutorSetup(wallet)

  const runStart = Date.now()
  const nowSeconds = Math.floor(runStart / 1000)
  const proposals = yield* findExecutableProposals(db, nowSeconds)
  const bySpace = Map.groupBy(proposals, (p) => p.spaceId)

  yield* Effect.logInfo("run_start").pipe(
    Effect.annotateLogs({ proposalsFound: proposals.length, spaces: bySpace.size })
  )

  if (proposals.length === 0) {
    yield* Effect.logInfo("run_end").pipe(
      Effect.annotateLogs({ succeeded: 0, failed: 0, total: 0, durationMs: Date.now() - runStart })
    )
    return { succeeded: 0, failed: 0 }
  }

  // Execute spaces in parallel (capped at SPACE_CONCURRENCY); sequential
  // within each space. Each space is independent — an InfraError in one
  // space doesn't affect others. If Pimlico is truly down, every space
  // will independently exhaust retries and abort.
  const results = yield* Effect.forEach(
    [...bySpace.entries()],
    ([spaceId, spaceProposals]) =>
      executeSpaceProposals(spaceId, spaceProposals).pipe(
        Effect.map((outcomes) => ({
          spaceId,
          succeeded: outcomes.filter((r) => r !== "skipped").length,
          skipped: outcomes.filter((r) => r === "skipped").length,
        })),
        // Catch InfraError at the space level so one space failing
        // doesn't cancel the others via fail-fast
        Effect.catchTag("InfraError", (e) =>
          Effect.logError("space_aborted").pipe(
            Effect.annotateLogs({ spaceId, error: e.message, proposalId: e.proposalId }),
            Effect.as({ spaceId, succeeded: 0, skipped: 0, infraError: true as const }),
          )
        ),
      ),
    { concurrency: "unbounded" },
  )

  const succeeded = results.reduce((n, r) => n + r.succeeded, 0)
  const failed = results.filter((r) => "infraError" in r).length
  const skipped = results.reduce((n, r) => n + r.skipped, 0)

  yield* Effect.logInfo("run_end").pipe(
    Effect.annotateLogs({ succeeded, failed, skipped, total: proposals.length, spaces: bySpace.size, durationMs: Date.now() - runStart })
  )

  return { succeeded, failed }
})

// --- Entry point ---
const runId = crypto.randomUUID()

const main = program.pipe(
  Effect.annotateLogs({ runId }),
  Effect.withSpan("proposal-executor-run"),
  Effect.provide(Logger.json),
  Effect.catchAllDefect((defect) => {
    console.error(JSON.stringify({ event: "fatal", runId, error: String(defect) }))
    return Effect.succeed({ succeeded: 0, failed: 1 })
  }),
)

Effect.runPromise(main).then(({ succeeded, failed }) => {
  process.exit(failed > 0 && succeeded === 0 ? 1 : 0)
})
```

**Why Effect-TS:**
- **Retry + fail-fast** — `Effect.retry` with `Schedule.exponential` + `Schedule.recurs(2)` retries transient infra errors per-proposal. Reverts are never retried (gated on `e._tag === "InfraError"`). After retries are exhausted, `Effect.forEach` fail-fast aborts the remaining proposals in that space. No manual error budget counter or shared mutable state.
- **Concurrency** — `Effect.forEach` with `{ concurrency: "unbounded" }` for parallel-by-space (one fiber per space, no artificial cap). `{ concurrency: 1 }` for sequential-within-space. Replaces manual `Promise.allSettled` + chunking.
- **Error classification** — Tagged errors (`RevertError`, `InfraError`) with `catchTag` for type-safe routing. Reverts are skipped, infra errors are retried then propagated.
- **Timeout** — `Effect.timeout(Duration.seconds(30))` per proposal.
- **Logging** — `Effect.annotateLogs({ runId })` at the top propagates the run ID to all nested log calls. `Logger.json` outputs structured JSON. No manual `log()` wrapper needed.
- **No shared mutable state** — Results are collected from `Effect.forEach` return values and reduced. No `Ref`, no `aborted` flag, no manual counters.
- **Matches codebase** — The API uses Effect for error handling and tracing. `geo-cli` is entirely Effect-based. This follows established patterns.

- [ ] `index.test.ts` — Test orchestration logic with mocked `executeProposal` Effect: verify retry behavior (InfraError retried 2x then space aborted), reverts skipped without retry, exit codes (all fail → 1, partial → 0, none found → 0), independent space isolation (one space's InfraError doesn't affect others). Use `Effect.runPromise` in tests.
- [ ] Optional Sentry integration — If `SENTRY_DSN` is set, initialize Sentry before `Effect.runPromise`. Capture unhandled defects and set context (run ID, proposal counts).
- [ ] Exit code: 0 if at least one proposal succeeded or none were found. 1 only if every attempt failed (signals a systemic issue to K8s). Rationale: partial success indicates individual proposal issues (expected reverts), not systemic failure.

### Phase 5: Deployment

- [ ] `Dockerfile` — Multi-stage Bun build (pinned image, non-root user):

```dockerfile
FROM oven/bun:1.3.9 AS builder
WORKDIR /app
COPY package.json bun.lock* ./
RUN bun install --frozen-lockfile
COPY . .

FROM oven/bun:1.3.9
ENV NODE_ENV=production
RUN addgroup --system --gid 1001 geo && adduser --system --uid 1001 geo
WORKDIR /app
COPY --from=builder --chown=geo:geo /app .
USER geo
CMD ["bun", "run", "src/index.ts"]
```

- [ ] `deployment/namespace.yaml` — `proposal-executor` namespace
- [ ] `deployment/secrets.yaml.example` — Template documenting required secrets: `EXECUTOR_PRIVATE_KEY`, `DATABASE_URL`, `PIMLICO_API_KEY`. Real secrets managed externally (not committed).
- [ ] `deployment/cronjob.yaml`:

```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: proposal-executor
  namespace: proposal-executor
spec:
  schedule: "*/5 * * * *"
  concurrencyPolicy: Forbid
  successfulJobsHistoryLimit: 5
  failedJobsHistoryLimit: 5
  jobTemplate:
    spec:
      activeDeadlineSeconds: 290   # 10s gap before next 5-min CronJob invocation
      backoffLimit: 1           # Don't restart-loop on failure — try again in 5 min
      template:
        spec:
          restartPolicy: Never  # CronJob retries via next scheduled run, not restarts
          securityContext:
            runAsNonRoot: true
            runAsUser: 1001
          imagePullSecrets:
            - name: regcred
          containers:
            - name: proposal-executor
              image: registry.digitalocean.com/geo/proposal-executor:latest
              imagePullPolicy: Always
              env:
                # Secrets (sensitive — from K8s Secret)
                - name: DATABASE_URL
                  valueFrom:
                    secretKeyRef:
                      name: proposal-executor-credentials
                      key: DATABASE_URL
                - name: EXECUTOR_PRIVATE_KEY
                  valueFrom:
                    secretKeyRef:
                      name: proposal-executor-credentials
                      key: EXECUTOR_PRIVATE_KEY
                - name: PIMLICO_API_KEY
                  valueFrom:
                    secretKeyRef:
                      name: proposal-executor-credentials
                      key: PIMLICO_API_KEY
                # Config (non-sensitive — plain values)
                - name: EXECUTOR_SPACE_ID
                  value: ""     # Set per environment (public on-chain data, not a secret)
                - name: SPACE_REGISTRY_ADDRESS
                  value: ""     # Set per environment (public blockchain data, not a secret)
                - name: RPC_URL
                  value: ""     # Set per environment
                - name: CHAIN_ID
                  value: "80451"
                # Optional: Sentry error reporting
                - name: SENTRY_DSN
                  valueFrom:
                    secretKeyRef:
                      name: proposal-executor-credentials
                      key: SENTRY_DSN
                      optional: true
              resources:
                requests:
                  memory: "256Mi"
                  cpu: "250m"
                limits:
                  memory: "512Mi"
                  cpu: "500m"
```

**Environment-specific values:** `SPACE_REGISTRY_ADDRESS`, `RPC_URL`, `CHAIN_ID`, and Sentry DSN differ between staging (testnet: chain 19411) and production (mainnet: chain 80451). These are set per-cluster deployment, not per-manifest. One `cronjob.yaml` serves both environments — only the values change.

**Note on `DATABASE_URL`:** Should use a read-only PostgreSQL user/role. The executor only reads from the proposals table — write access is unnecessary and a security risk for a service that handles private keys.

## Acceptance Criteria

- [ ] CronJob runs every 5 minutes and exits cleanly when no EXECUTABLE proposals exist
- [ ] Slow-path proposals that meet quorum + threshold + voting ended are detected and executed on-chain
- [ ] Proposals within each space are executed in `created_at::bigint ASC` order (FIFO per space)
- [ ] Spaces are executed in parallel — proposals across different spaces have no ordering dependency
- [ ] On-chain reverts are logged and skipped — the next proposal is attempted
- [ ] Expected reverts (e.g., "already executed") logged at INFO, not ERROR
- [ ] Infrastructure errors (Pimlico, RPC) are retried per-proposal (2 retries, exponential backoff). If retries exhausted, remaining proposals in that space are aborted. Other spaces continue independently.
- [ ] Service fails fast on startup if: env vars are missing, DB is unreachable, executor's personal space is not registered
- [ ] **Structured JSON logs** — canonical `run_start`/`run_end` events with proposal counts, space count, succeeded, failed, and total run `durationMs`
- [ ] All log lines include a `runId` (UUID) for correlating events within a single CronJob invocation
- [ ] Per-proposal JSON logs include: proposal ID, space ID, outcome, tx hash (on success), and `durationMs`
- [ ] Unbounded space parallelism — one fiber per space, no artificial concurrency cap
- [ ] No shared mutable state — results collected from `Effect.forEach` return values, reduced at the end
- [ ] No artificial batch limit — all EXECUTABLE proposals fetched in one query, bounded by reality
- [ ] `activeDeadlineSeconds: 290` prevents runaway CronJob runs (10s gap before next 5-min invocation)
- [ ] `restartPolicy: Never` with `backoffLimit: 1` — no restart storms
- [ ] `concurrencyPolicy: Forbid` prevents overlapping runs
- [ ] `securityContext: runAsNonRoot` on the pod spec
- [ ] Gas is fully sponsored via Pimlico — no ETH balance required on the executor wallet
- [ ] Container runs as non-root user with pinned Bun image
- [ ] `DATABASE_URL` uses a read-only PostgreSQL user/role
- [ ] Tests cover: encoding against real on-chain values, error classification (RevertError vs InfraError tagging), retry behavior (InfraError retried 2x, RevertError never retried), space isolation, exit codes, cross-validation against `computeProposalStatus()`

## Dependencies & Risks

| Dependency | Risk | Mitigation |
|---|---|---|
| Pimlico sponsorship budget | Budget exhaustion stops all executions | Monitor Pimlico dashboard, set budget alerts |
| Executor personal space | Must be pre-registered or all txs revert | Startup verification + fail-fast |
| Space Registry ABI stability | ABI change breaks encoding | Pin contract version, monitor for upgrades |
| kg-indexer lag | Double-execution attempts in race window | Benign: reverts are caught and skipped |
| Mainnet Space Registry address | Not in geo-cli contracts.ts yet | Add to env var config, not hardcoded |
| Detection SQL is 3rd copy of status logic | Drift from `computeProposalStatus()` or `sqlIsExecutable()` | Add a cross-validation test that asserts executor SQL matches `computeProposalStatus()` for known proposal states. **Note:** the executor SQL is intentionally stricter — it adds a 60s clock skew buffer (`end_time + 60` vs `end_time`). The test must use `nowSeconds` values well past the end time so the buffer doesn't affect results, or explicitly account for the delta. Also assert the hardcoded `10000000` matches `RATIO_BASE` from `api/src/proposals/types.ts`. |
| geo-cli code fork | Encoding/ABI/chain defs diverge from geo-cli | Comment source+version in each forked file; long-term: shared `@geo/protocol` package |

## Open Questions

- **Gas funding:** With Pimlico sponsorship, the executor wallet doesn't need ETH. But who owns the Pimlico account and monitors its balance?
- **Monitoring/alerting:** Should we add a Prometheus metric or Sentry alert for proposals stuck in EXECUTABLE for >15 minutes? (Sentry DSN is now optional in the CronJob spec.)
- **Testnet Safe addresses:** The testnet uses custom Safe deployment addresses (defined in `geo-cli/src/wallet.ts:54-61`). These need to be replicated in the executor's staging config.
- **~~`enter(PROPOSAL_EXECUTED)` permission model~~** — **Resolved.** The protocol doc (`docs/protocol/dao-space.md:179`) explicitly states: "Anyone can call `enter()` with `PROPOSAL_EXECUTED` once criteria are met." No membership or editorship required. The executor's personal space just needs to be registered.

## References

### Internal
- Brainstorm: `docs/brainstorms/2026-03-02-proposal-auto-executor-brainstorm.md`
- Status computation: `api/src/proposals/status.ts` (pure function)
- SQL detection fragments: `api/src/proposals/queries.ts:286-345` (`sqlIsExecutable()`)
- Proposal types: `api/src/proposals/types.ts`
- Database schema: `api/src/services/storage/schema.ts:100-130` (proposals table)
- Scoring-service CronJob: `scoring-service/deployment/production/cronjob.yaml`
- Smart wallet pattern: `~/work/code/geo-cli/src/wallet.ts:191-261`
- Proposal execution pattern: `~/work/code/geo-cli/src/cli.ts:2235-2254`
- Governance encoding: `~/work/code/geo-cli/src/governance.ts`
- SpaceRegistry ABI: `~/work/code/geo-cli/src/contracts.ts:140-152`
- Chain definitions: `~/work/code/geo-cli/src/network.ts`
- Fast-path execution gap: `hermes-pipeline/docs/GOTCHAS.md`
- Gas sponsorship research: `docs/research/gas-sponsorship-eoa-identity.md`
- Protocol docs: `docs/protocol/space-registry.md`, `docs/protocol/dao-space.md`
