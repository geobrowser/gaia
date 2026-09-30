//! Painless scripts for OpenSearch operations.

/// Atomically add a relation to the relations array.
/// Idempotent - checks for duplicates before adding.
/// Enforces tombstone dominance - ignores updates to deleted entities.
/// Stamps `indexed_at` (from `params.indexed_at`) only when the relation was actually added:
/// a relation can move a document in or out of an embedding scope, a replay changes nothing.
pub const ADD_RELATION_SCRIPT: &str = r#"
    if (ctx._source.containsKey('deleted') && ctx._source.deleted == true) {
        ctx.op = 'noop';
    } else {
        def newRelation = ['relation_id': params.relation_id, 'relation_type': params.relation_type, 'to_entity_id': params.to_entity_id];
        if (ctx._source.relations == null) {
            ctx._source.relations = [newRelation];
            ctx._source.indexed_at = params.indexed_at;
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
                ctx._source.indexed_at = params.indexed_at;
            }
        }
    }
"#;

/// Atomically remove a relation from the relations array by relation_id.
/// Enforces tombstone dominance - ignores updates to deleted entities.
/// Stamps `indexed_at` (from `params.indexed_at`) only when a relation was actually removed.
pub const REMOVE_RELATION_SCRIPT: &str = r#"
    if (ctx._source.containsKey('deleted') && ctx._source.deleted == true) {
        ctx.op = 'noop';
    } else if (ctx._source.relations != null) {
        if (ctx._source.relations.removeIf(rel -> rel.relation_id != null && rel.relation_id.equals(params.relation_id))) {
            ctx._source.indexed_at = params.indexed_at;
        }
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
