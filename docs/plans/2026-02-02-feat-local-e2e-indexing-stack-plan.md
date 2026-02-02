---
title: "feat: Local E2E Development Environment for Indexing Stack"
type: feat
date: 2026-02-02
status: proposed
revision: 2
---

# Local E2E Development Environment for Indexing Stack

## Overview

Create a minimal local development environment that allows developers to iterate on `hermes-pipeline` and `hermes-ipfs-cache` without depending on external Substreams endpoints. The MVP focuses on the **indexing pipeline only**—Anvil + fireeth + existing Kafka/PostgreSQL—tested with simple EOA transactions.

**Key Insight**: Indexing doesn't care *how* transactions were submitted. The pipeline sees blocks with events, not UserOperations or Safe accounts. We can test the full indexing path with `cast send` and defer the ERC-4337 stack to a separate effort.

## Problem Statement

### Current Pain Points

1. **External Dependencies**: Developers must connect to Pinax Substreams endpoints, introducing network latency and rate limits
2. **Debugging Complexity**: When indexing fails, it's hard to isolate whether it's local code, network, or external service
3. **Limited Control**: Can't replay transactions, reset chain state, or control block timing
4. **Mock Mode Limitations**: `USE_MOCK=true` simulates events but doesn't test the actual Substreams WASM modules

### Why MVP Scope

The original plan bundled two independent problems:
1. **Local indexing pipeline** (fireeth + Substreams) — for testing block indexing
2. **Local ERC-4337 stack** (Alto, Safe, Paymaster) — for testing transaction submission

These are separate concerns. A developer debugging `hermes-pipeline` doesn't need a working bundler—they need blocks with the right events. **This plan focuses on #1 only.**

## Proposed Solution

Add Anvil and fireeth to the existing docker-compose stack. Test with simple EOA transactions via `cast send`.

```
┌─────────────────────────────────────────────────────────────────────┐
│                         LOCAL (docker-compose)                       │
│                                                                      │
│  ┌─────────┐     ┌─────────┐     ┌─────────────────┐   ┌─────────┐ │
│  │  Anvil  │────▶│ fireeth │────▶│ hermes-pipeline │──▶│  Kafka  │ │
│  │  (EVM)  │     │ (Poller)│     │                 │   │         │ │
│  └─────────┘     └─────────┘     └─────────────────┘   └────┬────┘ │
│       ▲                                                      │      │
│       │                                                      ▼      │
│  cast send                                          ┌───────────────┐│
│  (test txs)                                         │hermes-ipfs-   ││
│                                                     │cache          ││
│  Space Registry                                     └───────┬───────┘│
│  (deployed on startup)                                      ▼       │
│                                                     ┌────────────────┐
│                                                     │   PostgreSQL   │
│                                                     └────────────────┘
└─────────────────────────────────────────────────────────────────────┘
```

## Technical Approach

### Architecture Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Local EVM | Anvil (Foundry) | Fast, widely used, good fireeth compatibility |
| Block Indexing | fireeth RPC Poller | Uses same tooling as production |
| Transaction Submission | Simple EOA via `cast` | Indexing doesn't care about 4337 |
| Contract Deployment | Foundry `cast` + shell script | Simple, no extra tooling |
| ERC-4337 Stack | Deferred | Separate plan if/when needed |

### Critical Technical Issue: Space Registry Address

**Problem**: The Space Registry address is hardcoded in `hermes-substream/src/lib.rs:17-20` as a compile-time constant:

```rust
const SPACE_REGISTRY_ADDRESS: [u8; 20] = [
    0xb0, 0x16, 0x83, 0xb2, 0xf0, 0xd3, 0x8d, 0x43, ...
];
```

Local Anvil will deploy the Space Registry at a **different address**.

**Solution Options** (pick one during implementation):

1. **Deterministic CREATE2 deployment** — Deploy Space Registry to the same address on Anvil using CREATE2 with a known salt. Requires deploying a CREATE2 factory first.

