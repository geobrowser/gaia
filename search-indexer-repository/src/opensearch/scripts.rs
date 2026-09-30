//! Painless scripts for OpenSearch operations.

/// Atomically add a relation to the relations array.
/// Idempotent - checks for duplicates before adding.
/// Enforces tombstone dominance - ignores updates to deleted entities.
pub const ADD_RELATION_SCRIPT: &str = r#"
    if (ctx._source.containsKey('deleted') && ctx._source.deleted == true) {
        ctx.op = 'noop';
    } else {
        def newRelation = ['relation_id': params.relation_id, 'relation_type': params.relation_type, 'to_entity_id': params.to_entity_id];
        if (ctx._source.relations == null) {
            ctx._source.relations = [newRelation];
        } else {
            boolean exists = false;
            for (rel in ctx._source.relations) {
                if (rel.relation_id == params.relation_id) {
                    exists = true;
                    break;
                }
            }
            if (!exists) {
                ctx._source.relations.add(newRelation);
            }
        }
    }
"#;

/// Atomically remove a relation from the relations array by relation_id.
/// Enforces tombstone dominance - ignores updates to deleted entities.
pub const REMOVE_RELATION_SCRIPT: &str = r#"
    if (ctx._source.containsKey('deleted') && ctx._source.deleted == true) {
        ctx.op = 'noop';
    } else if (ctx._source.relations != null) {
        ctx._source.relations.removeIf(rel -> rel.relation_id != null && rel.relation_id.equals(params.relation_id));
    }
"#;

/// Delete a per-space document whose entity has nothing left in the space (GEO-2548).
///
/// The caller has already confirmed in Postgres that the entity has no value and no
/// outgoing relation in the space. Two kinds of document are kept anyway, exactly as
/// `search-admin reconcile-orphans` keeps them:
///
/// - tombstones (`deleted: true`), which record a `DeleteEntity` that kg-indexer does not
///   apply, so Postgres cannot speak for them, and which `/search` already hides;
/// - space-topic stubs (`space_topic_entity_id == entity_id`), which search-indexer writes
///   from `space.topics` with no edit behind them.
///
/// Deleting rather than tombstoning is deliberate: a tombstone dominates every later
/// update, so an entity that gains a value in the space again would stay hidden. After a
/// delete, the next upsert simply recreates the document.
pub const RETIRE_EMPTY_DOC_SCRIPT: &str = r#"
    if (ctx._source.deleted == true) {
        ctx.op = 'noop';
    } else if (ctx._source.space_topic_entity_id != null
            && ctx._source.space_topic_entity_id == ctx._source.entity_id) {
        ctx.op = 'noop';
    } else {
        ctx.op = 'delete';
    }
"#;

/// Script for updating document fields with tombstone dominance.
/// If entity is deleted, the update is ignored (noop) unless the update explicitly sets the deleted field
/// (either to true for re-delete or false for restore).
/// If entity is not deleted, fields from params.doc are merged into _source.
pub const UPDATE_WITH_TOMBSTONE_CHECK_SCRIPT: &str = r#"
    if (ctx._source.containsKey('deleted') && ctx._source.deleted == true && !params.doc.containsKey('deleted')) {
        ctx.op = 'noop';
    } else {
        for (entry in params.doc.entrySet()) {
            ctx._source[entry.getKey()] = entry.getValue();
        }
    }
"#;
