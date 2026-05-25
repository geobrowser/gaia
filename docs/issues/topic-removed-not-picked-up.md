# TOPIC_REMOVED not picked up by indexer

## Symptom

When a space declares a topic on-chain, the assignment shows up correctly in the DB and the search index. When a space later removes that topic, **nothing happens downstream** — `spaces.topic_id` stays set, OpenSearch documents still carry `space_topic_entity_id`, and the cache warm-up keeps the stale mapping after a restart.

Both v1 (`TOPIC_REMOVED`) and v2 (`TOPIC_UNSET`) hit the same wall: the indexer was only ever built to handle declarations.

## Root cause

Two separate issues sit on top of each other:

1. **v2 selectors aren't on `dev` yet.** The rename of `TOPIC_DECLARED`/`TOPIC_REMOVED` to `TOPIC_SET`/`TOPIC_UNSET` (PR #177, GEO-568) lives on `feat/governance-v2-contract-migration` and has not been merged into `dev` or `main`. Testnet still runs the v1 keccak hashes — that's why declarations work today and any v2-named event would be invisible.
2. **The v1 `TOPIC_REMOVED` action has no consumer.** The constant is defined (`hermes-substream/src/lib.rs:99`) and re-exported (`hermes-relay/src/actions.rs:61`), but `hermes-pipeline/src/pipelines/topics.rs::transform` only has an `if matches(TOPIC_DECLARED)` branch — removals fall through and are dropped. No `HermesTopicRemoved` proto, no Kafka emit, no kg-indexer handler, no search-indexer handler.

This document covers fix (2). Fix (1) is just merging the v2 rename when contracts land.

For context, the symmetric flow for *subspace*-trust topics (`SUBSPACE_TOPIC_DECLARED`/`SUBSPACE_TOPIC_REMOVED`) **is** fully wired through `hermes-pipeline/src/pipelines/trust.rs` and emits a `SubtopicRemoval` variant on `space.trust.extensions`. The gap is only in the direct space-→-topic relation (`spaces.topic_id`).

## Decisions

**Disambiguation: new proto `HermesTopicRemoved`, dispatched on Kafka `event-type` header.**
We add a second proto message rather than reusing `HermesTopicDeclared` with a header switch or rebuilding the existing message as a `oneof`. The first option lies about what the message carries; the third forces a coordinated schema rollout. A sibling proto is the same pattern used elsewhere in the codebase (`hermes_space_trust_extension::Extension` variants) and keeps consumers type-safe. Both events are still emitted to the same Kafka topic `space.topics` with the same `space_id` partition key, so per-space ordering is preserved.

**OpenSearch: keep the doc, null the field.**
On `TOPIC_REMOVED(S, T)` we patch `space_topic_entity_id` to `null` on doc `{T}_{S}`. The doc is *not* deleted. Two reasons:
1. The doc may already hold real entity content (name, description, relations) written by `knowledge.edits`. There is no separate "mapping" doc to delete in isolation — the mapping field lives on the entity-in-space doc.
2. The update path is patch-style (`provider.rs:295-341`, `:1197-1213`) — setting other fields to `None` leaves them untouched. The cache warm-up aggregator (`provider.rs:351`) only counts spaces where `space_topic_entity_id` is set, so nulling correctly removes the mapping from startup state. Re-declare later cleanly restores the field with all prior content intact.

**kg-indexer: `UPDATE spaces SET topic_id = NULL`.**
Symmetric to the existing `update_space_topic` (`kg-indexer/src/storage.rs:459-472`). No row delete, no soft-delete column. The topic entity row in `entities` is left untouched — it may be referenced by other spaces, by `subspace_topics`, or by knowledge values, and the topic concept itself outlives any one space's assignment.

## Implementation plan

Layered top-down. Each layer's code is sketched but not exhaustive.

### 1. Proto schema — `hermes-schema/proto/topics.proto`

Add a sibling message:

```proto
message HermesTopicRemoved {
  bytes space_id = 1;   // 16 bytes - space removing topic
  bytes topic_id = 2;   // 16 bytes - topic UUID
  blockchain_metadata.BlockchainMetadata meta = 3;
}
```

Regenerate `hermes-schema/src/pb/topics.rs` via the existing prost build.

### 2. Pipeline transform — `hermes-pipeline/src/pipelines/topics.rs`

Extend `TransformResult` and `transform()`:

```rust
pub struct TransformResult {
    pub topics_declared: Vec<HermesTopicDeclared>,
    pub topics_removed: Vec<HermesTopicRemoved>,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.topics_declared.len() + self.topics_removed.len()
    }
}
```

Add a second branch in the loop (line 37-45):

