# Proposal Auto-Executor — Architecture & Design Decisions

> For operational procedures, troubleshooting, and monitoring, see [RUNBOOK.md](./RUNBOOK.md).

## File Structure

```
proposal-executor/
├── src/
│   ├── index.ts        # Effect orchestration, config parsing, entry point
│   ├── detect.ts       # DB connection + detection SQL query
│   ├── execute.ts      # Smart wallet, encoding helpers, on-chain execution
│   ├── contracts.ts    # ABI subset, chain defs, tagged errors, governance constants
│   └── telemetry.ts    # OTel tracing + Sentry error tracking (adapted from api/)
├── tests/
│   ├── detect.test.ts  # RATIO_BASE cross-validation, SQL structure, Proposal shape
│   ├── execute.test.ts # Encoding correctness, constant validation, error classification
│   └── index.test.ts   # Tagged error discrimination, exit code logic, concurrency model
├── deployment/         # Both environments deploy into the 'knowledge' namespace
│   ├── staging/        # Testnet (chain 19411) manifests
│   │   ├── cronjob.yaml
│   │   └── secrets.yaml.example
│   └── production/     # Mainnet (chain 80451) manifests
│       ├── cronjob.yaml
│       └── secrets.yaml.example
├── Dockerfile
├── package.json
├── tsconfig.json
└── biome.json
```

### Why 4 Source Files, Not 8

- `config.ts` → folded into `index.ts` as `parseConfig()` (~15 lines)
- `encoding.ts` → folded into `execute.ts` (only called there, ~20 lines)
- `chain.ts` → folded into `contracts.ts` (one chain definition literal per network)
- `db.ts` → folded into `detect.ts` (5 lines of `pg.Client` connect/query/end)

Each file has enough substance (~30–100 lines) to justify its existence without artificial splitting.

## Concurrency Model

**Parallel across spaces, sequential within each space.**

- `Effect.forEach` with `{ concurrency: "unbounded" }` across spaces — one fiber per space
- `Effect.forEach` with `{ concurrency: 1 }` within each space — proposals may have ordering dependencies (e.g., "add member" then "grant editor")

### Why Unbounded Parallelism

Each UserOperation is an independent pipeline (gas estimation → paymaster → bundler → confirmation). N concurrent spaces ≠ N simultaneous RPC calls — the pipeline stages interleave naturally. Pimlico handles concurrent UserOperations fine.

The CronJob must finish within 290s to avoid colliding with the next 5-minute schedule. An artificial concurrency cap (e.g., 10 spaces at a time) risks not finishing in time. With unbounded concurrency, the bottleneck is the slowest single space's sequential chain, not the total proposal count.

**Maximum expected cardinality:** Currently bounded by the number of active DAO spaces on-chain. If this grows to hundreds of concurrent spaces, the service may need a concurrency cap and resource limit adjustments. Monitor `run_end` → `spaces` count to track growth.

## Error Classification

The executor classifies errors into two tagged types using a 3-tier strategy:

### Tagged Error Types

```typescript
class RevertError extends Data.TaggedError("RevertError")<{
  proposalId: string; message: string; expected: boolean; durationMs: number
}>

class InfraError extends Data.TaggedError("InfraError")<{
  proposalId: string; message: string; durationMs: number
}>
```

### Classification Tiers

1. **Structured error name** (most reliable) — viem exposes `ContractFunctionRevertedError`, `ContractFunctionExecutionError`, `CallExecutionError`
2. **Cause chain walking** — `UserOperationExecutionError` wraps the revert as `.cause`
3. **String fallback** (least reliable) — checks for "revert", "execution reverted", "CALL_EXCEPTION", "UserOperation reverted"

Error names are logged with `[errorName]` prefix for observability — use these to tighten classification patterns over time.

### Retry Policy

```typescript
const infraRetryPolicy = Schedule.compose(
  Schedule.exponential(Duration.seconds(1)),
  Schedule.recurs(2),
)
```

- Gated on `_tag === "InfraError"` — reverts are never retried
- Per-proposal scope — each proposal gets its own retry budget
- Fail-fast within space — if retries are exhausted, `InfraError` propagates and `Effect.forEach` aborts remaining proposals in that space
- Space isolation — other spaces continue independently

If Pimlico is truly down, every space will independently exhaust retries on its first proposal and abort. The net effect is a natural kill switch without shared mutable state.

## Detection SQL

### The 3rd Copy Problem

The executable-proposal detection logic exists in three places:

1. `api/src/proposals/status.ts` — `computeProposalStatus()` (pure TypeScript)
2. `api/src/proposals/queries.ts` — `sqlIsExecutable()` (SQL fragments)
3. `proposal-executor/src/detect.ts` — Detection SQL

The executor's copy is intentionally stricter — it adds a 60s clock-skew buffer not present in the other copies.

**What's cross-validated by tests:**
- `RATIO_BASE` constant (10,000,000) matches `api/src/proposals/types.ts`