2. **Substreams params** — Make the address configurable via Substreams module parameters (not environment variables—WASM modules don't have env access).

3. **Foundry fork mode** — Fork testnet state into Anvil so contracts are at the same addresses. Simplest but requires network access on startup.

**Recommendation**: Option 1 (CREATE2) for full local control, or Option 3 (fork) for fastest implementation.

## Implementation Plan

### Phase 1: Minimal Local Indexing (5-7 days)

**Objective**: Get transactions from Anvil into Kafka via the Substreams pipeline.

**Tasks:**

1. **Resolve Space Registry address problem**
   - Decide on CREATE2 vs fork approach
   - If CREATE2: Create deployment script with deterministic address
   - If fork: Configure Anvil to fork from testnet

2. **Add Anvil service to docker-compose**
   ```yaml
   anvil:
     image: ghcr.io/foundry-rs/foundry:nightly
     ports: ["127.0.0.1:8545:8545"]  # Localhost only
     entrypoint: ["anvil", "--host", "0.0.0.0", "--block-time", "1", "--chain-id", "31337"]
     healthcheck:
       test: ["CMD-SHELL", "curl -sf http://localhost:8545 -X POST -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"method\":\"eth_chainId\",\"params\":[],\"id\":1}'"]
       interval: 5s
       timeout: 3s
       retries: 10
   ```

3. **Add fireeth RPC Poller service**
   - Verify correct command syntax (research needed—see Open Questions)
   - Add gRPC healthcheck
   ```yaml
   fireeth:
     image: ghcr.io/streamingfast/firehose-ethereum:latest
     command: [TBD - verify syntax]
     depends_on:
       anvil:
         condition: service_healthy
     healthcheck:
       test: ["CMD-SHELL", "grpc_health_probe -addr=localhost:9000 || exit 1"]
       interval: 5s
       timeout: 5s
       retries: 10
   ```

4. **Deploy Space Registry to Anvil**
   - Create `hermes/scripts/deploy-contracts.sh`
   - Use `cast` for deployment
   - Output deployed address for verification

5. **Update hermes-pipeline environment**
   ```yaml
   hermes-pipeline:
     environment:
       - SUBSTREAMS_ENDPOINT=fireeth:9000
       - SUBSTREAMS_API_TOKEN=local-dev-token
     depends_on:
       fireeth:
         condition: service_healthy
       kafka:
         condition: service_healthy
   ```

6. **Create smoke test script**
   - `hermes/scripts/smoke-test.sh`
   - Submit test transaction via `cast send`
   - Verify event appears in Kafka
   - Return pass/fail

**Deliverables:**
- [ ] `hermes/docker-compose.local.yaml` (override file)
- [ ] `hermes/scripts/deploy-contracts.sh`
- [ ] `hermes/scripts/smoke-test.sh`
- [ ] Updated `hermes/README.md` with local dev section
- [ ] Makefile target: `make local`

**Success Criteria:**
- `docker-compose -f docker-compose.yaml -f docker-compose.local.yaml up` starts stack
- `cast send <space-registry> "createSpace(...)"` produces Kafka message
- Smoke test passes

### Phase 2: Developer Experience (3-4 days)

**Objective**: Make the local stack pleasant to use.

**Tasks:**

1. **Test data generation**
   - `hermes/scripts/seed-test-data.sh`
   - Create sample spaces, edits, votes
   - Use addresses from `mock_events.rs` for consistency

2. **Makefile targets**
   ```makefile
   local:          # Start local stack
   local-logs:     # Tail all logs
   local-reset:    # Wipe state and restart
   local-test:     # Run smoke test
   ```

3. **Documentation**
   - Add "Local Development" section to `hermes/README.md`
   - Include: setup, common commands, troubleshooting
   - Keep it to one file, not four

4. **Troubleshooting guide** (inline in README)
   - "fireeth stuck at block 0" → Check Anvil block production
   - "Kafka messages not appearing" → Check topic auto-creation
   - "Substreams connection failed" → Check gRPC port

**Deliverables:**
- [ ] `hermes/scripts/seed-test-data.sh`
- [ ] Updated `hermes/Makefile`
- [ ] Updated `hermes/README.md`

**Success Criteria:**
- New developer can start stack in < 5 minutes (following README)
- `make local-test` passes
- `make local-reset` cleanly restarts

## What's NOT In Scope (Deferred)

These are explicitly out of scope for this plan. Create separate plans if needed:

| Item | Reason for Deferral |
|------|---------------------|
| Alto bundler | Not needed for indexing tests |
| Mock Paymaster | Not needed for indexing tests |
| Safe Smart Accounts | Not needed for indexing tests |
| ERC-4337 EntryPoint | Not needed for indexing tests |
| Privy integration | Use `cast` with test accounts |
| Loki + Grafana | `docker-compose logs` is sufficient |
| CI integration | Get it working locally first |
| State persistence | Just restart fresh |
| Hot reload | Rebuild containers as needed |

## Open Questions

These must be resolved before or during implementation:

| Question | Impact | When to Resolve |
|----------|--------|-----------------|
| fireeth RPC Poller exact command syntax | Blocks fireeth service | Start of Phase 1 |
| CREATE2 vs fork for deterministic addresses | Blocks contract deployment | Start of Phase 1 |
| Does fireeth need Substreams tier services, or is it all-in-one in `--dev` mode? | Affects docker-compose | Start of Phase 1 |
| What block time works best with fireeth polling? | May need tuning | During Phase 1 |

## Acceptance Criteria

### Must Have (MVP)

- [ ] Stack starts with `docker-compose up`
- [ ] Transactions on Anvil produce Kafka messages
- [ ] Space Registry events are correctly filtered by Substreams
- [ ] hermes-pipeline and hermes-ipfs-cache work with local stack
- [ ] Smoke test script passes

### Nice to Have

- [ ] Test data generation script
- [ ] Makefile targets for common operations
- [ ] README documentation

### Explicitly Not Required

- [ ] ERC-4337 support
- [ ] Production-like performance
- [ ] Offline operation
- [ ] Resource usage optimization

## Dependencies

### Service Startup Order

```
Kafka + PostgreSQL (existing, healthy)
         │
         ▼
      Anvil (healthy)
         │
         ▼
  Contract Deployment (completes)
         │
         ▼
      fireeth (healthy)
         │
         ▼
   hermes-pipeline
         │
         ▼
   hermes-ipfs-cache
```

### Required Tools

| Tool | Required | Notes |
|------|----------|-------|
| Docker & Docker Compose | Yes | v2.0+ |
| Foundry (`cast`) | Yes | For deploying contracts and testing |
| 4GB+ RAM | Yes | Reduced from original 8GB |

## Risk Analysis

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| fireeth doesn't work with Anvil | Medium | High | Test in first 2 days; fallback to fork mode |
| Space Registry address mismatch | High | High | Resolve in Phase 1 Task 1 before proceeding |
| Substreams WASM module incompatible with local fireeth | Medium | Medium | May need to rebuild `.spkg` with different params |

## Effort Estimate

| Phase | Effort | Total |
|-------|--------|-------|
| Phase 1: Minimal Local Indexing | 5-7 days | 5-7 days |
| Phase 2: Developer Experience | 3-4 days | 8-11 days |

**Compare to original plan**: 11-17 days → 8-11 days (30-35% reduction)

## References

### Internal

- Existing docker-compose: `hermes/docker-compose.yaml`
- Space Registry address: `hermes-substream/src/lib.rs:17-20`
- Mock events pattern: `hermes-relay/src/source/mock_events.rs`
- Brainstorm: `docs/brainstorms/2026-02-02-local-e2e-indexing-brainstorm.md`

### External

- fireeth documentation: https://firehose.streamingfast.io/firehose-setup/ethereum
- Foundry Book (Anvil): https://book.getfoundry.sh/anvil/

---

## Appendix: Docker Compose Services (MVP)

```yaml
# hermes/docker-compose.local.yaml

services:
  anvil:
    image: ghcr.io/foundry-rs/foundry:nightly
    ports: ["127.0.0.1:8545:8545"]
    entrypoint: ["anvil", "--host", "0.0.0.0", "--block-time", "1", "--chain-id", "31337"]
    healthcheck:
      test: ["CMD-SHELL", "curl -sf http://localhost:8545 -X POST -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"method\":\"eth_chainId\",\"params\":[],\"id\":1}'"]
      interval: 5s
      timeout: 3s
      retries: 10

  # Note: Contract deployment runs as a one-shot script, not a service
  # Use: ./scripts/deploy-contracts.sh after anvil is healthy

  fireeth:
    image: ghcr.io/streamingfast/firehose-ethereum:latest
    # TODO: Verify exact command syntax
    command: >
      start rpc-poller
      --rpc-endpoint=http://anvil:8545
      --first-streamable-block=0
    depends_on:
      anvil:
        condition: service_healthy
    ports:
      - "9000:9000"
    healthcheck:
      test: ["CMD-SHELL", "grpc_health_probe -addr=localhost:9000 || exit 1"]
      interval: 5s
      timeout: 5s
      retries: 10

  hermes-pipeline:
    environment:
      - SUBSTREAMS_ENDPOINT=fireeth:9000
      - SUBSTREAMS_API_TOKEN=local-dev-token
    depends_on:
      fireeth:
        condition: service_healthy
      kafka:
        condition: service_healthy

  hermes-ipfs-cache:
    depends_on:
      kafka:
        condition: service_healthy
      ipfs-cache-postgres:
        condition: service_healthy
```

## Appendix: Future Work (Separate Plans)

If ERC-4337 local testing is needed, create a separate plan covering:

- Alto bundler service
- Mock Paymaster service
- ERC-4337 EntryPoint deployment
- Safe contract deployment (Factory, Singleton, 4337Module)
- Privy + local chain integration

This keeps concerns separated and allows teams to adopt what they need.
