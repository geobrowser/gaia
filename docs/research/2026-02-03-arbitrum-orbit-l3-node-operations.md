# Running a Self-Hosted Node for an Arbitrum Orbit L3 on Base

**Date:** 2026-02-03  
**Status:** Research  
**Stack:** Arbitrum Orbit L3 settling on Base (L2)

---

## TL;DR

To eliminate Conduit's RPC rate limits, run a **pruned full node**. That's it.

- **What you run:** 1 full node (pruned)
- **What Conduit runs:** Sequencer, Data Availability Servers (if AnyTrust), batch posting
- **Hardware:** 4 cores, 16 GB RAM, 500 GB - 1 TB NVMe
- **Cost:** ~$100-200/mo

---

## Overview

This document covers running your own node infrastructure for a custom Arbitrum Orbit L3 chain that settles on Base. Running your own node eliminates rate limiting from Conduit's managed RPCs.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         CONDUIT MANAGES                                  │
│                                                                          │
│   ┌───────────┐      ┌───────────┐      ┌───────────┐                   │
│   │ Sequencer │─────▶│  DAS (1)  │      │  DAS (2)  │  (AnyTrust only)  │
│   └─────┬─────┘      └─────┬─────┘      └─────┬─────┘                   │
│         │                  │                  │                          │
│         │ Posts batches    │ Stores tx data   │                          │
│         ▼                  │                  │                          │
│   ┌───────────┐            │                  │                          │
│   │   Base    │            │                  │                          │
│   └───────────┘            │                  │                          │
└─────────────────────────────┼──────────────────┼─────────────────────────┘
                              │                  │
              Fetches data    │                  │
              (AnyTrust)      ▼                  ▼
                    ┌─────────────────────────────────┐
                    │       YOUR FULL NODE            │
                    │                                 │
                    │  • Syncs from sequencer feed    │
                    │  • Serves unlimited RPC         │
                    │  • Forwards txs to sequencer    │
                    └─────────────────────────────────┘
```

## What You Need vs What Conduit Provides

| Component | Who Runs It | Notes |
|-----------|-------------|-------|
| **Sequencer** | Conduit | Orders txs, posts batches |
| **Data Availability Servers** | Conduit | Stores tx data (AnyTrust only) |
| **Batch posting to Base** | Conduit | Via sequencer |
| **Full node (your RPC)** | **You** | The only thing you run |

### You DON'T Need To Run

- **Sequencer** - Conduit operates this; you can't run your own
- **DAS nodes** - Conduit runs these; your node just fetches from their endpoints
- **Archive node** - Pruned is sufficient for RPC and event indexing
- **Validator/Staker** - Optional, watchtower mode runs by default anyway

---

## Full Node vs Archive Node

| Data | Pruned Full Node | Archive Node |
|------|------------------|--------------|
| All blocks | ✅ Forever | ✅ Forever |
| All transactions | ✅ Forever | ✅ Forever |
| All event logs | ✅ Forever | ✅ Forever |
| All receipts | ✅ Forever | ✅ Forever |
| State at old blocks | ❌ Only ~128 recent | ✅ Every block |

**For indexers that read events/logs:** Pruned is fine. All historical blocks, transactions, and logs are retained.

**Archive is only needed for:** Historical `eth_call` (e.g., "what was this balance 6 months ago?"), historical trace calls, or block explorer backends that show old state.

**Can you upgrade to archive later?** Yes, but you'd need to resync from genesis or an archive snapshot. You can't convert pruned → archive.

---

## Hardware Requirements

| Traffic Level | RAM | CPU | Storage | Monthly Cost |
|---------------|-----|-----|---------|--------------|
| **Low** | 8 GB | 2 cores | 200 GB NVMe | $40-80 |
| **Medium** | 16 GB | 4 cores | 500 GB NVMe | $100-200 |
| **High** | 32-64 GB | 8 cores | 1 TB NVMe | $300-500 |

**Key points:**
- Single-core performance matters most
- NVMe SSDs required
- Network: 100 Mbps min, 1 Gbps recommended

### Storage Estimates

Arbitrum Orbit: 32M gas/block, ~250ms blocks, max ~1,280 TPS

| Chain Activity | 1 Year (Pruned) |
|----------------|-----------------|
| Light (~5%) | 50-100 GB |
| Moderate (~25%) | 150-400 GB |
| Heavy (~50%) | 300-800 GB |
| Saturated (100%) | 500 GB - 1.5 TB |

**Recommendation:** Start with 500 GB - 1 TB for production.

---

## Getting Configuration from Conduit

1. Log into [Conduit App](https://app.conduit.xyz)
2. Go to deployment dashboard → Settings → General
3. Enable "Enable external nodes"
4. Click "Run a Node" to get:
   - **Chain Info JSON** - Chain ID, contract addresses, config
   - **Sequencer Feed URL** - WebSocket endpoint
   - **Sequencer RPC URL** - For forwarding transactions
   - **DAS REST endpoints** - For AnyTrust chains (to fetch tx data)

### Check: Rollup vs AnyTrust

Look for `DataAvailabilityCommittee` in the chain config JSON:
- `false` = Rollup mode (no DAS config needed)
- `true` = AnyTrust mode (need DAS endpoints from Conduit)

---

## Running the Node

### Prerequisites

1. **Base RPC** - Your node needs to read from Base (the parent chain)
   - Use a provider (Alchemy, Infura, QuickNode): $50-200/mo
   - Or Conduit's Base RPC (rate limited)

2. **Config from Conduit** - Chain info, sequencer feed, DAS endpoints

### Docker Command (Rollup Mode)

```bash
docker run -d \
  --name l3-node \
  --restart unless-stopped \
  -v /data/l3:/home/user/.arbitrum \
  -p 8547:8547 \
  -p 8548:8548 \
  offchainlabs/nitro-node:v3.9.4-7f582c3 \
  --parent-chain.connection.url="<BASE_RPC_URL>" \
  --chain.info-json='<CHAIN_INFO_JSON>' \
  --chain.name="<CHAIN_NAME>" \
  --node.feed.input.url="<SEQUENCER_FEED_WSS_URL>" \
  --execution.forwarding-target="<SEQUENCER_RPC_URL>" \
  --http.api=net,web3,eth \
  --http.corsdomain=* \
  --http.addr=0.0.0.0 \
  --http.vhosts=*
