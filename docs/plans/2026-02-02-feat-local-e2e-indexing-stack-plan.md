---
title: "feat: Local E2E Development Environment for Indexing Stack"
type: feat
date: 2026-02-02
status: proposed
revision: 3
---

# Local E2E Development Environment for Indexing Stack

## Overview

Create a local development environment that allows developers to iterate on the full stack—from UserOperation submission through block production to Kafka message consumption—without depending on external blockchain, bundler, or Substreams services.

**Components:**
- **Anvil** — Local EVM chain
- **Alto bundler + Mock Paymaster** — ERC-4337 account abstraction
- **fireeth** — Block indexing via RPC Poller
- **Existing Kafka/PostgreSQL** — Message streaming and storage

## Problem Statement

### Current Pain Points

1. **External Dependencies**: Developers must connect to Pinax Substreams endpoints and testnet bundlers, introducing network latency and rate limits
2. **Debugging Complexity**: When something fails, it's hard to isolate whether it's local code, network, or external service
3. **Limited Control**: Can't replay transactions, reset chain state, or control block timing
4. **Mock Mode Limitations**: `USE_MOCK=true` simulates events but doesn't test the actual Substreams WASM modules or 4337 flow

## Proposed Solution

Extend `hermes/docker-compose.yaml` with local blockchain and ERC-4337 services:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              LOCAL (docker-compose)                          │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────┐    │
│  │                    ERC-4337 / Account Abstraction                   │    │
│  │                                                                     │    │
│  │  ┌─────────┐  ┌─────────┐  ┌──────────────┐  ┌───────────────┐    │    │
│  │  │  Anvil  │◄─│  Alto   │◄─│  EntryPoint  │◄─│ Safe Factory  │    │    │
│  │  │  (EVM)  │  │(Bundler)│  │   (v0.7)     │  │  + Singleton  │    │    │
│  │  └────┬────┘  └─────────┘  └──────────────┘  └───────────────┘    │    │
│  │       │            ▲              ▲                                │    │
│  │       │            │       ┌──────┴──────┐  ┌────────────────┐    │    │
│  │       │            │       │    Mock     │  │ Space Registry │    │    │
│  │       │            │       │  Paymaster  │  │  + Geo Proto   │    │    │
│  │       │            │       └─────────────┘  └────────────────┘    │    │
│  └───────┼────────────┼───────────────────────────────────────────────┘    │
│          │ RPC Polling                                                      │
│          ▼                                                                  │
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │                         Indexing Pipeline                             │  │
│  │                                                                       │  │
│  │  ┌─────────┐  ┌─────────────────┐  ┌─────────┐  ┌─────────────────┐ │  │
│  │  │ fireeth │─▶│ hermes-pipeline │─▶│  Kafka  │─▶│ hermes-ipfs-    │ │  │
│  │  │ (Poller)│  │                 │  │         │  │ cache           │ │  │
│  │  └─────────┘  └─────────────────┘  └─────────┘  └────────┬────────┘ │  │
│  │                                                           │          │  │
│  │                                                           ▼          │  │
│  │                                                    ┌────────────────┐│  │
│  │                                                    │   PostgreSQL   ││  │
│  │                                                    └────────────────┘│  │
│  └──────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Technical Approach

### Architecture Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Local EVM | Anvil (Foundry) | Fast, widely used, good fireeth compatibility |
| ERC-4337 Bundler | Alto (Pimlico) | Open-source, has localhost config, Docker images |
| Paymaster | Mock Verifying Paymaster | Sponsors all UserOps, no signature verification |
| Contract Deployment | Pimlico mock-contract-deployer | Deploys EntryPoint, Safe contracts automatically |
| Block Indexing | fireeth RPC Poller | Uses same tooling as production |
| Geo Contracts | Foundry script | Space Registry + Geo protocol |

### Critical Technical Issue: Space Registry Address

**Problem**: The Space Registry address is hardcoded in `hermes-substream/src/lib.rs:17-20` as a compile-time constant:

```rust
const SPACE_REGISTRY_ADDRESS: [u8; 20] = [
    0xb0, 0x16, 0x83, 0xb2, 0xf0, 0xd3, 0x8d, 0x43, ...
];
```

Local Anvil will deploy the Space Registry at a **different address**.

**Solution Options** (pick one during implementation):

1. **Deterministic CREATE2 deployment** — Deploy Space Registry to the same address on Anvil using CREATE2 with a known salt.

