# Proposal Auto-Executor

A K8s CronJob that automatically executes slow-path governance proposals on-chain. Runs every 5 minutes, detects proposals in EXECUTABLE status, and calls `enter(PROPOSAL_EXECUTED)` on the Space Registry contract via a gas-sponsored Safe smart account.

## Why

Slow-path proposals require an explicit on-chain call after voting ends and quorum + threshold conditions are met. Without this service, proposals sit in EXECUTABLE state indefinitely until someone manually executes them. Fast-path proposals don't have this problem — they auto-execute in the contract when the decisive YES vote lands.

## How it works

1. **Detect** — Query PostgreSQL for slow-path proposals where voting has ended, quorum is met, and threshold is reached
2. **Execute** — For each proposal (FIFO per space), call `enter(PROPOSAL_EXECUTED)` via Safe + Pimlico paymaster
3. **Skip on failure** — On-chain reverts are logged and skipped; infra errors are retried then the space is aborted
4. **Exit** — Process completes, container exits

Parallel across spaces, sequential within each space (proposals may have ordering dependencies).

## Quick start

```bash
# Install
bun install

# Run tests
bun test

# Lint + typecheck
bun run lint && bun run typecheck

# Run locally (requires all env vars — see .env.example)
bun run start
```

## Configuration

| Variable | Sensitive | Description |
|---|---|---|
| `DATABASE_URL` | Yes | PostgreSQL connection string (read-only user) |
| `EXECUTOR_PRIVATE_KEY` | Yes | EOA private key that owns the Safe smart account |
| `PIMLICO_API_KEY` | Yes | Pimlico bundler/paymaster API key |
| `RPC_URL` | Yes | Chain RPC endpoint (may contain API keys) |
| `EXECUTOR_SPACE_ID` | No | Executor's personal space ID (bytes16, 0x-prefixed) |
| `SPACE_REGISTRY_ADDRESS` | No | Space Registry contract address |
| `CHAIN_ID` | No | `80451` (mainnet) or `19411` (testnet) |

## Documentation

- **[RUNBOOK.md](./RUNBOOK.md)** — Operations: setup, logs, troubleshooting, suspend/resume, key rotation, monitoring
- **[ARCHITECTURE.md](./ARCHITECTURE.md)** — Design: concurrency model, error classification, detection SQL, forked code, performance model

## Deployment

Deploys to the `knowledge` namespace via GitHub Actions:

- **Production**: push to `main` → `proposal-executor-deploy.yml`

The manifest lives in `deployment/production/`.

There is no staging deployment. The service shares its chain, contracts, and bot
identities across environments, so a "staging" instance would issue real on-chain
transactions as the same actor as production — it isn't sandboxed. A pre-prod
deployment would require genuinely isolated infrastructure (a separate chain/RPC/
registry, its own database, and throwaway bot keys that are not editors anywhere).
