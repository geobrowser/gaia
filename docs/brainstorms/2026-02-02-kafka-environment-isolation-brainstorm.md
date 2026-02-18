---
date: 2026-02-02
topic: kafka-environment-isolation
---

# Kafka Environment Isolation

## What We're Building

Environment-isolated Kafka topic routing so staging and production no longer share event streams. Each environment (staging, production) will produce and consume from its own set of topics within the shared Kafka cluster.

**Current state:** Staging and production share the same Kafka topics. Both environments process all events, leading to:
- Duplicate processing (wasted compute)
- Data divergence when code versions differ
- Inability to test with mock data without polluting production

**Target state:** Complete isolation via topic prefixes:
- Production: `knowledge.edits`, `space.creations`, `hermes.blocks`, etc.
- Staging: `staging.knowledge.edits`, `staging.space.creations`, `staging.hermes.blocks`, etc.

## Why This Approach

**Approaches considered:**

| Approach | Verdict |
|----------|---------|
| Topic prefixes | **Chosen** - Strong isolation, simple implementation |
| Header-based filtering | Rejected - Still wastes resources deserializing all messages, risk of filtering bugs |
| Separate Kafka clusters | Rejected - Infrastructure overhead for what's achievable with topic separation |
| Topic registry | Rejected - Overkill for two-environment scenario |

**Why topic prefixes:**
1. Physical isolation eliminates cross-contamination risk
2. Simple env var configuration (`TOPIC_PREFIX`)
3. Matches current pattern of namespace separation in K8s
4. Supports canary testing of all services (hermes-pipeline, Atlas, indexers)

## Key Decisions

- **Prefix convention**: Staging uses `staging.` prefix; production uses no prefix (preserves existing topic names)
- **Environment injection**: Services receive `TOPIC_PREFIX` via K8s manifest env vars (same pattern as `KAFKA_GROUP_ID`)
- **Scope**: All Kafka-connected services—hermes-pipeline, Atlas, kg-indexer, search-indexer, vote-indexer
- **Consumer groups**: Keep existing `*-staging` suffix pattern for consumer group IDs
- **Topic creation**: Staging topics must be created (can be automated in deployment workflows)

## Configuration Pattern

```yaml
# staging manifest
env:
  - name: TOPIC_PREFIX
    value: "staging."
  - name: KAFKA_GROUP_ID
    value: "kg-indexer-staging"

# production manifest (TOPIC_PREFIX omitted or empty)
env:
  - name: KAFKA_GROUP_ID
    value: "kg-indexer"
```

Services prepend `TOPIC_PREFIX` (defaulting to empty string) to all topic names.

## Architecture

```
Shared Kafka Cluster
├── Production Topics (no prefix)
│   ├── hermes.blocks
│   ├── knowledge.edits
│   ├── space.creations
│   ├── space.membership
│   ├── space.trust.extensions
│   ├── space.governance
│   ├── curation.votes
│   └── topology.canonical
│
└── Staging Topics (staging. prefix)
    ├── staging.hermes.blocks
    ├── staging.knowledge.edits
    ├── staging.space.creations
    ├── staging.space.membership
    ├── staging.space.trust.extensions
    ├── staging.space.governance
    ├── staging.curation.votes
    └── staging.topology.canonical
```

**Service isolation per environment:**
- hermes-pipeline (staging) → produces to `staging.*` topics
- hermes-pipeline (prod) → produces to unprefixed topics
- Atlas (staging) → consumes `staging.*`, produces `staging.topology.canonical`
- Atlas (prod) → consumes unprefixed, produces `topology.canonical`
- Indexers follow same pattern

## Services Affected

| Service | Role | Changes Needed |
|---------|------|----------------|
| hermes-pipeline | Producer | Add `TOPIC_PREFIX` env var, prepend to all produced topics |
| atlas | Consumer + Producer | Add `TOPIC_PREFIX` for both consuming and producing |
| kg-indexer | Consumer | Add `TOPIC_PREFIX` to subscribed topic list |
| search-indexer | Consumer | Add `TOPIC_PREFIX` to `KAFKA_TOPIC` config |
| vote-indexer | Consumer | Add `TOPIC_PREFIX` to subscribed topic |

## Open Questions

- **Topic auto-creation**: Should Kafka be configured to auto-create topics, or should deployment workflows explicitly create staging topics?
- **Retention policies**: Should staging topics have shorter retention than production?
- **Migration**: How to handle in-flight messages during rollout? (Likely: coordinate deployment to minimize overlap)

## Next Steps

Run `/workflows:plan` for implementation details.