**What's NOT automatically validated (manual check on API changes):**
- Threshold formula shape: `(RATIO_BASE - threshold) * yes > threshold * no`
- `voting_mode` enum value: `'Slow'`
- Clock-skew buffer delta (executor's `+ 60` vs API's bare `end_time`)

Long-term mitigation: extract shared types/logic into `@geo/protocol` when a third consumer appears.

### SQL Implementation Notes

- **`created_at` is stored as text** — Unix timestamp in the proposals table. `ORDER BY created_at::bigint ASC` ensures numeric ordering. Without the `::bigint` cast, string ordering would be incorrect for timestamps of different lengths.
- **`pg` doesn't support JS BigInt** — `nowSeconds` is passed as `Number()` and cast to `::bigint` in SQL. All Unix timestamps fit safely in JS number precision.
- **No `LIMIT`** — Result set is bounded by reality (proposals simultaneously in EXECUTABLE state). The DB read is fast; throughput is bounded by on-chain submission, not the query.

## Smart Wallet

Pattern forked from `geo-cli/src/wallet.ts:191-261`:

```
Private Key (env var) → EOA → Safe Smart Account → Smart Account Client (with Pimlico paymaster)
```

The Safe smart account address is deterministic from the owner EOA. The same private key always produces the same smart account address.

### Executor Personal Space Prerequisite

`enter()` requires `_fromSpaceId` — the executor's registered personal space ID. The contract verifies `msg.sender == spaceIdToAddress[_fromSpaceId]`. Since the Safe smart account IS the `msg.sender`, and the personal space is registered to the Safe's address, the signature parameter is ignored (`"0x"`).

### Testnet Safe Addresses

The testnet uses custom Safe deployment addresses (defined in `contracts.ts`, forked from `geo-cli/src/wallet.ts:54-61`). These are selected via `config.chainId === 19411`.

## Race Condition: Double Execution

After the executor submits `enter(PROPOSAL_EXECUTED)` on-chain, the proposal's `executed_at` remains NULL until the kg-indexer processes the event (seconds to minutes). During this window, the next CronJob run re-detects and re-submits. The contract reverts ("already executed").

**Decision: Let it revert.**
- Gas is sponsored — reverts are free
- No on-chain view function to check execution state
- `RevertError` with `expected: true` logged at INFO, not ERROR

**Alternatives considered and rejected:**
- **Check on-chain before executing** — No view function exists. Storage layout is fragile/undocumented.
- **Local tracking column** (`execution_submitted_at`) — Adds a DB write (breaks read-only boundary), introduces new failure modes (tx submitted but failed → stale flag → proposal never retried).
- **Check API/subgraph** — Same kg-indexer data source, same lag. Doesn't help.

If revert noise becomes a problem at scale, the best future option is a local tracking column with a TTL (auto-clear after 10 minutes).

## Config Validation

`parseConfig` in `index.ts` validates all inputs at startup:

| Variable | Validation |
|---|---|
| `EXECUTOR_PRIVATE_KEY` | 0x-prefixed, 64 hex chars (regex). Auto-prefixes `0x` if missing. |
| `EXECUTOR_SPACE_ID` | 0x-prefixed bytes16 — 34 chars total (regex). |
| `SPACE_REGISTRY_ADDRESS` | Valid Ethereum address (viem `getAddress()` checksum). |
| `RPC_URL` | Non-empty, starts with `http://` or `https://`. |
| `CHAIN_ID` | Must be `80451` or `19411` (exhaustive check). |

Sensitive values use `Config.redacted` (matches API + geo-cli patterns). All validation failures are fail-fast `InfraError` with descriptive messages.

## Telemetry

Adapted from `api/src/services/telemetry.ts` for a short-lived CronJob.

**Architecture:** OTel spans → `SentrySpanProcessor` → Sentry. Same as the API.

**Differences from the API's telemetry:**
- No HTTP/GraphQL middleware (batch job, not a server)
- Always emits to console AND Sentry (API uses Sentry breadcrumbs as the sole output when Sentry is enabled — we dual-write because CronJob pods rely on `kubectl logs` / log aggregators as the primary observability channel)
- Exports `flush` Effect called before `process.exit()` — short-lived processes must flush or pending events are lost
- Initializes eagerly at module load (same as API)

**OTel Spans:**

| Span | Scope |
|---|---|
| `proposal-executor.run` | Entire CronJob invocation (top-level) |
| `proposal-executor.detect` | PostgreSQL detection query |
| `proposal-executor.execute-proposal` | Per-proposal execution (attributes: `proposalId`, `spaceId`) |

**Effect Logger routing:**
- `ERROR` / `FATAL` → `Sentry.captureMessage` (creates Sentry issues)
- `INFO` / `WARN` / `DEBUG` → `Sentry.addBreadcrumb` (context for subsequent errors)
- All levels also emit structured JSON to console

**Graceful degradation:** When `SENTRY_DSN` is not set, `telemetry.ts` logs `[TELEMETRY] Sentry disabled` and falls back to console-only. No spans are exported. The service runs identically otherwise.