```rust
} else if actions::matches(action_type, &actions::TOPIC_REMOVED) {
    let event = debug_span!(
        "convert.topics.removed",
        space_id = %hex::encode(&action.from_id),
        topic_id = %hex::encode(&action.topic)
    )
    .in_scope(|| convert_topic_removed(action, meta, sequence))?;
    result.topics_removed.push(event);
}
```

`convert_topic_removed` is a near-copy of `convert_topic_declared` — same topic-field layout (`bytes32(bytes16(topicId) | padding)`), same `decode::decode_topic_declared` helper. The decode helper's name is now slightly misleading; rename to `decode_topic_id` in a follow-up if we touch it.

### 3. Pipeline emit — `hermes-pipeline/src/emit.rs`

`impl KafkaEvent for HermesTopicRemoved`: same topic constant `topics::TOPICS = "space.topics"`, same partition key (`space_id`), header `event-type: TOPIC_REMOVED`. Mirrors the existing `impl KafkaEvent for HermesTopicDeclared` at `emit.rs:481-500`.

### 4. Pipeline orchestration — `hermes-pipeline/src/main.rs`

Three changes:

- Add `max_sequence(&topics.topics_removed)` to the array at `:279-298` so block-end marking sees these events.
- Add `mark_sequence_as_last(&mut topics.topics_removed, max_seq)` at `:312`.
- Add `"TOPIC_REMOVED"` to `counts_by_event_type` at `:406-409`.
- Add a sibling emit loop after the declared one at `:540-550`:
  ```rust
  for event in &topics.topics_removed {
      emitter.emit(event).await?;
  }
  ```
  Iteration order in the transform already preserves block-level sequence between declared and removed actions, so interleaved declare-then-remove (or remove-then-declare) within one block apply in the correct order at each consumer's partition.

### 5. kg-indexer consumer dispatch — `kg-indexer/src/consumer.rs`

The `space.topics` parse path at `:117-223` currently decodes every message as `HermesTopicDeclared`. Add a header check:

```rust
let event_type = headers.get("event-type"); // existing helper or inline
let kg_msg = match event_type.as_deref() {
    Some("TOPIC_REMOVED") => KgMessage::TopicRemoved(HermesTopicRemoved::decode(payload)?),
    _ => KgMessage::TopicDeclared(HermesTopicDeclared::decode(payload)?),
};
```

Default-to-declared keeps backward compatibility with any in-flight messages that don't carry the header (the existing emit path already sets it, so this is belt-and-braces).

### 6. kg-indexer handler — `kg-indexer/src/handlers/topics.rs`

Add `handle_topic_removed` returning a typed `SpaceTopicRemoval { space_id, topic_id }`. The `topic_id` is carried mainly for logging/traceability; the storage update only needs `space_id`.

### 7. kg-indexer storage — `kg-indexer/src/storage.rs`

Sibling to `update_space_topic` (`:459-472`):

```rust
pub async fn clear_space_topic(
    &self,
    space_id: Uuid,
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), StorageError> {
    sqlx::query!(
        "UPDATE spaces SET topic_id = NULL WHERE id = $1",
        space_id,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}
```

Idempotent: if the space doesn't exist or already has `topic_id IS NULL`, the UPDATE is a no-op.

### 8. kg-indexer main — `kg-indexer/src/main.rs`

In `handle_message` (~`:1134`) and the batched path (~`:1594`), add a `KgMessage::TopicRemoved` branch that calls `storage.clear_space_topic`.

The `pending_space_topics: HashMap<Uuid, Uuid>` map (`:761-768, :1282, :1466, :1606, :1800-1813`) needs a small tweak: change the value to `Option<Uuid>` so a removal in the same batch as a `SpaceItem` correctly leaves `topic_id` as `None` rather than the prior value. `apply_pending_space_topic` updates accordingly: `Some(t) => SET topic_id = t`, `None => SET topic_id = NULL`. The test at `:1800-1813` gains a removal case.

### 9. search-indexer consumer dispatch — `search-indexer/src/consumer/space_topics_consumer.rs:328-380`

Mirror the kg-indexer dispatch — read `event-type` header, decode either `HermesTopicDeclared` or `HermesTopicRemoved`. Yield a `SpaceTopicEvent` carrying a `kind: TopicEventKind` (or two variants) so the processor can branch.

### 10. search-indexer processor — `search-indexer/src/processor/mod.rs:505-619`

`process_space_topic_batch` branches on the event kind:

- **Declared (existing path)** — unchanged. Insert into `space_topic_cache`, upsert stub doc with `space_topic_entity_id = Some(topic_entity_id)`, fan out doc-id updates (fast path) or `update_by_query` (fallback).
- **Removed (new path)** — `space_topic_cache.remove(&space_id)`, emit a clear operation for the same fan-out shape:
  - Fast path: `ProcessedEvent::UpdateSpaceTopicEntityIdByDoc { doc_id, topic_entity_id: None }` for every entity doc in the space.
  - Slow path: `ProcessedEvent::ClearSpaceTopicEntityId { space_id }`.
  - Also patch the topic entity's own stub doc `{topic_entity_id}_{space_id}` with `space_topic_entity_id = None`.