```

### Docker Command (AnyTrust Mode)

Add DAS configuration:

```bash
docker run -d \
  --name l3-node \
  --restart unless-stopped \
  -v /data/l3:/home/user/.arbitrum \
  -p 8547:8547 \
  -p 8548:8548 \
  offchainlabs/nitro-node:v3.9.4-7f582c3 \
  --parent-chain.connection.url="<BASE_RPC_URL>" \
  --chain.info-json='<CHAIN_INFO_JSON>' \
  --chain.name="<CHAIN_NAME>" \
  --node.feed.input.url="<SEQUENCER_FEED_WSS_URL>" \
  --execution.forwarding-target="<SEQUENCER_RPC_URL>" \
  --node.data-availability.enable \
  --node.data-availability.rest-aggregator.enable \
  --node.data-availability.rest-aggregator.urls="<CONDUIT_DAS_ENDPOINT>" \
  --http.api=net,web3,eth \
  --http.corsdomain=* \
  --http.addr=0.0.0.0 \
  --http.vhosts=*
```

### Key Parameters

| Parameter | Description |
|-----------|-------------|
| `--parent-chain.connection.url` | Base RPC endpoint |
| `--chain.info-json` | Chain config from Conduit |
| `--chain.name` | Must match name in chain.info-json |
| `--node.feed.input.url` | Sequencer feed WebSocket |
| `--execution.forwarding-target` | Sequencer RPC for tx forwarding |
| `--node.data-availability.*` | DAS config (AnyTrust only) |

### Ports

| Protocol | Port |
|----------|------|
| RPC/HTTP | 8547 |
| RPC/WebSocket | 8548 |

---

## Operations

### Initial Sync

- First start syncs from genesis (can take hours to days)
- Ask Conduit for a snapshot URL to speed this up:
  ```bash
  --init.url="<SNAPSHOT_URL>"
  ```

### Graceful Shutdown

```bash
docker stop --time=300 l3-node
```

### Monitoring

Enable Prometheus metrics:
```bash
--metrics \
--metrics-server.addr=0.0.0.0 \
--metrics-server.port=6070
```

Key things to watch:
- Block height (should match sequencer)
- RPC latency
- Sync status

### Multiple Nodes

If running multiple nodes, run a **Feed Relay** to avoid duplicate sequencer connections:
- One node connects to Conduit's feed with `--node.feed.output.enable`
- Other nodes connect to that relay

---

## Cost Comparison

### Conduit Managed RPC

| Tier | Throughput | Monthly |
|------|-----------|---------|
| Free | 12,500 CU/s | $0 |
| Pro | 250,000 CU/s | $50 (free with mainnet rollup) |

### Self-Hosted

| Component | Monthly |
|-----------|---------|
| VM (4 core, 16GB, 500GB) | $100-200 |
| Base RPC provider | $50-200 |
| **Total** | **$150-400** |

**When to self-host:**
- Hitting rate limits
- Need guaranteed latency
- Want full control

---

## Checklist

- [ ] Get chain info JSON from Conduit dashboard
- [ ] Get sequencer feed URL from Conduit
- [ ] Get sequencer RPC URL from Conduit  
- [ ] Check if AnyTrust mode → get DAS endpoints from Conduit
- [ ] Set up Base RPC (provider or Conduit's)
- [ ] Provision VM (4 core, 16GB RAM, 500GB NVMe)
- [ ] Run node, wait for sync
- [ ] Point apps at your node's RPC

---

## Resources

- [Arbitrum Docs: Run a Full Node](https://docs.arbitrum.io/run-arbitrum-node/run-full-node)
- [Conduit Docs: Run an Arbitrum Node](https://docs.conduit.xyz/chains/getting-started/run-a-node/arbitrum-nodes)
- [Nitro Node Docker Images](https://hub.docker.com/r/offchainlabs/nitro-node/tags)
