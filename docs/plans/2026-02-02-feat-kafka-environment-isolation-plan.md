---
title: "feat: Kafka Environment Isolation via Topic Prefixes"
type: feat
date: 2026-02-02
brainstorm: docs/brainstorms/2026-02-02-kafka-environment-isolation-brainstorm.md
---

# feat: Kafka Environment Isolation via Topic Prefixes

## Problem

Staging and production share the same Kafka topics. Both environments process all events, causing:
- Duplicate processing (wasted compute)
- Data divergence when code versions differ
- Inability to test with isolated data

## Solution

Require `ENVIRONMENT` env var and derive topic prefix from it:
- `ENVIRONMENT=staging` → topics prefixed with `staging.`
- `ENVIRONMENT=production` → no prefix (existing topics unchanged)
- `ENVIRONMENT` not set → service panics at startup (fail-safe)

```
Production: knowledge.edits, hermes.blocks, space.creations
Staging:    staging.knowledge.edits, staging.hermes.blocks, staging.space.creations
```

## Implementation

Each service reads `ENVIRONMENT` and derives prefix:

```rust
fn get_topic_prefix() -> String {
    let environment = std::env::var("ENVIRONMENT").expect(
        "ENVIRONMENT variable must be set to 'staging' or 'production'"
    );
    match environment.as_str() {
        "staging" => "staging.".to_string(),
        "production" => String::new(),
        other => panic!(
            "ENVIRONMENT must be 'staging' or 'production', got '{}'",
            other
        ),
    }
}

// At startup:
let prefix = get_topic_prefix();
let topic = format!("{}{}", prefix, "knowledge.edits");
tracing::info!(%topic, "Kafka topic configured");
```

### Files to Modify

#### hermes-pipeline/src/emit.rs (Producer - 9 topics)

```rust
// Current (lines 38-49): hardcoded constants
pub mod topics {
    pub const BLOCK_SUMMARY: &str = "hermes.blocks";
    // ...
}

// Change to: read prefix once, format topics
let prefix = get_topic_prefix();
// Then format each topic where used: format!("{}hermes.blocks", prefix)
```

Topics: `hermes.blocks`, `space.creations`, `space.trust.extensions`, `space.membership`, `space.moderation`, `space.topics`, `space.governance`, `curation.votes`, `knowledge.edits`

#### atlas/src/main.rs (Producer - 1 topic)

```rust
// Current (line 247):
let topic = env::var("KAFKA_TOPIC").unwrap_or_else(|_| "topology.canonical".to_string());

// Change to:
let prefix = get_topic_prefix();
let base = env::var("KAFKA_TOPIC").unwrap_or_else(|_| "topology.canonical".to_string());
let topic = format!("{}{}", prefix, base);
```

#### kg-indexer/src/consumer.rs (Consumer - 6 topics)

```rust
// Current (lines 48-56): hardcoded list
let topics = vec!["hermes.blocks".to_string(), ...];

// Change to:
let prefix = get_topic_prefix();
let topics = vec![
    format!("{}hermes.blocks", prefix),
    format!("{}knowledge.edits", prefix),
    format!("{}space.creations", prefix),
    format!("{}space.membership", prefix),
    format!("{}space.trust.extensions", prefix),
    format!("{}space.governance", prefix),
];
```

#### search-indexer/src/consumer/entities_consumer.rs (Consumer - 1 topic)

```rust
// Current (line 42):
const KNOWLEDGE_EDITS_TOPIC: &str = "knowledge.edits";

// Change to:
let prefix = get_topic_prefix();
let base = env::var("KAFKA_TOPIC").unwrap_or_else(|_| "knowledge.edits".to_string());
let topic = format!("{}{}", prefix, base);
```

#### search-indexer/src/consumer/scores_consumer.rs (Consumer - 1 topic)

Same pattern. Topic: `curation.scores`

#### scoring-service/vote-indexer/src/consumer.rs (Consumer - 1 topic)

```rust
// Current (line 15):
const VOTES_TOPIC: &str = "curation.votes";

// Change to:
let prefix = get_topic_prefix();
let topic = format!("{}curation.votes", prefix);
```

### K8s Manifests

Add `ENVIRONMENT` to **all** manifests:

**Staging:**
```yaml
- name: ENVIRONMENT
  value: "staging"
```

**Production:**
```yaml
- name: ENVIRONMENT
  value: "production"
```

Files to update:
- `hermes/k8s/staging/hermes-pipeline.yaml` + `hermes/k8s/production/hermes-pipeline.yaml`
- `hermes/k8s/staging/atlas.yaml` + `hermes/k8s/production/atlas.yaml`
- `kg-indexer/k8s/staging/kg-indexer.yaml` + `kg-indexer/k8s/production/kg-indexer.yaml`
- `search-indexer-deploy/k8s/staging/search-indexer.yaml` + `search-indexer-deploy/k8s/production/search-indexer.yaml`
- `scoring-service/deployment/staging/vote-indexer.yaml` + `scoring-service/deployment/production/vote-indexer.yaml`

## Deployment

**Critical: Deploy consumers before producers to avoid data loss.**

1. Deploy consumers (kg-indexer, search-indexer, vote-indexer)
2. Deploy producers (hermes-pipeline, atlas)
3. Verify data flows through `staging.*` topics in Kafka UI

For production: order doesn't matter (prefix is empty, topics unchanged).

## Acceptance Criteria

- [ ] All services prepend `TOPIC_PREFIX` to topic names
- [ ] Empty/unset prefix = identical behavior to current (backward compatible)
- [ ] Staging K8s manifests set `TOPIC_PREFIX: "staging."`
- [ ] Services log resolved topic names at startup
- [ ] Unit tests for prefix application

## Open Questions

- [ ] Is Kafka auto-create enabled? If not, pre-create staging topics
- [ ] Should staging topics have shorter retention? (Recommended: 1 day vs 7 days)

## References

- Brainstorm: `docs/brainstorms/2026-02-02-kafka-environment-isolation-brainstorm.md`
- hermes-pipeline topics: `hermes-pipeline/src/emit.rs:38-49`
- kg-indexer subscriptions: `kg-indexer/src/consumer.rs:48-56`
- Runbook: `docs/runbooks/staging-production.md`