`UpdateSpaceTopicEntityIdByDoc.topic_entity_id` becomes `Option<String>` (or we add a sibling `ClearSpaceTopicEntityIdByDoc`). I lean toward making the field `Option` because the bulk write path is otherwise identical — `provider.rs:1197-1213` already sends `{"doc": {"space_topic_entity_id": ...}}` and serializing `None` as JSON `null` gives the exact semantics we want.

### 11. search-indexer-repository — `search-indexer-repository/src/opensearch/provider.rs:1231-1303`

Add a sibling `ClearSpaceTopicEntityId` `update_by_query` that runs:

```painless
ctx._source.remove('space_topic_entity_id')
```

over `term: space_id`. Mirrors the existing `UpdateSpaceTopicEntityId` script body but removes the field rather than setting it. Removing (rather than nulling) keeps the field-presence aggregator at `:351 get_space_topic_mappings` clean.

For the by-doc fast path, the existing `build_update_doc` already omits `None` fields, so the patch body for an `Option<String>::None` should serialize as the explicit JSON `null`. Verify: if it does, `space_topic_entity_id: null` is fine on a `keyword` mapping (treated as missing). If it doesn't and OpenSearch keeps the prior value, use a small painless inline script in the by-doc path too.

## Tests

### Unit

- `hermes-pipeline/src/pipelines/topics.rs` — add `test_convert_topic_removed_uses_topic_field`, `test_transform_counts_remove`, `test_transform_interleaved_declare_remove`.
- `kg-indexer/src/handlers/topics.rs` — add `test_handle_topic_removed`, `test_handle_topic_removed_invalid_topic_id`.

### Mock-event helpers

- `hermes-relay/src/source/mock_events.rs` — add `topic_removed(space_id, topic_id)` mirroring `topic_declared` at `:887-901`. Action const already wired (`actions::TOPIC_REMOVED`).
- `search-indexer/tests/e2e-kafka-search-api/src/generators/space_topics.rs` — add `create_topic_removed` mirroring `create_topic_declared`.

### E2E

- `kg-indexer/tests/e2e.rs` — add a test that emits declare → assert `spaces.topic_id` is set → emit remove → assert `spaces.topic_id IS NULL`. There is no existing e2e for the basic declare flow either; this closes that gap simultaneously.
- `search-indexer/tests/e2e-kafka-search-api/src/main.rs` — extend the step around `:1048-1075` with a removal that asserts:
  - `space_topic_entity_id` is absent (or null) on the topic entity's doc and on other entities in the space.
  - The doc itself still exists with its prior fields.
  - Cache warm-up (simulate restart) no longer maps `S → T`.

## Rollout order

The change is one logical PR. Order within the PR:

1. Proto + generated code (no behavior change).
2. Pipeline transform + emit (now produces `HermesTopicRemoved`, but nothing consumes).
3. kg-indexer dispatch + handler + storage + main wiring.
4. search-indexer dispatch + processor + repository.
5. Tests at each layer.

Deploy order in staging: deploy `hermes-pipeline` first (starts emitting the new event), then kg-indexer and search-indexer (start consuming it). The other way around is also safe — consumers that don't see any `TOPIC_REMOVED` events yet are a no-op. There is no backfill: events already missed will simply not be replayed.

## Open items

- **Decode helper name.** `hermes-pipeline/src/decode.rs::decode_topic_declared` will be used for both events. Rename in a separate cleanup PR.
- **Re-declare semantics.** Declare → Remove → Declare in three separate blocks works correctly with the doc-preserve approach. Declare → Remove → Declare in the *same* block is rare but should also work: the events stream through the partition in sequence order and apply in order. Tested by `test_transform_interleaved_declare_remove`.
- **Notification UX.** No notification fires for `TOPIC_DECLARED` today; we mirror that and emit none for `TOPIC_REMOVED`. Out of scope.
- **`pending_space_topics` type change.** Going from `HashMap<Uuid, Uuid>` to `HashMap<Uuid, Option<Uuid>>` ripples through `apply_pending_space_topic` and its test. Self-contained but worth pre-flagging in review.
- **v2 rename (PR #177, GEO-568).** Once `feat/governance-v2-contract-migration` lands, the constants `ACTION_TOPIC_DECLARED`/`ACTION_TOPIC_REMOVED` become `ACTION_TOPIC_SET`/`ACTION_TOPIC_UNSET` with new keccak hashes. Everything downstream in this plan keeps working — only the substream-layer constants change.
