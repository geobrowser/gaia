//! Painless scripts for OpenSearch operations.

/// Atomically add a type relation to the type_relations array.
/// Idempotent - checks for duplicates before adding.
pub const ADD_TYPE_RELATION_SCRIPT: &str = r#"
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
"#;

/// Atomically remove a type relation from the type_relations array by relation_id.
pub const REMOVE_TYPE_RELATION_SCRIPT: &str = r#"
    if (ctx._source.type_relations != null) {
        ctx._source.type_relations.removeIf(rel -> rel.relation_id != null && rel.relation_id.equals(params.relation_id));
    }
"#;
