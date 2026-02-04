//! Painless scripts for OpenSearch operations.

/// Atomically add a type relation to the type_relations array.
/// Idempotent - checks for duplicates before adding.
/// Enforces tombstone dominance - ignores updates to deleted entities.
pub const ADD_TYPE_RELATION_SCRIPT: &str = r#"
    if (ctx._source.containsKey('deleted') && ctx._source.deleted == true) {
        ctx.op = 'noop';
    } else {
        def newRelation = ['relation_id': params.relation_id, 'entity_to_id': params.entity_to_id];
        if (ctx._source.type_relations == null) {
            ctx._source.type_relations = [newRelation];
        } else {
            boolean exists = false;
            for (rel in ctx._source.type_relations) {
                if (rel.relation_id == params.relation_id) {
                    exists = true;
                    break;
                }
            }
            if (!exists) {
                ctx._source.type_relations.add(newRelation);
            }
        }
    }
"#;

/// Atomically remove a type relation from the type_relations array by relation_id.
/// Enforces tombstone dominance - ignores updates to deleted entities.
pub const REMOVE_TYPE_RELATION_SCRIPT: &str = r#"
    if (ctx._source.containsKey('deleted') && ctx._source.deleted == true) {
        ctx.op = 'noop';
    } else if (ctx._source.type_relations != null) {
        ctx._source.type_relations.removeIf(rel -> rel.relation_id != null && rel.relation_id.equals(params.relation_id));
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
