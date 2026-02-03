# Running a Self-Hosted Node for an Arbitrum Orbit L3 on Base

**Date:** 2026-02-03  
**Status:** Research  
**Stack:** Arbitrum Orbit L3 settling on Base (L2)

---

## Overview

This document covers the operational requirements for running your own node infrastructure for a custom Arbitrum Orbit L3 chain that settles on Base. Running your own nodes eliminates rate limiting from third-party RPC providers and gives you full control over your infrastructure.

## Architecture Summary

```
┌─────────────────────────────────────────────────────────────────┐
│                        Your L3 Chain                            │
│                    (Arbitrum Orbit Nitro)                       │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ Batches posted to SequencerInbox
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                         Base (L2)                               │
│                    Parent Chain for L3                          │
│           RPC needed: --parent-chain.connection.url             │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ Base settles to Ethereum
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                     Ethereum Mainnet (L1)                       │
│          (Only needed if L3 uses blobs via Base)                │
└─────────────────────────────────────────────────────────────────┘
```

## Node Types

### 1. Full Node (Reader Node)

A full node syncs the chain and serves RPC requests. This is what you need to replace Conduit's managed RPCs.

**Purpose:**
- Serve RPC requests without rate limits
- Validate chain state independently
- Provide redundancy/backup

### 2. Archive Node

