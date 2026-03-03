# Brainstorm: Proposal Auto-Executor Service

**Date:** 2026-03-02
**Status:** Ready for planning

## What We're Building

A standalone service that detects slow-path governance proposals that have become EXECUTABLE and automatically executes them on-chain. Today, slow-path proposals sit in an EXECUTABLE state indefinitely until someone manually triggers `enter(PROPOSAL_EXECUTED)` on the Space Registry contract. This service closes that gap.

**Scope:**
- Slow-path proposals only (fast-path is already auto-executed by the smart contract inline)
- All spaces (no opt-in/opt-out mechanism)
- Runs as a K8s CronJob every ~5 minutes
- Uses Safe smart wallet + Pimlico gas sponsorship (same pattern as geo-cli)

## Why This Approach

**K8s CronJob over a long-running service** because:
- The 5-minute cadence maps perfectly to CronJob scheduling
- No long-lived process state to manage (crash recovery, health checks, leader election)
- Matches the existing scoring-service CronJob pattern in the codebase
- YAGNI — a persistent polling service is over-engineered for this cadence

**Standalone service over co-location** (in the API or kg-indexer) because:
- On-chain transaction submission is new infrastructure — it shouldn't be coupled to the read-serving API or the Rust indexer
- Clean separation: the API serves reads, the executor handles writes
- Independent scaling and failure isolation

**SQL-driven detection** because:
- The existing `sqlIsExecutable()` SQL fragments in `queries.ts` already encode the status logic in SQL
- One query finds all eligible proposals — no need to load into TypeScript and recompute
- Database is the source of truth for vote counts (maintained by the tally worker)

## Key Decisions

1. **Slow-path only** — Fast-path proposals are auto-executed by the contract when the decisive YES vote lands. The auto-executor targets the actual gap: slow-path proposals where voting has ended, quorum is met, and threshold is reached.

2. **FIFO execution order** — Execute proposals in `created_at ASC` order to respect governance chronology. Older proposals execute first.

3. **Skip-on-failure** — If an on-chain execution reverts or fails, log the error, skip that proposal, and continue with the next. Failed proposals will be retried on the next CronJob run. No retry-with-backoff within a single run.

4. **Smart wallet with gas sponsorship** — Use the same pattern as `geo-cli`: a private key (from env var) creates an EOA that owns a Safe smart account via `permissionless` + `viem`. Gas is sponsored through Pimlico as bundler/paymaster. This means the executor wallet doesn't need to hold ETH for gas. Stack: `permissionless` (Safe smart account + Pimlico client) + `viem` (public client, EOA account).

5. **Detection query** — Query for proposals where: `voting_mode = 'Slow'`, `executed_at IS NULL`, voting has ended (`end_time < now`), and the status computation yields EXECUTABLE (quorum met + threshold reached). Reuse/adapt the existing SQL fragments.

6. **K8s CronJob with `concurrencyPolicy: Forbid`** — Prevents overlapping runs if execution takes longer than 5 minutes. Matches the scoring-service pattern.

## Open Questions

- **Gas funding:** Who funds the executor wallet? How do we monitor gas balance and alert when low?
- **Monitoring/alerting:** Should we alert when proposals have been EXECUTABLE for more than X minutes without successful execution?
- **Rate limiting:** Should we cap the number of proposals executed per run to avoid gas spikes?
- **Contract ABI:** Need to confirm the exact function signature and encoding for `enter(PROPOSAL_EXECUTED)` with the proposal ID.
- **Space address resolution:** The executor needs the space's contract address to call `enter()`. Confirm this is available via the `spaces.address` column.
- **Pimlico API key:** Use the existing shared key or provision a dedicated one for the executor service?
- **Smart account address permissions:** Does the executor's smart account need any special role/permission in the Space Registry to call `enter(PROPOSAL_EXECUTED)`, or can any address trigger execution?

## Next Steps

→ `/workflows:plan` for implementation details
