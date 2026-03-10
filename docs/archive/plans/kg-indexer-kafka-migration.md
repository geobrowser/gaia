# KG Indexer Kafka Migration Plan

## Overview

Migrate from Substreams-based `indexer` crate to a new Kafka-consuming `kg-indexer` crate that processes events from Hermes Kafka topics.

## Architecture

Simple loop-based consumer - no channels or orchestrator overhead:

```
+-------------------------------------------------+
|  kg-indexer                                     |
|                                                 |
|  loop {                                         |
|      batch = consumer.poll_batch()              |
|      for msg in batch {                         |
|          events = process(msg)                  |
|          storage.write(events)                  |
|      }                                          |
|      consumer.commit()                          |
|  }                                              |
+-------------------------------------------------+

Kafka Topics:
+-- knowledge.edits      (HermesEdit)
+-- space.creations      (HermesCreateSpace)
+-- space.membership     (HermesMembershipChange)
+-- space.trust.extensions (HermesSpaceTrustExtension)
```

## Crate Structure

```
kg-indexer/
+-- Cargo.toml
+-- src/
    +-- main.rs           # Entry point, config, main loop
    +-- consumer.rs       # Kafka consumer setup
    +-- handlers/
    |   +-- mod.rs
    |   +-- edits.rs      # Process HermesEdit -> entities/values/relations
    |   +-- spaces.rs     # Process HermesCreateSpace -> spaces
    |   +-- membership.rs # Process membership -> members/editors
    |   +-- subspaces.rs  # Process trust extensions -> subspaces
    +-- models/           # Copy from indexer or share
    +-- storage.rs        # PostgreSQL operations
    +-- error.rs
```

## Implementation Steps

1. Create crate with Cargo.toml
2. Set up Kafka consumer with rdkafka
3. Copy/adapt models and storage from existing indexer
4. Implement handlers for each message type
5. Wire up main loop
6. Test with docker-compose
7. Create K8s deployment

## Environment Variables

```bash
DATABASE_URL=postgres://...
KAFKA_BROKER=localhost:9092
KAFKA_GROUP_ID=kg-indexer
KAFKA_USERNAME=        # optional
KAFKA_PASSWORD=        # optional
KAFKA_SSL_CA_PEM=      # optional
RUST_LOG=info
```

## Open Questions

1. **PropertiesCache**: Still needed? HermesEdit includes property types.
2. **Blocklist filtering**: Keep in indexer or move to Hermes?
3. **Initial offset**: `earliest` or specific block?