## Timeout Layering

```
K8s activeDeadlineSeconds (290s) — SIGKILL
  └── Effect top-level timeout (270s) — graceful, 20s margin for finalizers
       └── Per-proposal timeout (30s) — prevents hung UserOperations
            └── DB timeouts (5-30s) — fail-fast on connection/query issues
```

The 20s gap between 270s and 290s ensures `Effect.addFinalizer` (DB disconnect) runs and a structured `run_failed` log is emitted before K8s kills the pod.

### DB Connection Settings

Chosen for a batch CronJob with `activeDeadlineSeconds: 290`:

| Setting | Value | Rationale |
|---|---|---|
| `connectionTimeoutMillis` | 5s | Fail fast if DB/PgBouncer unreachable |
| `statement_timeout` | 30s | Cap on detection query (normally <100ms) |
| `idle_in_transaction_session_timeout` | 60s | Safety net against leaked transactions |
| `lock_timeout` | 5s | Prevent hangs on metadata locks (reads only) |
| `keepAlive` | true | Detect broken TCP connections |
| `keepAliveInitialDelayMillis` | 10s | Start probing quickly (short-lived process) |
| `application_name` | `"proposal-executor"` | Visible in `pg_stat_activity` |

## Security Considerations

### Credential Handling

- **Private key + Pimlico API key + DATABASE_URL** — `Config.redacted` prevents accidental logging/serialization
- **DB connection string sanitization** — `connectDb` strips `postgresql://...` URLs from error messages via regex
- **Detection query error sanitization** — `findExecutableProposals` applies the same regex sanitization
- **Pimlico API key in error messages** — `executeProposal` strips `apikey=...` from error messages as defense in depth (the bundler URL contains the key)

### Pimlico API Key in Bundler URL

The Pimlico API key appears in the bundler URL (`https://api.pimlico.io/v2/{chainId}/rpc?apikey={key}`). The key is handled via `Config.redacted` in config parsing, but it's embedded in the URL string passed to `http()` transport. Error messages are sanitized to strip `apikey=` parameters. The key has limited scope (bundler/paymaster only, not account access).

### K8s Security Context

```yaml
securityContext:
  runAsNonRoot: true
  runAsUser: 1001
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities:
    drop: ["ALL"]
```

### enter() Permission Model

`enter()` with `PROPOSAL_EXECUTED` is **permissionless** — anyone can call it once criteria are met (confirmed in `docs/protocol/dao-space.md:179`). No membership or editorship required. The executor's personal space just needs to be registered.

## Forked Code from geo-cli

The executor forks code from `geo-cli` into two files:

| Source (geo-cli) | Destination | What |
|---|---|---|
| `src/wallet.ts:191-261` | `execute.ts` | Smart wallet creation (`createSmartWallet`) |
| `src/wallet.ts:54-61` | `contracts.ts` | Testnet Safe deployment addresses |
| `src/contracts.ts:125-152` | `contracts.ts` | SpaceRegistry ABI subset (`enter`, `addressToSpaceId`) |
| `src/network.ts` | `contracts.ts` | Chain definitions (mainnet 80451, testnet 19411) |
| `src/governance.ts` | `execute.ts` | Encoding helpers (`uuidToBytes16`, `padBytes16ToBytes32`, `encodeProposalExecutedData`) |
| `src/governance.ts` | `contracts.ts` | Governance constants (`PROPOSAL_EXECUTED_ACTION`, `EMPTY_SIGNATURE`) |

Each forked section has source comments referencing the origin file and version. If geo-cli updates these, the executor may drift.

**Long-term plan:** Extract a shared `@geo/protocol` package when a third consumer appears.

**Drift detection:** Not yet automated. When updating geo-cli wallet/governance/contract code, manually check the executor's copies. A future CI check could diff the forked sections.

## Dependencies

Version-matched to geo-cli to minimize divergence. See `package.json` for current versions.

## `Map.groupBy` and ES2024

The `tsconfig.json` targets ES2024 lib for `Map.groupBy`. Bun 1.3.9 supports this natively. If upgrading Bun or changing TypeScript targets, verify `Map.groupBy` availability.

## Performance Model

Each proposal execution involves a full UserOperation pipeline: gas estimation → paymaster signature → bundler submission → on-chain confirmation. Typical latency: **~3–8 seconds** per proposal.

| Scenario | Proposals | Estimated time | Fits in 270s? |
|---|---|---|---|
| Sequential (1 space) | 30 happy-path (~8s each) | ~240s | Yes, tight |
| Sequential (1 space) | 10 with retries (~30s each) | ~300s | No — hits timeout |
| Parallel (10 spaces × 3 each) | 30 total | ~24s | Easily |
| Parallel (50 spaces × 1 each) | 50 total | ~8s | Easily |

The parallel case dominates in practice — proposals are spread across many spaces. A single space with 20+ simultaneous executable proposals is unlikely due to staggered voting periods.