Same as a full node but retains all historical state (doesn't prune).

**Purpose:**
- Historical queries (trace calls, old state lookups)
- Block explorers
- Analytics

### 3. Sequencer Node

The sequencer orders transactions and produces batches. Conduit runs this for managed chains.

**Purpose:**
- Transaction ordering
- Batch posting to parent chain
- Only one active sequencer per chain

### 4. Validator/Staker Node

Validates chain assertions and can challenge invalid state roots.

**Purpose:**
- Security (fraud proofs)
- Watchtower mode (default) logs disagreements
- Active staker mode participates in challenges

---

## Hardware Requirements

### Full Node (Non-Archive)

| Resource | Minimum | Recommended |
|----------|---------|-------------|
| RAM | 16 GB | 64 GB |
| CPU | 4 cores | 8 cores (fast single-core performance critical) |
| Storage | 500 GB NVMe SSD | 1+ TB NVMe SSD |
| Network | 100 Mbps | 1 Gbps |

**Notes:**
- Single-core performance matters most. If falling behind and one core is at 100%, upgrade CPU.
- Storage grows with chain traffic over time.
- NVMe SSDs with locally attached drives strongly recommended.
- For high RPC traffic, increase RAM and CPU cores proportionally.

### Archive Node

Same as full node but significantly more storage:
- Storage scales with full chain history
- Plan for 2-4 TB+ depending on chain activity

### Data Availability Server (DAS) - AnyTrust Only

| Resource | Minimum |
|----------|---------|
| CPU | 1 core |
| RAM | 1 GB |
| Storage | Scales with retention policy |

**Notes:**
- DAS is lightweight; most work is storage I/O
- CDN (Cloudflare, Fastly, CloudFront) is **mandatory** for public REST endpoints
- Without CDN, you're exposed to DoS attacks

---

## Running a Full Node

### Prerequisites

1. **Parent Chain RPC** (Base) - You need an RPC endpoint for Base
   - Self-host a Base node, OR
   - Use a provider (Alchemy, Infura, QuickNode)
   - Conduit provides Base RPC (rate limited)

2. **Chain Info JSON** - Provided by Conduit/chain owner, contains:
   - Chain ID
   - Contract addresses (bridge, inbox, rollup, etc.)
   - Chain configuration

3. **Sequencer Feed URL** - WebSocket endpoint for real-time transaction feed
   - Get from Conduit dashboard: Settings → General → Run a Node

4. **Sequencer RPC Endpoint** - For forwarding transactions

### Docker Command

```bash
docker run --rm -it \
  -v /data/arbitrum:/home/user/.arbitrum \
  -p 0.0.0.0:8547:8547 \
  -p 0.0.0.0:8548:8548 \
  offchainlabs/nitro-node:v3.9.4-7f582c3 \
  --parent-chain.connection.url=<BASE_RPC_URL> \
  --chain.info-json='<CHAIN_INFO_JSON>' \
  --chain.name=<YOUR_CHAIN_NAME> \
  --node.feed.input.url=<SEQUENCER_FEED_WSS_URL> \
  --execution.forwarding-target=<SEQUENCER_RPC_URL> \
  --http.api=net,web3,eth \
  --http.corsdomain=* \
  --http.addr=0.0.0.0 \
  --http.vhosts=*
```

### Key Parameters

| Parameter | Description |
|-----------|-------------|
| `--parent-chain.connection.url` | Base RPC endpoint |
| `--chain.info-json` | JSON blob with chain configuration |
| `--chain.name` | Must match name in chain.info-json |
| `--node.feed.input.url` | Sequencer feed WebSocket URL |
| `--execution.forwarding-target` | Sequencer RPC for tx forwarding |
| `--http.api` | Enabled RPC methods (add `debug` for tracing) |

### Optional Parameters

| Parameter | Description |
|-----------|-------------|
| `--execution.caching.archive` | Enable archive mode (retain all state) |
| `--init.prune` | Prune before starting (`minimal`, `full`, `validator`) |
| `--node.staker.enable=false` | Disable watchtower mode (saves some overhead) |
| `--execution.rpc.evm-timeout` | Timeout for eth_call (default 5s) |
| `--execution.rpc.gas-cap` | Gas cap for eth_call/estimateGas |

### Important Ports

| Protocol | Default Port |
|----------|-------------|
| RPC/HTTP | 8547 |
| RPC/WebSocket | 8548 |
| Sequencer Feed | 9642 |

---

## Getting Chain Configuration from Conduit

### Enable External Nodes

1. Log into [Conduit App](https://app.conduit.xyz)
2. Go to your deployment dashboard
3. Settings → General
4. Enable "Enable external nodes"
5. Click "Run a Node" button to get:
   - Sequencer Feed Relay Endpoint (WebSocket URL)
   - Chain info JSON

### Chain Info JSON Example

```json
[{
  "chain-id": 94692861356,
  "parent-chain-id": 8453,
  "chain-name": "My L3 Chain",
  "chain-config": {
    "chainId": 94692861356,
    "homesteadBlock": 0,
    "arbitrum": {
      "EnableArbOS": true,
      "AllowDebugPrecompiles": false,
      "DataAvailabilityCommittee": false,
      "InitialArbOSVersion": 10,
      "InitialChainOwner": "0x..."
    }
  },
  "rollup": {
    "bridge": "0x...",
    "inbox": "0x...",
    "sequencer-inbox": "0x...",
    "rollup": "0x...",
    "validator-utils": "0x...",
    "validator-wallet-creator": "0x...",
    "deployed-at": 1764099
  }
}]
```

---

## AnyTrust vs Rollup Mode

Your L3 can operate in two modes:

### Rollup Mode
- All transaction data posted to Base (parent chain)
- Higher data costs, stronger security guarantees
- No additional infrastructure needed

### AnyTrust Mode
- Transaction data stored by Data Availability Committee (DAC)
- Only data hashes (DACerts) posted to Base
- Lower costs, requires running/trusting DAC members
- Need to run Data Availability Servers (DAS)

**Check your chain config:** Look for `DataAvailabilityCommittee` in chain-config:
- `false` = Rollup mode
- `true` = AnyTrust mode (need DAS configuration)

### AnyTrust Node Configuration

If your chain uses AnyTrust, add these parameters:

```bash
--node.data-availability.enable \
--node.data-availability.rest-aggregator.enable \
--node.data-availability.rest-aggregator.urls="<DAS_REST_ENDPOINT_1>,<DAS_REST_ENDPOINT_2>"
```

Or use an online URL list:
```bash
--node.data-availability.rest-aggregator.online-url-list="<URL_TO_DAS_LIST>"
```

---

## Running a Data Availability Server (AnyTrust Only)

If you're a DAC member or want to run your own DAS:

### Generate BLS Keypair

```bash
docker run -v $(pwd)/bls_keys:/data/keys --entrypoint datool \
  offchainlabs/nitro-node:v3.9.4-7f582c3 keygen --dir /data/keys
```

### Run DAS

```bash
docker run --rm -it \
  -v /data/das:/home/user/data \
  -p 9876:9876 \
  -p 9877:9877 \
  offchainlabs/nitro-node:v3.9.4-7f582c3 daserver \
  --data-availability.parent-chain-node-url="<BASE_RPC_URL>" \
  --data-availability.sequencer-inbox-address="<SEQUENCER_INBOX_ADDRESS>" \
  --data-availability.key.key-dir=/home/user/data/keys \
  --enable-rpc \
  --rpc-addr='0.0.0.0' \
  --enable-rest \
  --rest-addr='0.0.0.0' \
  --data-availability.local-cache.enable \
  --data-availability.local-file-storage.enable \
  --data-availability.local-file-storage.data-dir=/home/user/data/das-data
```

### DAS Storage Options

| Backend | Use Case |
|---------|----------|
| AWS S3 | Production, scalable, managed |
| Local Files | Simple, self-contained |
| Google Cloud Storage | Experimental |

### Mirror DAS

For public REST traffic, run a Mirror DAS behind a CDN:
- Doesn't need BLS keys
- Only serves REST requests (no RPC)
- Syncs from main DAS via REST aggregator
- Protects main DAS from DoS

---

## Parent Chain RPC Options

For your L3 node, you need a Base RPC. Options:

### 1. Conduit Managed (Rate Limited)

Use Conduit's Base RPC - subject to rate limits discussed earlier.

### 2. Third-Party Providers

| Provider | Notes |
|----------|-------|
| Alchemy | Good reliability, paid tiers available |
| Infura | Established, paid tiers |
| QuickNode | Low latency options |
| Ankr | Budget option |

### 3. Self-Host Base Node

Run your own Base (OP Stack) node:

```bash
docker run --rm -it \
  -v /data/base:/data \
  -p 8545:8545 \
  us-docker.pkg.dev/oplabs-tools-artifacts/images/op-geth:latest \
  --datadir=/data \
  --http \
  --http.addr=0.0.0.0 \
  --http.api=eth,net,web3,debug \
  --syncmode=snap \
  --op-network=base-mainnet
```

**Requires:**
- Ethereum L1 RPC (for Base to read from)
- Significant storage (Base state)
- See [Base docs](https://docs.base.org/guides/run-a-base-node) for full setup

---

## Operational Considerations

### Startup and Sync

1. **First Start:** Node syncs from genesis or snapshot
2. **Database Snapshots:** Ask chain owner for snapshot URL to speed up initial sync
   - Use `--init.url="<SNAPSHOT_URL>"` or `--init.latest=pruned`
3. **Sync Time:** Depends on chain age and traffic; hours to days

### Graceful Shutdown

Always allow graceful shutdown to save state:

```bash
docker stop --time=1800 $(docker ps -aq)
```

### Multiple Nodes

If running multiple nodes:
- Run a **Feed Relay** to avoid duplicate sequencer connections
- Use `--node.feed.output.enable` on relay
- Point nodes at relay instead of sequencer feed

### Monitoring

Enable Prometheus metrics:

```bash
--metrics \
--metrics-server.addr=0.0.0.0 \
--metrics-server.port=6070
```

Key metrics:
- Block height vs sequencer
- RPC latency
- Memory/CPU usage
- Sync status

### Watchtower Mode

By default, nodes run in watchtower mode:
- Validates on-chain assertions
- Logs errors if it disagrees with posted state
- Low overhead, good security practice
- Disable with `--node.staker.enable=false` if not needed

---

## Cost Comparison

### Conduit Managed RPC

| Tier | Throughput | Monthly Cost |
|------|-----------|--------------|
| Free | 12,500 CU/s | $0 |
| Pro | 250,000 CU/s | $50 (free with mainnet rollup) |
| Enterprise | Custom | Contact sales |

### Self-Hosted (Estimates)

| Component | Monthly Cost |
|-----------|-------------|
| Cloud VM (8 core, 64GB RAM) | $200-400 |
| NVMe Storage (1TB) | $50-100 |
| Network egress | Variable |
| Base RPC (if not self-hosted) | $50-200 |

**Break-even:** Self-hosting makes sense when:
- You're hitting rate limits frequently
- Need guaranteed uptime/latency
- Want full control over infrastructure
- Running multiple services that share the node

---

## Quick Reference: Minimum Viable Setup

### For a Full Node (Reader)

1. Get chain info from Conduit dashboard
2. Get Base RPC (provider or self-host)
3. Run:

```bash
docker run -d \
  --name l3-node \
  -v /data/l3:/home/user/.arbitrum \
  -p 8547:8547 \
  offchainlabs/nitro-node:v3.9.4-7f582c3 \
  --parent-chain.connection.url="<BASE_RPC>" \
  --chain.info-json='<CHAIN_INFO>' \
  --chain.name="<CHAIN_NAME>" \
  --node.feed.input.url="<SEQUENCER_FEED>" \
  --execution.forwarding-target="<SEQUENCER_RPC>" \
  --http.api=net,web3,eth \
  --http.addr=0.0.0.0
```

4. Wait for sync
5. Point your apps at `http://localhost:8547`

---

## Resources

- [Arbitrum Docs: Run a Full Node](https://docs.arbitrum.io/run-arbitrum-node/run-full-node)
- [Arbitrum Docs: Running an Orbit Node](https://docs.arbitrum.io/node-running/how-tos/running-an-orbit-node)
- [Conduit Docs: Run an Arbitrum Node](https://docs.conduit.xyz/chains/getting-started/run-a-node/arbitrum-nodes)
- [Arbitrum Docs: Data Availability Committees](https://docs.arbitrum.io/launch-arbitrum-chain/configure-your-chain/common/data-availability/data-availability-committees/get-started)
- [Nitro Node Docker Images](https://hub.docker.com/r/offchainlabs/nitro-node/tags)
- [Arbitrum Discord](https://discord.gg/arbitrum)

---

## Open Questions

- [ ] What's the exact chain info JSON for our L3? (Get from Conduit)
- [ ] Is our L3 Rollup or AnyTrust mode?
- [ ] Do we need archive node capabilities or just full node?
- [ ] What's our expected RPC traffic volume?
- [ ] Self-host Base node or use provider?