2. **Substreams params** — Make the address configurable via Substreams module parameters (WASM modules don't have env access).

3. **Foundry fork mode** — Fork testnet state into Anvil so contracts are at the same addresses. Simplest but requires network access on startup.

**Recommendation**: Option 1 (CREATE2) for full local control, or Option 3 (fork) for fastest implementation.

## Implementation Plan

### Phase 1: Blockchain + ERC-4337 Stack

**Objective**: Get a local blockchain with working account abstraction.

**Tasks:**

1. **Add Anvil service**
   ```yaml
   anvil:
     image: ghcr.io/foundry-rs/foundry:nightly
     ports: ["127.0.0.1:8545:8545"]
     entrypoint: ["anvil", "--host", "0.0.0.0", "--block-time", "1", "--chain-id", "31337"]
     healthcheck:
       test: ["CMD-SHELL", "curl -sf http://localhost:8545 -X POST -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"method\":\"eth_chainId\",\"params\":[],\"id\":1}'"]
       interval: 5s
       timeout: 3s
       retries: 10
   ```

2. **Add ERC-4337 contract deployer**
   ```yaml
   contract-deployer:
     image: ghcr.io/pimlicolabs/mock-contract-deployer:main
     environment:
       - ANVIL_RPC=http://anvil:8545
     depends_on:
       anvil:
         condition: service_healthy
   ```
   This deploys: EntryPoint v0.6/v0.7/v0.8, Safe contracts (Factory, Singleton, 4337Module), simulation contracts.

3. **Add Alto bundler**
   ```yaml
   alto:
     image: ghcr.io/pimlicolabs/alto:latest
     ports: ["127.0.0.1:4337:4337"]
     environment:
       - ALTO_RPC_URL=http://anvil:8545
       - ALTO_ENTRYPOINTS=0x5ff137d4b0fdcd49dca30c7cf57e578a026d2789,0x0000000071727De22E5E9d8BAf0edAc6f37da032
       - ALTO_EXECUTOR_PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
       - ALTO_UTILITY_PRIVATE_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
       - ALTO_MIN_BALANCE=0
       - ALTO_SAFE_MODE=false
     depends_on:
       contract-deployer:
         condition: service_completed_successfully
   ```
   Note: Private keys are Anvil's default test accounts—safe to commit.

4. **Add Mock Paymaster**
   ```yaml
   mock-paymaster:
     image: ghcr.io/pimlicolabs/mock-verifying-paymaster:main
     ports: ["127.0.0.1:3000:3000"]
     environment:
       - ALTO_RPC=http://alto:4337
       - ANVIL_RPC=http://anvil:8545
     depends_on:
       alto:
         condition: service_started
   ```

5. **Deploy Geo protocol contracts**
   - Create `hermes/scripts/deploy-geo-contracts.sh`
   - Deploy Space Registry (with deterministic address or fork)
   - Deploy other Geo protocol contracts
   - Output addresses for verification

**Deliverables:**
- [ ] Anvil, contract-deployer, alto, mock-paymaster services in docker-compose
- [ ] `hermes/scripts/deploy-geo-contracts.sh`
- [ ] Smoke test: UserOp submission → transaction on Anvil

**Success Criteria:**
- All 4337 services start and report healthy
- Can submit UserOperation via Alto
- Mock Paymaster sponsors the transaction
- Transaction executes on Anvil

### Phase 2: Indexing Pipeline

**Objective**: Index blocks from Anvil through fireeth into Kafka.

**Tasks:**

1. **Resolve Space Registry address problem**
   - Implement CREATE2 deterministic deployment OR fork mode
   - Verify hermes-substream can filter events at the deployed address

2. **Add fireeth RPC Poller service**
   ```yaml
   fireeth:
     image: ghcr.io/streamingfast/firehose-ethereum:latest
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
   ```
   Note: Exact command syntax needs verification—see Open Questions.

3. **Update hermes-pipeline for local mode**
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

4. **End-to-end test**
   - Submit UserOperation via Alto
   - Verify transaction indexed by fireeth
   - Verify event processed by Substreams
   - Verify message in Kafka
   - Verify hermes-ipfs-cache writes to PostgreSQL

**Deliverables:**
- [ ] fireeth service in docker-compose
- [ ] Updated hermes-pipeline environment config
- [ ] End-to-end smoke test script

**Success Criteria:**
- UserOp submission → Kafka message in < 10 seconds
- Space Registry events correctly filtered
- hermes-ipfs-cache processes messages

### Phase 3: Developer Experience

**Objective**: Make the local stack easy to use.

**Tasks:**

1. **Test data generation**
   - `hermes/scripts/seed-test-data.sh`
   - Create sample spaces, edits, votes via UserOperations
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
   - Document bundler/paymaster endpoints for frontend configuration

4. **Troubleshooting guide** (inline in README)
   - "Bundler returns AA21 didn't pay prefund" → Check executor account balance
   - "fireeth stuck at block 0" → Check Anvil block production
   - "Kafka messages not appearing" → Check topic auto-creation

**Deliverables:**
- [ ] `hermes/scripts/seed-test-data.sh`
- [ ] Updated `hermes/Makefile`
- [ ] Updated `hermes/README.md`

**Success Criteria:**
- New developer can start stack in < 5 minutes
- `make local-test` passes
- Documentation covers common issues

## Open Questions

| Question | Impact | When to Resolve |
|----------|--------|-----------------|
| fireeth RPC Poller exact command syntax | Blocks fireeth service | Start of Phase 2 |
| CREATE2 vs fork for deterministic addresses | Blocks Geo contract deployment | Start of Phase 2 |
| Does fireeth need Substreams tier services, or is it all-in-one? | Affects docker-compose complexity | Start of Phase 2 |
| Alto config: env vars vs config file? | Minor—affects docker-compose | Phase 1 |
| Frontend config for local bundler/paymaster | Needed for full E2E testing | Phase 3 |

## Acceptance Criteria

### Must Have

- [ ] All services start with `docker-compose up`
- [ ] UserOperations submitted to Alto execute on Anvil
- [ ] Mock Paymaster sponsors transactions
- [ ] fireeth indexes blocks from Anvil
- [ ] hermes-pipeline produces Kafka messages
- [ ] hermes-ipfs-cache writes to PostgreSQL
- [ ] Smoke test script passes

### Nice to Have

- [ ] Test data generation script
- [ ] Makefile targets
- [ ] README documentation

## Service Startup Order

```
Kafka + PostgreSQL (existing, healthy)
         │
         ▼
      Anvil (healthy)
         │
         ▼
  Contract Deployer (completes) ──────────┐
         │                                 │
         ▼                                 ▼
      Alto ◄────────────────────── Mock Paymaster
         │
         ▼
  Geo Contract Deployment (completes)
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

## Required Tools

| Tool | Required | Notes |
|------|----------|-------|
| Docker & Docker Compose | Yes | v2.0+ with health checks |
| Foundry (`cast`, `forge`) | Yes | For Geo contract deployment and testing |
| 6GB+ RAM | Yes | For running all services |

## Risk Analysis

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| fireeth doesn't work with Anvil | Medium | High | Test in first 2 days of Phase 2; fallback to fork mode |
| Space Registry address mismatch | High | High | Resolve before Phase 2 proceeds |
| Alto/Paymaster Docker images have breaking changes | Low | Medium | Pin to specific image tags |
| Substreams WASM incompatible with local fireeth | Medium | Medium | May need to rebuild `.spkg` |

## References

### Internal

- Existing docker-compose: `hermes/docker-compose.yaml`
- Space Registry address: `hermes-substream/src/lib.rs:17-20`
- Mock events pattern: `hermes-relay/src/source/mock_events.rs`
- Brainstorm: `docs/brainstorms/2026-02-02-local-e2e-indexing-brainstorm.md`

### External

- fireeth documentation: https://firehose.streamingfast.io/firehose-setup/ethereum
- Alto bundler: https://docs.pimlico.io/infra/bundler
- Pimlico Docker testing: https://docs.pimlico.io/guides/how-to/testing/docker
- Foundry Book (Anvil): https://book.getfoundry.sh/anvil/

---

## Appendix: Docker Compose Services

```yaml
# hermes/docker-compose.local.yaml

services:
  # === Blockchain Layer ===
  
  anvil:
    image: ghcr.io/foundry-rs/foundry:nightly
    ports: ["127.0.0.1:8545:8545"]
    entrypoint: ["anvil", "--host", "0.0.0.0", "--block-time", "1", "--chain-id", "31337"]
    healthcheck:
      test: ["CMD-SHELL", "curl -sf http://localhost:8545 -X POST -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"method\":\"eth_chainId\",\"params\":[],\"id\":1}'"]
      interval: 5s
      timeout: 3s
      retries: 10

  # === ERC-4337 Layer ===

  contract-deployer:
    image: ghcr.io/pimlicolabs/mock-contract-deployer:main
    environment:
      - ANVIL_RPC=http://anvil:8545
    depends_on:
      anvil:
        condition: service_healthy

  alto:
    image: ghcr.io/pimlicolabs/alto:latest
    ports: ["127.0.0.1:4337:4337"]
    environment:
      - ALTO_RPC_URL=http://anvil:8545
      - ALTO_ENTRYPOINTS=0x5ff137d4b0fdcd49dca30c7cf57e578a026d2789,0x0000000071727De22E5E9d8BAf0edAc6f37da032
      - ALTO_EXECUTOR_PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
      - ALTO_UTILITY_PRIVATE_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
      - ALTO_MIN_BALANCE=0
      - ALTO_SAFE_MODE=false
    depends_on:
      contract-deployer:
        condition: service_completed_successfully

  mock-paymaster:
    image: ghcr.io/pimlicolabs/mock-verifying-paymaster:main
    ports: ["127.0.0.1:3000:3000"]
    environment:
      - ALTO_RPC=http://alto:4337
      - ANVIL_RPC=http://anvil:8545
    depends_on:
      alto:
        condition: service_started

  # === Indexing Layer ===

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

## Appendix: Endpoint Reference

| Service | URL | Purpose |
|---------|-----|---------|
| Anvil | http://localhost:8545 | Ethereum JSON-RPC |
| Alto Bundler | http://localhost:4337 | ERC-4337 Bundler RPC |
| Mock Paymaster | http://localhost:3000 | Paymaster + Bundler proxy |
| fireeth gRPC | localhost:9000 | Firehose streaming |
| Kafka | localhost:9092 | Message queue |
| Kafka UI | http://localhost:8080 | Kafka management |
| PostgreSQL | localhost:5432 | IPFS cache database |
