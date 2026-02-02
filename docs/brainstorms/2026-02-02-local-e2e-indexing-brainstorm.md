---
date: 2026-02-02
topic: local-e2e-indexing
---

# Local E2E Setup for Indexing Stack

## What We're Building

A fully local end-to-end development environment for the indexing stack, allowing developers to iterate on `hermes-pipeline` and `hermes-ipfs-cache` without depending on external blockchain/bundler services.

### Local Components
- **Anvil** (Foundry) as the local EVM chain
- **Alto** (Pimlico's bundler) for ERC-4337 UserOperation handling
- **Safe Smart Account contracts** (Factory, Singleton, 4337 Module)
- **Mock Paymaster** - real contract that sponsors all txs (no signature verification)
- **ERC-4337 EntryPoint** (canonical)
- **fireeth** (RPC Poller mode) to produce Substreams-compatible blocks from Anvil
- **Substreams tier1/tier2** to execute `hermes-substream` WASM modules
- **Local Kafka** for message streaming
- **Local PostgreSQL** for IPFS cache storage
- **Space Registry + Geo protocol contracts**

### External Services (kept as-is)
- **Privy** - Authentication + embedded EOA wallet (users sign in via email)
- **Public IPFS gateway** - content already exists on IPFS

### User Flow
1. User authenticates via Privy (production) → gets EOA
2. Safe Smart Account is created/derived from EOA on local Anvil
3. UserOperations are submitted to local Alto bundler
4. Mock Paymaster sponsors gas
5. Transactions execute on Anvil
6. fireeth indexes blocks → Substreams → hermes-pipeline → Kafka
7. Developers can debug the full indexing flow locally

## Why This Approach

### Approaches Considered

1. **fireeth + Anvil (RPC Poller)** - Full Substreams stack with local chain
2. **Modified hermes-relay** - Direct Anvil polling bypassing Substreams
3. **Record & Replay** - Use captured testnet blocks

### Why fireeth + Anvil

- **Authenticity**: Uses the same `fireeth` binary and Substreams infrastructure as production
- **Sufficient Data Model**: `hermes-substream` only uses event logs (no internal calls, balance changes), so Base Blocks from RPC Poller are sufficient
- **Official Tooling**: Leverages maintained StreamingFast tooling rather than custom implementations
- **Future-Proof**: When Amp migration completes (PR #288), the local setup approach remains similar

### Trade-offs Accepted

- **Infrastructure Overhead**: Running multiple fireeth processes (merger, tier1, tier2) adds complexity
- **Setup Complexity**: Requires deploying contracts to Anvil and seeding test data
- **Documentation Gap**: RPC Poller mode documentation is sparse, may require experimentation

## Key Decisions

1. **Use Anvil over Hardhat/Ganache**: Anvil is fast, feature-rich, and commonly used. Integrates well with Foundry tooling for contract deployment.

2. **RPC Poller over Firehose Geth**: Avoids the need for a modified Geth node. RPC Poller produces "Base Blocks" which are sufficient since we only use event logs.

3. **Keep IPFS external**: Content referenced in events already exists on public IPFS. Running a local IPFS node adds complexity without significant benefit for local dev.

4. **Focus on Substreams (not Amp)**: While Amp migration (PR #288) is in progress, this local setup targets the current production architecture. Can adapt for Amp later.

5. **Docker Compose orchestration**: Extend existing `hermes/docker-compose.yaml` with Anvil, fireeth, and Substreams tier services.

6. **Use Alto bundler for ERC-4337**: Pimlico's open-source Alto bundler runs locally with Anvil. It has a `config.localhost.json` and helper scripts designed for local development.

7. **Safe Smart Accounts**: Deploy Safe's ERC-4337 compatible contracts (SafeProxyFactory, Safe Singleton, Safe4337Module, etc.) to match production architecture where Privy EOAs own Safe accounts.

8. **Mock Paymaster with real integration**: Deploy a real paymaster contract that exercises the full ERC-4337 flow, but with simplified logic (sponsors all transactions without signature verification). This ensures the paymaster integration is tested, just with permissive sponsorship.

9. **Keep Privy external**: Users authenticate via production Privy. The EOA from Privy is used to derive/own Safe accounts on local Anvil. This avoids mocking auth complexity.

## Open Questions

1. **fireeth RPC Poller configuration**: Need to research exact flags/config for running fireeth in RPC Poller mode against Anvil.

2. **Block production timing**: How does fireeth handle Anvil's instant mining? May need to configure Anvil's block time.

3. **Contract deployment**: Should contracts be deployed via Foundry scripts or a dedicated initialization service?

4. **Test data generation**: How will developers seed realistic test scenarios (spaces, edits, votes)?

5. **Startup ordering**: What's the correct service dependency order for docker-compose?

6. **Bundler choice confirmation**: Alto (Pimlico) is recommended for local dev, but need to confirm this aligns with production bundler patterns. If migrating to Openfort, check what bundler they recommend/use.

7. **Safe contract versions**: Which versions of Safe contracts are used in production? Need to deploy matching versions locally.

8. **Privy + local chain integration**: How does the frontend configure Privy to work with local Anvil instead of testnet? May need environment-based chain configuration.

## Architecture Diagram

```
  ┌──────────────┐
  │    Privy     │  (external - production auth)
  │  Auth + EOA  │
  └──────┬───────┘
         │ EOA signs UserOps
         ▼
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                              docker-compose (local)                                  │
│                                                                                      │
│  ┌─────────────────────────────────────────────────────────────────────────────┐   │
│  │                         ERC-4337 / Account Abstraction                       │   │
│  │                                                                              │   │
│  │   ┌───────────┐    ┌─────────────┐    ┌──────────────┐    ┌────────────┐   │   │
│  │   │   Alto    │───▶│  EntryPoint │───▶│ Safe Account │───▶│   Anvil    │   │   │
│  │   │ (bundler) │    │  (v0.6/v0.7)│    │   + Module   │    │   (EVM)    │   │   │
│  │   └───────────┘    └─────────────┘    └──────────────┘    └─────┬──────┘   │   │
│  │         │                                    ▲                   │          │   │
│  │         │              ┌─────────────────────┘                   │          │   │
│  │         ▼              │                                         │          │   │
│  │   ┌────────────┐  ┌────┴───────┐                                │          │   │
│  │   │   Mock     │  │   Safe     │  ◀── Contracts deployed        │          │   │
│  │   │ Paymaster  │  │  Factory   │      on Anvil startup          │          │   │
│  │   └────────────┘  └────────────┘                                │          │   │
│  └─────────────────────────────────────────────────────────────────┼──────────┘   │
│                                                                     │              │
│  ┌─────────────────────────────────────────────────────────────────┼──────────┐   │
│  │                         Indexing Pipeline                        │          │   │
│  │                                                                  ▼          │   │
│  │   ┌─────────┐    ┌──────────┐    ┌────────────────────┐   ┌─────────┐     │   │
│  │   │ fireeth │───▶│Substreams│───▶│  hermes-pipeline   │──▶│  Kafka  │     │   │
│  │   │ (Poller)│    │ Tier1/2  │    │                    │   │         │     │   │
│  │   └─────────┘    └──────────┘    └────────────────────┘   └─────────┘     │   │
│  │                                                                            │   │
│  │   ┌─────────────┐    ┌─────────────────────┐                              │   │
│  │   │  PostgreSQL │◀───│  hermes-ipfs-cache  │───────────────────────────────┼───┼──▶ Public IPFS
│  │   │ (ipfs_cache)│    │                     │                              │   │     (external)
│  │   └─────────────┘    └─────────────────────┘                              │   │
│  └───────────────────────────────────────────────────────────────────────────┘   │
│                                                                                   │
│  Contracts on Anvil: EntryPoint, Safe Factory, Safe Singleton, Safe4337Module,   │
│                      Mock Paymaster, Space Registry, Geo Protocol contracts      │
└───────────────────────────────────────────────────────────────────────────────────┘
```

## Components Required

### Infrastructure

| Component | Source | Notes |
|-----------|--------|-------|
| Anvil | `foundry` | `anvil --block-time 2` for realistic timing |
| Alto bundler | `pimlicolabs/alto` | TypeScript ERC-4337 bundler, has `config.localhost.json` |
| fireeth | `brew install streamingfast/tap/firehose-ethereum` | RPC Poller mode |
| Substreams tiers | Part of fireeth | `fireeth start substreams-tier1,substreams-tier2` |
| Kafka | Apache Kafka | Already in docker-compose |
| PostgreSQL | postgres:16 | Already in docker-compose |

### Contracts (deployed to Anvil on startup)

| Contract | Source | Notes |
|----------|--------|-------|
| ERC-4337 EntryPoint | `eth-infinitism/account-abstraction` | Canonical v0.6 or v0.7 |
| SafeProxyFactory | `safe-global/safe-smart-account` | Creates Safe proxy instances |
| Safe Singleton | `safe-global/safe-smart-account` | Safe implementation contract |
| Safe4337Module | `safe-global/safe-modules` | ERC-4337 compatibility for Safe |
| Mock Paymaster | Custom or simplified | Sponsors all txs, no sig verification |
| Space Registry | Geo protocol contracts | Core indexing target |
| Other Geo contracts | Geo protocol | Any additional protocol contracts |

### Indexing Services

| Component | Source | Notes |
|-----------|--------|-------|
| hermes-substream | `hermes-substream/` crate | Compile to WASM, package as .spkg |
| hermes-pipeline | `hermes-pipeline/` crate | No changes needed |
| hermes-ipfs-cache | `hermes-ipfs-cache/` crate | No changes needed |

## Next Steps

1. Research fireeth RPC Poller mode configuration
2. Create docker-compose services for Anvil + fireeth
3. Set up contract deployment script for Anvil
4. Create test data generation scripts
5. Document the local development workflow

Run `/workflows:plan` when ready to implement.
