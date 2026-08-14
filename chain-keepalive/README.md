# Chain Keep-Alive

A K8s CronJob that prevents ZeroDev's bundler relayer for chain 55516 from wedging after the chain goes idle for a while.

## Why

Every UserOperation on this chain goes through ZeroDev's bundler (sponsored gas via EIP-7702 Kernel accounts). Scanning the chain's full history (block 1 → current tip) found the bundler's relayer has repeatedly stopped calling the EntryPoint contract after the chain sits idle for a while — gaps ranging from ~11 hours up to 18 days, recurring since June. Once wedged, every sponsored UserOperation is accepted (`200 OK` from `eth_sendUserOperation`) but never gets a receipt, and stays that way until *some* new block appears on-chain — during the Aug 2026 incident (GEO-2549/GEO-2550), the relayer resumed calling the EntryPoint exactly one second after an unrelated, manually-sent transaction produced a fresh block.

This job runs every 5 minutes and, only if the chain has actually been idle for a while, sends a trivial real-gas self-transfer directly to the chain — deliberately bypassing the ZeroDev bundler entirely, since a keep-alive that depends on the thing that's wedging can't unwedge it.

This is a stopgap, not a fix — the underlying bundler reliability issue has been reported to ZeroDev.

## How it works

1. Fetch the latest block; compute how long it's been since that block's timestamp.
2. If under the idle threshold (default 10 minutes — comfortably below the shortest wedge-inducing gap observed historically), exit — nothing to do.
3. Otherwise, send a 0-value self-transfer using the current chain fee estimate (floored at 1 gwei — the value verified during the incident to mine instantly) and wait for its receipt.

## Quick start

```bash
# Install
bun install

# Run tests (pure logic only — the RPC/chain-touching parts are verified manually against the live chain)
bun test

# Lint + typecheck
bun run lint && bun run typecheck

# Run locally
RPC_URL=... KEEPALIVE_PRIVATE_KEY=... bun run start
```

## Config

| Env var | Required | Default | Notes |
|---|---|---|---|
| `RPC_URL` | yes | — | Plain chain RPC, not the ZeroDev bundler endpoint |
| `KEEPALIVE_PRIVATE_KEY` | yes | — | Needs a real (non-zero) balance to pay gas |
| `CHAIN_ID` | no | `55516` | |
| `IDLE_THRESHOLD_MS` | no | `600000` (10 min) | |
