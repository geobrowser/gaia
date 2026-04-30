use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;

#[cfg(test)]
use grc_20::model::ContextEdge;
use grc_20::{
    decode_edit, model::Context, Edit as Grc20Edit, Id as Grc20Id, Op as Grc20Op, PropertyValue,
    UnsetRelationField, Value as Grc20Value,
};
use hermes_schema::pb::knowledge::HermesEdit;
use sdk::core::ids::{PROTECTED_PROPERTY_IDS, PROTECTED_RELATION_TYPE_IDS, ROOT_SPACE_ID};
use tracing::{debug_span, warn};
use uuid::Uuid;

use crate::error::HandlerError;
use crate::models::{
    entities::EntityItem,
    relations::{
        DeleteRelationItem, RelationOp, SetRelationItem, UnsetRelationItem, UpdateRelationItem,
    },
    values::{ValueChangeType, ValueOp},
};

static PROTECTED_PROPERTIES: LazyLock<HashSet<Uuid>> = LazyLock::new(|| {
    PROTECTED_PROPERTY_IDS
        .iter()
        .map(|id| Uuid::parse_str(id).expect("invalid protected property ID"))
        .collect()
});

static PROTECTED_RELATION_TYPES: LazyLock<HashSet<Uuid>> = LazyLock::new(|| {
    PROTECTED_RELATION_TYPE_IDS
        .iter()
        .map(|id| Uuid::parse_str(id).expect("invalid protected relation type ID"))
        .collect()
});

static ROOT_SPACE_UUID: LazyLock<Uuid> =
    LazyLock::new(|| Uuid::parse_str(ROOT_SPACE_ID).expect("invalid ROOT_SPACE_ID"));

/// Result of processing an edit message
pub struct EditResult {
    /// The edit ID (from HermesEdit.id)
    pub edit_id: Uuid,
    pub entities: Vec<EntityItem>,
    pub values: Vec<ValueOp>,
    pub relations: Vec<RelationOp>,
}

/// Metadata extracted from the HermesEdit for timestamping
pub struct EditMetadata {
    pub timestamp: String,
    pub block_number: String,
}

impl EditMetadata {
    pub fn from_edit(edit: &HermesEdit) -> Self {
        let (timestamp, block_number) = edit
            .meta
            .as_ref()
            .map(|m| (m.created_at.to_string(), m.block_number.to_string()))
            .unwrap_or_else(|| ("0".to_string(), "0".to_string()));

        Self {
            timestamp,
            block_number,
        }
    }
}

/// Decode GRC2/GRC2Z payload bytes into a grc_20::Edit
///
/// The grc_20::decode_edit function handles both compressed (GRC2Z) and
/// uncompressed (GRC2) formats internally. For compressed data, it returns
/// owned strings; for uncompressed, it borrows from the input.
fn decode_payload(payload: &[u8]) -> Result<Grc20Edit<'_>, HandlerError> {
    if payload.is_empty() {
        warn!("decode_payload called with empty payload");
        return Err(HandlerError::DecodeError("Empty payload".to_string()));
    }

    decode_edit(payload).map_err(|e| {
        // Log with context to help debug decode failures
        let first_bytes = &payload[..payload.len().min(32)];
        warn!(
            payload_size = payload.len(),
            first_bytes = ?first_bytes,
            error = ?e,
            "GRC-20 decode failed"
        );
        HandlerError::DecodeError(format!("GRC-20 decode error: {:?}", e))
    })
}

/// Convert a grc_20::Id (16 bytes) to a Uuid
fn id_to_uuid(id: &Grc20Id) -> Uuid {
    Uuid::from_bytes(*id)
}

/// Extract context metadata for grouping changes (GRC-20 Section 4.5).
/// Returns (root_id, first_edge_type_id) from the context.
fn extract_context(ctx: &Option<Context>) -> (Option<Uuid>, Option<Uuid>) {
    match ctx {
        Some(c) => {
            let root_id = id_to_uuid(&c.root_id);
            let edge_type_id = c.edges.first().map(|e| id_to_uuid(&e.type_id));
            (Some(root_id), edge_type_id)
        }
        None => (None, None),
    }
}

/// Filter out value operations targeting protected property IDs.
///
/// Values for `SpaceAddress` / `VotingMode` / `SpaceId` / `CreatedAtBlock` are
/// written directly by `handlers::system_entities::map_space_registered` from
/// onchain events. Allowing edit-pipeline writes — even from the root space —
/// would let two writers race for the same value rows. Always drop, regardless
/// of `space_id`.
fn filter_protected_values(ops: Vec<ValueOp>) -> Vec<ValueOp> {
    ops.into_iter()
        .filter(|op| !PROTECTED_PROPERTIES.contains(&op.property_id))
        .collect()
}

/// Filter `Create` relation ops against system-protected state.
///
/// - `SYSTEM_TYPES_RELATION_TYPE_ID` is always dropped — owned by
///   `map_space_registered`, never authored via edits (even from root).
/// - Relations whose `from`/`to` is in `PROTECTED_PROPERTY_IDS` are dropped
///   unless the publishing space is the root space, which authors the
///   schema-defining seed edit (e.g. `SpaceAddress -types-> Property`).
/// - `Update` / `Delete` / `Unset` pass through; tightened separately.
fn filter_protected_relations(ops: Vec<RelationOp>, edit_space_id: &Uuid) -> Vec<RelationOp> {
    let is_root = edit_space_id == &*ROOT_SPACE_UUID;
    ops.into_iter()
        .filter(|op| match op {
            RelationOp::Create(rel) => {
                let type_protected = PROTECTED_RELATION_TYPES.contains(&rel.type_id);
                let from_protected = PROTECTED_PROPERTIES.contains(&rel.from_id);
                let to_protected = PROTECTED_PROPERTIES.contains(&rel.to_id);
                !type_protected && (is_root || (!from_protected && !to_protected))
            }
            _ => true,
        })
        .collect()
}

/// Process a HermesEdit message and return the extracted data
pub fn handle_edit(edit: &HermesEdit) -> Result<EditResult, HandlerError> {
    let edit_id = parse_edit_id(&edit.id)?;
    let space_id = parse_space_id(edit.space_id.as_slice())?;
    let meta = EditMetadata::from_edit(edit);

    // Decode the v2 payload bytes (DEBUG level - only visible when RUST_LOG=debug)
    let grc20_edit = {
        let _span =
            debug_span!("kg_indexer.decode_grc20", payload_size = edit.payload.len()).entered();
        decode_payload(&edit.payload)?
    };

    // Extract all data from the decoded edit (DEBUG level)
    let (entities, value_ops, relation_ops) = {
        let _span =
            debug_span!("kg_indexer.extract_ops", op_count = grc20_edit.ops.len()).entered();
        let entities = extract_entities(&grc20_edit, &space_id, &meta);
        let value_ops = extract_values(&grc20_edit, &space_id);
        let relation_ops = extract_relations(&grc20_edit, &space_id);
        (entities, value_ops, relation_ops)
    };

    // Filter out operations targeting system-protected properties and relation types
    let value_ops = filter_protected_values(value_ops);
    let relation_ops = filter_protected_relations(relation_ops, &space_id);

    // Squash operations within this edit to resolve conflicts
    let values = squash_values(&value_ops);
    let relations = squash_relations(&relation_ops);

    Ok(EditResult {
        edit_id,
        entities,
        values,
        relations,
    })
}

fn parse_edit_id(id_bytes: &[u8]) -> Result<Uuid, HandlerError> {
    if id_bytes.len() != 16 {
        return Err(HandlerError::DecodeError(format!(
            "Invalid edit ID: expected 16 bytes, got {}",
            id_bytes.len()
        )));
    }
    let bytes: [u8; 16] = id_bytes.try_into().map_err(|_| {
        HandlerError::DecodeError("Failed to convert edit ID bytes to array".to_string())
    })?;
    Ok(Uuid::from_bytes(bytes))
}

/// Squash value operations - last operation for each value ID wins
fn squash_values(value_ops: &[ValueOp]) -> Vec<ValueOp> {
    let mut hash: HashMap<Uuid, ValueOp> = HashMap::new();

    for op in value_ops {
        hash.insert(op.id, op.clone());
    }

    hash.into_values().collect()
}

/// Squash relation operations with proper merging logic
fn squash_relations(relation_ops: &[RelationOp]) -> Vec<RelationOp> {
    let mut hash: HashMap<Uuid, RelationOp> = HashMap::new();

    for op in relation_ops {
        let id = op.id();
        if let Some(existing) = hash.get(&id) {
            let merged = merge_relation_ops(existing.clone(), op.clone());
            hash.insert(id, merged);
        } else {
            hash.insert(id, op.clone());
        }
    }

    hash.into_values().collect()
}

/// Merge two relation operations for the same ID
fn merge_relation_ops(existing: RelationOp, new: RelationOp) -> RelationOp {
    match (existing, new.clone()) {
        // create -> create: Use the new create
        (RelationOp::Create(_), RelationOp::Create(_)) => new,

        // create -> update: Merge fields into create
        (RelationOp::Create(c), RelationOp::Update(u)) => RelationOp::Create(SetRelationItem {
            id: c.id,
            entity_id: c.entity_id,
            type_id: c.type_id,
            from_id: c.from_id,
            to_id: c.to_id,
            space_id: c.space_id,
            from_space_id: u.from_space_id.or(c.from_space_id),
            from_version_id: u.from_version_id.or(c.from_version_id),
            to_space_id: u.to_space_id.or(c.to_space_id),
            to_version_id: u.to_version_id.or(c.to_version_id),
            position: u.position.or(c.position),
            verified: u.verified.or(c.verified),
            is_system: c.is_system,
            context_root_id: c.context_root_id,
            context_edge_type_id: c.context_edge_type_id,
        }),

        // create -> delete: Delete wins
        (RelationOp::Create(_), RelationOp::Delete(d)) => RelationOp::Delete(d),

        // create -> unset: Apply unset to create
        (RelationOp::Create(c), RelationOp::Unset(u)) => RelationOp::Create(SetRelationItem {
            id: c.id,
            entity_id: c.entity_id,
            type_id: c.type_id,
            from_id: c.from_id,
            to_id: c.to_id,
            space_id: c.space_id,
            from_space_id: if u.from_space_id == Some(true) {
                None
            } else {
                c.from_space_id
            },
            from_version_id: if u.from_version_id == Some(true) {
                None
            } else {
                c.from_version_id
            },
            to_space_id: if u.to_space_id == Some(true) {
                None
            } else {
                c.to_space_id
            },
            to_version_id: if u.to_version_id == Some(true) {
                None
            } else {
                c.to_version_id
            },
            position: if u.position == Some(true) {
                None
            } else {
                c.position
            },
            verified: if u.verified == Some(true) {
                None
            } else {
                c.verified
            },
            is_system: c.is_system,
            context_root_id: c.context_root_id,
            context_edge_type_id: c.context_edge_type_id,
        }),

        // update -> create: Create wins (overwrites)
        (RelationOp::Update(_), RelationOp::Create(_)) => new,

        // update -> update: Merge fields
        (RelationOp::Update(e), RelationOp::Update(u)) => RelationOp::Update(UpdateRelationItem {
            id: e.id,
            space_id: e.space_id,
            from_space_id: u.from_space_id.or(e.from_space_id),
            from_version_id: u.from_version_id.or(e.from_version_id),
            to_space_id: u.to_space_id.or(e.to_space_id),
            to_version_id: u.to_version_id.or(e.to_version_id),
            position: u.position.or(e.position),
            verified: u.verified.or(e.verified),
            // Later context wins; fall back to earlier if later is None.
            context_root_id: u.context_root_id.or(e.context_root_id),
            context_edge_type_id: u.context_edge_type_id.or(e.context_edge_type_id),
        }),

        // update -> delete: Delete wins
        (RelationOp::Update(_), RelationOp::Delete(d)) => RelationOp::Delete(d),

        // update -> unset: Apply unset to update
        (RelationOp::Update(e), RelationOp::Unset(u)) => RelationOp::Update(UpdateRelationItem {
            id: e.id,
            space_id: e.space_id,
            from_space_id: if u.from_space_id == Some(true) {
                None
            } else {
                e.from_space_id
            },
            from_version_id: if u.from_version_id == Some(true) {
                None
            } else {
                e.from_version_id
            },
            to_space_id: if u.to_space_id == Some(true) {
                None
            } else {
                e.to_space_id
            },
            to_version_id: if u.to_version_id == Some(true) {
                None
            } else {
                e.to_version_id
            },
            position: if u.position == Some(true) {
                None
            } else {
                e.position
            },
            verified: if u.verified == Some(true) {
                None
            } else {
                e.verified
            },
            context_root_id: u.context_root_id.or(e.context_root_id),
            context_edge_type_id: u.context_edge_type_id.or(e.context_edge_type_id),
        }),

        // delete -> anything: New op wins (recreation after delete)
        (RelationOp::Delete(_), _) => new,

        // unset -> create: Create wins
        (RelationOp::Unset(_), RelationOp::Create(_)) => new,

        // unset -> update: Update wins
        (RelationOp::Unset(_), RelationOp::Update(_)) => new,

        // unset -> delete: Delete wins
        (RelationOp::Unset(_), RelationOp::Delete(d)) => RelationOp::Delete(d),

        // unset -> unset: Merge the unset flags
        (RelationOp::Unset(e), RelationOp::Unset(u)) => RelationOp::Unset(UnsetRelationItem {
            id: e.id,
            space_id: e.space_id,
            from_space_id: u.from_space_id.or(e.from_space_id),
            from_version_id: u.from_version_id.or(e.from_version_id),
            to_space_id: u.to_space_id.or(e.to_space_id),
            to_version_id: u.to_version_id.or(e.to_version_id),
            position: u.position.or(e.position),
            verified: u.verified.or(e.verified),
            context_root_id: u.context_root_id.or(e.context_root_id),
            context_edge_type_id: u.context_edge_type_id.or(e.context_edge_type_id),
        }),
    }
}

fn parse_space_id(space_id_bytes: &[u8]) -> Result<Uuid, HandlerError> {
    // Convert 16-byte UUID to UUID struct
    if space_id_bytes.len() != 16 {
        return Err(HandlerError::InvalidSpaceId(format!(
            "Expected 16 bytes, got {}",
            space_id_bytes.len()
        )));
    }
    let bytes: [u8; 16] = space_id_bytes.try_into().map_err(|_| {
        HandlerError::InvalidSpaceId("Failed to convert bytes to array".to_string())
    })?;
    Ok(Uuid::from_bytes(bytes))
}

fn extract_entities(edit: &Grc20Edit, _space_id: &Uuid, meta: &EditMetadata) -> Vec<EntityItem> {
    let mut entities = Vec::new();
    let mut seen = HashSet::new();

    for op in &edit.ops {
        let ids_to_add: Vec<Uuid> = match op {
            Grc20Op::CreateEntity(entity) => {
                let mut ids = vec![id_to_uuid(&entity.id)];
                // Add property IDs as entities
                for pv in &entity.values {
                    ids.push(id_to_uuid(&pv.property));
                }
                ids
            }
            Grc20Op::UpdateEntity(entity) => {
                let mut ids = vec![id_to_uuid(&entity.id)];
                // Add property IDs from set values
                for pv in &entity.set_properties {
                    ids.push(id_to_uuid(&pv.property));
                }
                ids
            }
            Grc20Op::CreateRelation(relation) => {
                let mut ids = vec![
                    id_to_uuid(&relation.id),
                    id_to_uuid(&relation.relation_type),
                    id_to_uuid(&relation.from),
                    id_to_uuid(&relation.to),
                ];
                // Add the reified entity ID
                ids.push(id_to_uuid(&relation.entity_id()));
                ids
            }
            Grc20Op::DeleteRelation(del) => {
                vec![id_to_uuid(&del.id)]
            }
            _ => vec![],
        };

        for id in ids_to_add {
            if !seen.contains(&id) {
                entities.push(EntityItem {
                    id,
                    created_at: meta.timestamp.clone(),
                    created_at_block: meta.block_number.clone(),
                    updated_at: meta.timestamp.clone(),
                    updated_at_block: meta.block_number.clone(),
                });
                seen.insert(id);
            }
        }
    }

    entities
}

/// Convert a grc_20::Value to a ValueOp with appropriate fields set
fn value_to_value_op(
    pv: &PropertyValue,
    entity_id: Uuid,
    space_id: Uuid,
    context: &Option<Context>,
) -> Option<ValueOp> {
    let property_id = id_to_uuid(&pv.property);
    let value_id = derive_value_id(&entity_id, &property_id, &space_id);
    let (context_root_id, context_edge_type_id) = extract_context(context);

    let mut op = ValueOp {
        id: value_id,
        change_type: ValueChangeType::Set,
        entity_id,
        property_id,
        space_id,
        language: None,
        unit: None,
        text: None,
        decimal: None,
        boolean: None,
        time: None,
        point: None,
        rect: None,
        integer: None,
        float: None,
        bytes: None,
        date: None,
        datetime: None,
        schedule: None,
        embedding: None,
        time_utc: None,
        datetime_utc: None,
        context_root_id,
        context_edge_type_id,
    };

    match &pv.value {
        Grc20Value::Boolean(v) => {
            op.boolean = Some(*v);
        }
        Grc20Value::Integer { value, unit } => {
            op.integer = Some(*value);
            if let Some(unit_id) = unit {
                op.unit = Some(id_to_uuid(unit_id).to_string());
            }
        }
        Grc20Value::Float { value, unit } => {
            op.float = Some(*value);
            if let Some(unit_id) = unit {
                op.unit = Some(id_to_uuid(unit_id).to_string());
            }
        }
        Grc20Value::Decimal {
            exponent,
            mantissa,
            unit,
        } => {
            // Convert decimal to string representation for storage
            let decimal_str = format_decimal_mantissa(mantissa, *exponent);
            op.decimal = Some(decimal_str);
            if let Some(unit_id) = unit {
                op.unit = Some(id_to_uuid(unit_id).to_string());
            }
        }
        Grc20Value::Text { value, language } => {
            op.text = Some(value.to_string());
            if let Some(lang_id) = language {
                op.language = Some(id_to_uuid(lang_id).to_string());
            }
        }
        Grc20Value::Bytes(v) => {
            op.bytes = Some(v.to_vec());
        }
        Grc20Value::Date(value) => {
            op.date = Some(value.to_string());
        }
        Grc20Value::Time(value) => {
            let time_str = value.to_string();
            op.time = Some(time_str.clone());
            op.time_utc = Some(time_str);
        }
        Grc20Value::Datetime(value) => {
            let datetime_str = value.to_string();
            op.datetime = Some(datetime_str.clone());
            op.datetime_utc = Some(datetime_str);
        }
        Grc20Value::Schedule(v) => {
            // Store as JSON string
            op.schedule = Some(serde_json::Value::String(v.to_string()));
        }
        Grc20Value::Point { lon, lat, alt } => {
            // Format as "lat,lon" or "lat,lon,alt"
            let point_str = match alt {
                Some(a) => format!("{},{},{}", lat, lon, a),
                None => format!("{},{}", lat, lon),
            };
            op.point = Some(point_str);
        }
        Grc20Value::Rect {
            min_lat,
            min_lon,
            max_lat,
            max_lon,
        } => {
            // Format as "min_lat,min_lon,max_lat,max_lon" (southwest corner, then northeast corner)
            let rect_str = format!("{},{},{},{}", min_lat, min_lon, max_lat, max_lon);
            op.rect = Some(rect_str);
        }
        Grc20Value::Embedding {
            sub_type,
            dims,
            data,
        } => {
            // Store embedding as JSON with metadata
            let embedding_json = serde_json::json!({
                "sub_type": format!("{:?}", sub_type),
                "dims": dims,
                "data": hex::encode(data.as_ref()),
            });
            op.embedding = Some(embedding_json);
        }
    }

    Some(op)
}

/// Format a DecimalMantissa with exponent as a decimal string
fn format_decimal_mantissa(mantissa: &grc_20::DecimalMantissa, exponent: i32) -> String {
    use grc_20::DecimalMantissa;

    match mantissa {
        DecimalMantissa::I64(val) => format_decimal_i64(*val, exponent),
        DecimalMantissa::Big(bytes) => {
            // For big integers, just store as hex for now
            // TODO: Convert big-endian two's complement to decimal string
            format!("0x{}", hex::encode(bytes.as_ref()))
        }
    }
}

/// Format an i64 mantissa with exponent as a decimal string
fn format_decimal_i64(mantissa: i64, exponent: i32) -> String {
    if exponent >= 0 {
        // Positive exponent: multiply by 10^exponent
        let multiplier = 10i64.pow(exponent as u32);
        match mantissa.checked_mul(multiplier) {
            Some(result) => result.to_string(),
            None => format!("{}e{}", mantissa, exponent), // Fallback to scientific notation
        }
    } else {
        // Negative exponent: divide by 10^|exponent|
        let abs_exp = (-exponent) as usize;
        let mantissa_str = mantissa.abs().to_string();
        let sign = if mantissa < 0 { "-" } else { "" };

        if abs_exp >= mantissa_str.len() {
            // Need leading zeros after decimal point
            let zeros = abs_exp - mantissa_str.len();
            format!("{}0.{}{}", sign, "0".repeat(zeros), mantissa_str)
        } else {
            // Insert decimal point within the number
            let decimal_pos = mantissa_str.len() - abs_exp;
            format!(
                "{}{}.{}",
                sign,
                &mantissa_str[..decimal_pos],
                &mantissa_str[decimal_pos..]
            )
        }
    }
}

fn extract_values(edit: &Grc20Edit, space_id: &Uuid) -> Vec<ValueOp> {
    let mut value_ops = Vec::new();

    for op in &edit.ops {
        match op {
            Grc20Op::CreateEntity(entity) => {
                let entity_id = id_to_uuid(&entity.id);
                for pv in &entity.values {
                    if let Some(value_op) =
                        value_to_value_op(pv, entity_id, *space_id, &entity.context)
                    {
                        value_ops.push(value_op);
                    }
                }
            }
            Grc20Op::UpdateEntity(entity) => {
                let entity_id = id_to_uuid(&entity.id);
                let (context_root_id, context_edge_type_id) = extract_context(&entity.context);

                // Handle unset values first
                for unset in &entity.unset_values {
                    let property_id = id_to_uuid(&unset.property);
                    let value_id = derive_value_id(&entity_id, &property_id, space_id);

                    value_ops.push(ValueOp {
                        id: value_id,
                        change_type: ValueChangeType::Delete,
                        entity_id,
                        property_id,
                        space_id: *space_id,
                        language: None,
                        unit: None,
                        text: None,
                        decimal: None,
                        boolean: None,
                        time: None,
                        point: None,
                        rect: None,
                        integer: None,
                        float: None,
                        bytes: None,
                        date: None,
                        datetime: None,
                        schedule: None,
                        embedding: None,
                        time_utc: None,
                        datetime_utc: None,
                        context_root_id,
                        context_edge_type_id,
                    });
                }

                // Handle set values
                for pv in &entity.set_properties {
                    if let Some(value_op) =
                        value_to_value_op(pv, entity_id, *space_id, &entity.context)
                    {
                        value_ops.push(value_op);
                    }
                }
            }
            _ => {}
        }
    }

    value_ops
}

fn extract_relations(edit: &Grc20Edit, space_id: &Uuid) -> Vec<RelationOp> {
    let mut relation_ops = Vec::new();

    for op in &edit.ops {
        match op {
            Grc20Op::CreateRelation(relation) => {
                let relation_id = id_to_uuid(&relation.id);
                let entity_id = id_to_uuid(&relation.entity_id());
                let type_id = id_to_uuid(&relation.relation_type);
                let from_id = id_to_uuid(&relation.from);
                let to_id = id_to_uuid(&relation.to);

                let from_space = relation.from_space.map(|id| id_to_uuid(&id).to_string());
                let from_version = relation.from_version.map(|id| id_to_uuid(&id).to_string());
                let to_space = relation.to_space.map(|id| id_to_uuid(&id).to_string());
                let to_version = relation.to_version.map(|id| id_to_uuid(&id).to_string());
                let (context_root_id, context_edge_type_id) = extract_context(&relation.context);

                relation_ops.push(RelationOp::Create(SetRelationItem {
                    id: relation_id,
                    entity_id,
                    space_id: *space_id,
                    position: relation.position.as_ref().map(|s| s.to_string()),
                    type_id,
                    from_id,
                    from_space_id: from_space,
                    from_version_id: from_version,
                    to_id,
                    to_space_id: to_space,
                    to_version_id: to_version,
                    verified: None, // v2 doesn't have verified field on CreateRelation
                    is_system: false,
                    context_root_id,
                    context_edge_type_id,
                }));
            }
            Grc20Op::UpdateRelation(updated) => {
                let relation_id = id_to_uuid(&updated.id);

                let from_space = updated.from_space.map(|id| id_to_uuid(&id).to_string());
                let from_version = updated.from_version.map(|id| id_to_uuid(&id).to_string());
                let to_space = updated.to_space.map(|id| id_to_uuid(&id).to_string());
                let to_version = updated.to_version.map(|id| id_to_uuid(&id).to_string());
                let (context_root_id, context_edge_type_id) = extract_context(&updated.context);

                // Check if any fields are being unset
                let has_unset = !updated.unset.is_empty();

                if has_unset {
                    // Convert unset fields to our UnsetRelationItem
                    relation_ops.push(RelationOp::Unset(UnsetRelationItem {
                        id: relation_id,
                        space_id: *space_id,
                        from_space_id: updated
                            .unset
                            .contains(&UnsetRelationField::FromSpace)
                            .then_some(true),
                        from_version_id: updated
                            .unset
                            .contains(&UnsetRelationField::FromVersion)
                            .then_some(true),
                        to_space_id: updated
                            .unset
                            .contains(&UnsetRelationField::ToSpace)
                            .then_some(true),
                        to_version_id: updated
                            .unset
                            .contains(&UnsetRelationField::ToVersion)
                            .then_some(true),
                        position: updated
                            .unset
                            .contains(&UnsetRelationField::Position)
                            .then_some(true),
                        verified: None,
                        context_root_id,
                        context_edge_type_id,
                    }));
                }

                // If there are any set fields, emit an Update op
                if from_space.is_some()
                    || from_version.is_some()
                    || to_space.is_some()
                    || to_version.is_some()
                    || updated.position.is_some()
                {
                    relation_ops.push(RelationOp::Update(UpdateRelationItem {
                        id: relation_id,
                        space_id: *space_id,
                        position: updated.position.as_ref().map(|s| s.to_string()),
                        verified: None,
                        to_space_id: to_space,
                        from_space_id: from_space,
                        from_version_id: from_version,
                        to_version_id: to_version,
                        context_root_id,
                        context_edge_type_id,
                    }));
                }
            }
            Grc20Op::DeleteRelation(del) => {
                // Delete-with-context tombstone is not yet implemented; see
                // DeleteRelationItem doc comment.
                relation_ops.push(RelationOp::Delete(DeleteRelationItem {
                    id: id_to_uuid(&del.id),
                    space_id: *space_id,
                }));
            }
            _ => {}
        }
    }

    relation_ops
}

pub(crate) fn derive_value_id(entity_id: &Uuid, property_id: &Uuid, space_id: &Uuid) -> Uuid {
    let mut hasher = DefaultHasher::new();
    entity_id.hash(&mut hasher);
    property_id.hash(&mut hasher);
    space_id.hash(&mut hasher);
    let hash_value = hasher.finish();

    let mut bytes = [0u8; 16];
    bytes[0..8].copy_from_slice(&hash_value.to_be_bytes());
    bytes[8..16].copy_from_slice(&hash_value.to_be_bytes());

    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_value_op(id: Uuid, change_type: ValueChangeType, value: Option<String>) -> ValueOp {
        ValueOp {
            id,
            change_type,
            entity_id: Uuid::new_v4(),
            property_id: Uuid::new_v4(),
            space_id: Uuid::new_v4(),
            language: None,
            unit: None,
            text: value,
            decimal: None,
            boolean: None,
            time: None,
            point: None,
            rect: None,
            integer: None,
            float: None,
            bytes: None,
            date: None,
            datetime: None,
            schedule: None,
            embedding: None,
            time_utc: None,
            datetime_utc: None,
            context_root_id: None,
            context_edge_type_id: None,
        }
    }

    fn make_create_relation(id: Uuid) -> SetRelationItem {
        SetRelationItem {
            id,
            entity_id: Uuid::new_v4(),
            type_id: Uuid::new_v4(),
            from_id: Uuid::new_v4(),
            to_id: Uuid::new_v4(),
            space_id: Uuid::new_v4(),
            from_space_id: None,
            from_version_id: None,
            to_space_id: None,
            to_version_id: None,
            position: None,
            verified: None,
            is_system: false,
            context_root_id: None,
            context_edge_type_id: None,
        }
    }

    fn make_update_relation(id: Uuid, space_id: Uuid) -> UpdateRelationItem {
        UpdateRelationItem {
            id,
            space_id,
            from_space_id: None,
            from_version_id: None,
            to_space_id: None,
            to_version_id: None,
            position: None,
            verified: None,
            context_root_id: None,
            context_edge_type_id: None,
        }
    }

    fn make_unset_relation(id: Uuid, space_id: Uuid) -> UnsetRelationItem {
        UnsetRelationItem {
            id,
            space_id,
            from_space_id: None,
            from_version_id: None,
            to_space_id: None,
            to_version_id: None,
            position: None,
            verified: None,
            context_root_id: None,
            context_edge_type_id: None,
        }
    }

    fn make_delete_relation(id: Uuid, space_id: Uuid) -> DeleteRelationItem {
        DeleteRelationItem { id, space_id }
    }

    // ===================
    // Value squashing tests
    // ===================

    #[test]
    fn test_squash_values_empty() {
        let ops: Vec<ValueOp> = vec![];
        let result = squash_values(&ops);
        assert!(result.is_empty());
    }

    #[test]
    fn test_squash_values_single_set() {
        let id = Uuid::new_v4();
        let ops = vec![make_value_op(
            id,
            ValueChangeType::Set,
            Some("hello".into()),
        )];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].change_type, ValueChangeType::Set));
    }

    #[test]
    fn test_squash_values_set_then_delete_same_id() {
        let id = Uuid::new_v4();
        let ops = vec![
            make_value_op(id, ValueChangeType::Set, Some("hello".into())),
            make_value_op(id, ValueChangeType::Delete, None),
        ];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].change_type, ValueChangeType::Delete));
    }

    #[test]
    fn test_squash_values_delete_then_set_same_id() {
        let id = Uuid::new_v4();
        let ops = vec![
            make_value_op(id, ValueChangeType::Delete, None),
            make_value_op(id, ValueChangeType::Set, Some("recreated".into())),
        ];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].change_type, ValueChangeType::Set));
        assert_eq!(result[0].text, Some("recreated".into()));
    }

    #[test]
    fn test_squash_values_multiple_sets_same_id() {
        let id = Uuid::new_v4();
        let ops = vec![
            make_value_op(id, ValueChangeType::Set, Some("first".into())),
            make_value_op(id, ValueChangeType::Set, Some("second".into())),
            make_value_op(id, ValueChangeType::Set, Some("third".into())),
        ];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 1);
        // Last one wins
        assert_eq!(result[0].text, Some("third".into()));
    }

    #[test]
    fn test_squash_values_different_ids_preserved() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let ops = vec![
            make_value_op(id1, ValueChangeType::Set, Some("value1".into())),
            make_value_op(id2, ValueChangeType::Set, Some("value2".into())),
        ];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_squash_values_mixed_operations() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let id3 = Uuid::new_v4();
        let ops = vec![
            make_value_op(id1, ValueChangeType::Set, Some("v1".into())),
            make_value_op(id2, ValueChangeType::Set, Some("v2".into())),
            make_value_op(id1, ValueChangeType::Delete, None), // id1 gets deleted
            make_value_op(id3, ValueChangeType::Set, Some("v3".into())),
            make_value_op(id2, ValueChangeType::Set, Some("v2-updated".into())), // id2 updated
        ];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 3);

        let id1_op = result.iter().find(|op| op.id == id1).unwrap();
        assert!(matches!(id1_op.change_type, ValueChangeType::Delete));

        let id2_op = result.iter().find(|op| op.id == id2).unwrap();
        assert_eq!(id2_op.text, Some("v2-updated".into()));

        let id3_op = result.iter().find(|op| op.id == id3).unwrap();
        assert_eq!(id3_op.text, Some("v3".into()));
    }

    // ===================
    // Relation squashing tests
    // ===================

    #[test]
    fn test_squash_relations_empty() {
        let ops: Vec<RelationOp> = vec![];
        let result = squash_relations(&ops);
        assert!(result.is_empty());
    }

    #[test]
    fn test_squash_relations_single_create() {
        let id = Uuid::new_v4();
        let ops = vec![RelationOp::Create(make_create_relation(id))];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], RelationOp::Create(_)));
    }

    #[test]
    fn test_squash_relations_create_then_delete() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let mut create = make_create_relation(id);
        create.space_id = space_id;
        let ops = vec![
            RelationOp::Create(create),
            RelationOp::Delete(make_delete_relation(id, space_id)),
        ];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], RelationOp::Delete(_)));
    }

    #[test]
    fn test_squash_relations_delete_then_create() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let ops = vec![
            RelationOp::Delete(make_delete_relation(id, space_id)),
            RelationOp::Create(make_create_relation(id)),
        ];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], RelationOp::Create(_)));
    }

    #[test]
    fn test_squash_relations_create_then_update_merges_fields() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut create = make_create_relation(id);
        create.space_id = space_id;
        create.position = Some("original".into());

        let mut update = make_update_relation(id, space_id);
        update.position = Some("updated".into());
        update.verified = Some(true);

        let ops = vec![RelationOp::Create(create), RelationOp::Update(update)];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);

        if let RelationOp::Create(r) = &result[0] {
            assert_eq!(r.position, Some("updated".into()));
            assert_eq!(r.verified, Some(true));
        } else {
            panic!("Expected Create variant");
        }
    }

    #[test]
    fn test_squash_relations_update_then_update_merges_fields() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut update1 = make_update_relation(id, space_id);
        update1.position = Some("pos1".into());
        update1.from_space_id = Some("from_space".into());

        let mut update2 = make_update_relation(id, space_id);
        update2.position = Some("pos2".into());
        update2.to_space_id = Some("to_space".into());

        let ops = vec![RelationOp::Update(update1), RelationOp::Update(update2)];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);

        if let RelationOp::Update(r) = &result[0] {
            assert_eq!(r.position, Some("pos2".into())); // Second update wins
            assert_eq!(r.from_space_id, Some("from_space".into())); // Preserved from first
            assert_eq!(r.to_space_id, Some("to_space".into())); // From second
        } else {
            panic!("Expected Update variant");
        }
    }

    #[test]
    fn test_squash_relations_update_then_delete() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let update = make_update_relation(id, space_id);
        let ops = vec![
            RelationOp::Update(update),
            RelationOp::Delete(make_delete_relation(id, space_id)),
        ];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], RelationOp::Delete(_)));
    }

    #[test]
    fn test_squash_relations_create_then_unset_clears_fields() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut create = make_create_relation(id);
        create.space_id = space_id;
        create.position = Some("has_position".into());
        create.verified = Some(true);

        let mut unset = make_unset_relation(id, space_id);
        unset.position = Some(true); // Unset position
        unset.verified = Some(false); // Don't unset verified

        let ops = vec![RelationOp::Create(create), RelationOp::Unset(unset)];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);

        if let RelationOp::Create(r) = &result[0] {
            assert_eq!(r.position, None); // Was unset
            assert_eq!(r.verified, Some(true)); // Was preserved
        } else {
            panic!("Expected Create variant");
        }
    }

    #[test]
    fn test_squash_relations_unset_then_unset_merges() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut unset1 = make_unset_relation(id, space_id);
        unset1.position = Some(true);

        let mut unset2 = make_unset_relation(id, space_id);
        unset2.verified = Some(true);

        let ops = vec![RelationOp::Unset(unset1), RelationOp::Unset(unset2)];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);

        if let RelationOp::Unset(r) = &result[0] {
            assert_eq!(r.position, Some(true));
            assert_eq!(r.verified, Some(true));
        } else {
            panic!("Expected Unset variant");
        }
    }

    #[test]
    fn test_squash_relations_unset_then_create_overwrites() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let unset = make_unset_relation(id, space_id);
        let create = make_create_relation(id);

        let ops = vec![RelationOp::Unset(unset), RelationOp::Create(create)];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], RelationOp::Create(_)));
    }

    #[test]
    fn test_squash_relations_different_ids_preserved() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        let ops = vec![
            RelationOp::Create(make_create_relation(id1)),
            RelationOp::Create(make_create_relation(id2)),
        ];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_squash_relations_complex_sequence() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut create1 = make_create_relation(id1);
        create1.space_id = space_id;

        let mut create2 = make_create_relation(id2);
        create2.space_id = space_id;

        let mut update1 = make_update_relation(id1, space_id);
        update1.position = Some("pos".into());

        let ops = vec![
            RelationOp::Create(create1),
            RelationOp::Create(create2),
            RelationOp::Update(update1),
            RelationOp::Delete(make_delete_relation(id2, space_id)), // id2 created then deleted
        ];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 2);

        // id1 should be Create with merged position
        let id1_op = result.iter().find(|op| op.id() == id1).unwrap();
        if let RelationOp::Create(r) = id1_op {
            assert_eq!(r.position, Some("pos".into()));
        } else {
            panic!("Expected Create for id1");
        }

        // id2 should be Delete
        let id2_op = result.iter().find(|op| op.id() == id2).unwrap();
        assert!(matches!(id2_op, RelationOp::Delete(_)));
    }

    // ===================
    // Context extraction tests
    // ===================

    #[test]
    fn test_extract_context_none() {
        let (root_id, edge_type_id) = extract_context(&None);
        assert!(root_id.is_none());
        assert!(edge_type_id.is_none());
    }

    #[test]
    fn test_extract_context_with_edges() {
        let root_id: [u8; 16] = [1; 16];
        let edge_type_id: [u8; 16] = [2; 16];
        let to_entity_id: [u8; 16] = [3; 16];

        let context = Context {
            root_id,
            edges: vec![ContextEdge {
                type_id: edge_type_id,
                to_entity_id,
            }],
        };

        let (extracted_root, extracted_edge_type) = extract_context(&Some(context));

        assert_eq!(extracted_root, Some(Uuid::from_bytes(root_id)));
        assert_eq!(extracted_edge_type, Some(Uuid::from_bytes(edge_type_id)));
    }

    #[test]
    fn test_extract_context_empty_edges() {
        let root_id: [u8; 16] = [1; 16];

        let context = Context {
            root_id,
            edges: vec![],
        };

        let (extracted_root, extracted_edge_type) = extract_context(&Some(context));

        assert_eq!(extracted_root, Some(Uuid::from_bytes(root_id)));
        assert!(extracted_edge_type.is_none());
    }

    // ===================
    // Protection filter tests
    // ===================

    fn make_value_op_with_property(property_id: Uuid) -> ValueOp {
        ValueOp {
            id: Uuid::new_v4(),
            change_type: ValueChangeType::Set,
            entity_id: Uuid::new_v4(),
            property_id,
            space_id: Uuid::new_v4(),
            language: None,
            unit: None,
            text: Some("test".into()),
            decimal: None,
            boolean: None,
            time: None,
            point: None,
            rect: None,
            integer: None,
            float: None,
            bytes: None,
            date: None,
            datetime: None,
            schedule: None,
            embedding: None,
            time_utc: None,
            datetime_utc: None,
            context_root_id: None,
            context_edge_type_id: None,
        }
    }

    fn make_create_relation_with_ids(type_id: Uuid, from_id: Uuid, to_id: Uuid) -> SetRelationItem {
        SetRelationItem {
            id: Uuid::new_v4(),
            entity_id: Uuid::new_v4(),
            type_id,
            from_id,
            to_id,
            space_id: Uuid::new_v4(),
            from_space_id: None,
            from_version_id: None,
            to_space_id: None,
            to_version_id: None,
            position: None,
            verified: None,
            is_system: false,
            context_root_id: None,
            context_edge_type_id: None,
        }
    }

    #[test]
    fn filter_values_drops_protected_property() {
        let protected = Uuid::parse_str(sdk::core::ids::SPACE_ADDRESS_PROPERTY_ID).unwrap();
        let ops = vec![make_value_op_with_property(protected)];
        let result = filter_protected_values(ops);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_values_drops_score_property() {
        let protected = Uuid::parse_str(sdk::core::ids::SCORE_PROPERTY_ID).unwrap();
        let ops = vec![make_value_op_with_property(protected)];
        let result = filter_protected_values(ops);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_values_mixed_batch_drops_score_keeps_normal() {
        let score = Uuid::parse_str(sdk::core::ids::SCORE_PROPERTY_ID).unwrap();
        let normal = Uuid::parse_str(sdk::core::ids::NAME_PROPERTY_ID).unwrap();
        let ops = vec![
            make_value_op_with_property(score),
            make_value_op_with_property(normal),
            make_value_op_with_property(score),
        ];
        let result = filter_protected_values(ops);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].property_id, normal);
    }

    #[test]
    fn filter_relations_drops_score_as_from_id() {
        let score = Uuid::parse_str(sdk::core::ids::SCORE_PROPERTY_ID).unwrap();
        let ops = vec![RelationOp::Create(make_create_relation_with_ids(
            Uuid::new_v4(),
            score,
            Uuid::new_v4(),
        ))];
        let result = filter_protected_relations(ops, &NON_ROOT_SPACE_ID);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_relations_drops_score_as_to_id() {
        let score = Uuid::parse_str(sdk::core::ids::SCORE_PROPERTY_ID).unwrap();
        let ops = vec![RelationOp::Create(make_create_relation_with_ids(
            Uuid::new_v4(),
            Uuid::new_v4(),
            score,
        ))];
        let result = filter_protected_relations(ops, &NON_ROOT_SPACE_ID);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_values_keeps_normal_property() {
        let normal = Uuid::parse_str(sdk::core::ids::NAME_PROPERTY_ID).unwrap();
        let ops = vec![make_value_op_with_property(normal)];
        let result = filter_protected_values(ops);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn filter_values_mixed_batch_keeps_only_normal() {
        let protected1 = Uuid::parse_str(sdk::core::ids::SPACE_ADDRESS_PROPERTY_ID).unwrap();
        let protected2 = Uuid::parse_str(sdk::core::ids::VOTING_MODE_PROPERTY_ID).unwrap();
        let normal = Uuid::parse_str(sdk::core::ids::NAME_PROPERTY_ID).unwrap();
        let ops = vec![
            make_value_op_with_property(protected1),
            make_value_op_with_property(normal),
            make_value_op_with_property(protected2),
        ];
        let result = filter_protected_values(ops);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].property_id, normal);
    }

    /// Arbitrary non-root space ID for tests that exercise the
    /// "publishing space is not root" branch of `filter_protected_relations`.
    /// Must NOT collide with `ROOT_SPACE_UUID`.
    const NON_ROOT_SPACE_ID: Uuid = Uuid::from_bytes([1; 16]);

    #[test]
    fn filter_relations_drops_protected_type() {
        let protected_type =
            Uuid::parse_str(sdk::core::ids::SYSTEM_TYPES_RELATION_TYPE_ID).unwrap();
        let ops = vec![RelationOp::Create(make_create_relation_with_ids(
            protected_type,
            Uuid::new_v4(),
            Uuid::new_v4(),
        ))];
        let result = filter_protected_relations(ops, &NON_ROOT_SPACE_ID);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_relations_drops_protected_from_id() {
        let protected_entity = Uuid::parse_str(sdk::core::ids::VOTING_MODE_PROPERTY_ID).unwrap();
        let ops = vec![RelationOp::Create(make_create_relation_with_ids(
            Uuid::new_v4(),
            protected_entity,
            Uuid::new_v4(),
        ))];
        let result = filter_protected_relations(ops, &NON_ROOT_SPACE_ID);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_relations_drops_protected_to_id() {
        let protected_entity = Uuid::parse_str(sdk::core::ids::SPACE_ID_PROPERTY_ID).unwrap();
        let ops = vec![RelationOp::Create(make_create_relation_with_ids(
            Uuid::new_v4(),
            Uuid::new_v4(),
            protected_entity,
        ))];
        let result = filter_protected_relations(ops, &NON_ROOT_SPACE_ID);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_relations_keeps_normal_create() {
        let ops = vec![RelationOp::Create(make_create_relation_with_ids(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        ))];
        let result = filter_protected_relations(ops, &NON_ROOT_SPACE_ID);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn filter_relations_passes_through_update_and_delete() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let ops = vec![
            RelationOp::Update(make_update_relation(id, space_id)),
            RelationOp::Delete(make_delete_relation(id, space_id)),
            RelationOp::Unset(make_unset_relation(id, space_id)),
        ];
        let result = filter_protected_relations(ops, &NON_ROOT_SPACE_ID);
        assert_eq!(result.len(), 3);
    }

    // ===================
    // Root-space gating tests
    // ===================

    #[test]
    fn root_space_can_create_relation_with_protected_from() {
        // Seed-edit shape: `SpaceAddress -types-> <Property>` from root.
        // `from` is protected, but `type` is not in PROTECTED_RELATION_TYPES
        // and we are publishing from the root space, so the relation is kept.
        let protected_from = Uuid::parse_str(sdk::core::ids::SPACE_ADDRESS_PROPERTY_ID).unwrap();
        let unprotected_type = Uuid::parse_str(sdk::core::ids::TYPE_RELATION_TYPE_ID).unwrap();
        let ops = vec![RelationOp::Create(make_create_relation_with_ids(
            unprotected_type,
            protected_from,
            Uuid::new_v4(),
        ))];
        let result = filter_protected_relations(ops, &ROOT_SPACE_UUID);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn root_space_can_create_relation_with_protected_to() {
        // Symmetric: `<something> -<unprotected>-> VotingMode` from root.
        let protected_to = Uuid::parse_str(sdk::core::ids::VOTING_MODE_PROPERTY_ID).unwrap();
        let unprotected_type = Uuid::parse_str(sdk::core::ids::TYPE_RELATION_TYPE_ID).unwrap();
        let ops = vec![RelationOp::Create(make_create_relation_with_ids(
            unprotected_type,
            Uuid::new_v4(),
            protected_to,
        ))];
        let result = filter_protected_relations(ops, &ROOT_SPACE_UUID);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn root_space_still_blocked_from_system_types_relation() {
        // Type-level protection is unconditional. Even root cannot author
        // SYSTEM_TYPES via the edit pipeline — that relation type is owned
        // by `map_space_registered`.
        let protected_type =
            Uuid::parse_str(sdk::core::ids::SYSTEM_TYPES_RELATION_TYPE_ID).unwrap();
        let ops = vec![RelationOp::Create(make_create_relation_with_ids(
            protected_type,
            Uuid::new_v4(),
            Uuid::new_v4(),
        ))];
        let result = filter_protected_relations(ops, &ROOT_SPACE_UUID);
        assert!(result.is_empty());
    }

    #[test]
    fn root_space_still_blocked_from_protected_value() {
        // `filter_protected_values` is space-agnostic — values for protected
        // properties are managed by `map_space_registered`, not the edit
        // pipeline. Confirm the value filter did NOT get relaxed alongside
        // the relation change.
        let protected = Uuid::parse_str(sdk::core::ids::SPACE_ADDRESS_PROPERTY_ID).unwrap();
        let ops = vec![make_value_op_with_property(protected)];
        let result = filter_protected_values(ops);
        assert!(result.is_empty());
    }

    #[test]
    fn seed_edit_smoke_root_space_passes_property_schema() {
        let property = Uuid::parse_str(sdk::core::ids::SPACE_ADDRESS_PROPERTY_ID).unwrap();
        let type_rel_type = Uuid::parse_str(sdk::core::ids::TYPE_RELATION_TYPE_ID).unwrap();

        // SDK doesn't export constants for these — use deterministic
        // placeholders. Test asserts gating behavior, not endpoint UUIDs.
        let property_type = Uuid::from_bytes([0xAA; 16]); // "Property" type entity
        let data_type_rel = Uuid::from_bytes([0xBB; 16]); // `DataType` relation type
        let text_type = Uuid::from_bytes([0xCC; 16]); // `Text` data type entity

        let ops = vec![
            // `<property> -types-> Property`
            RelationOp::Create(make_create_relation_with_ids(
                type_rel_type,
                property,
                property_type,
            )),
            // `<property> -DataType-> Text`
            RelationOp::Create(make_create_relation_with_ids(
                data_type_rel,
                property,
                text_type,
            )),
        ];

        let from_root = filter_protected_relations(ops.clone(), &ROOT_SPACE_UUID);
        assert_eq!(from_root.len(), 2, "root space must pass schema relations");

        let from_other = filter_protected_relations(ops, &NON_ROOT_SPACE_ID);
        assert!(
            from_other.is_empty(),
            "non-root space must not author relations whose endpoints are protected",
        );
    }

    #[test]
    fn test_extract_context_multiple_edges_uses_first() {
        let root_id: [u8; 16] = [1; 16];
        let first_edge_type: [u8; 16] = [2; 16];
        let second_edge_type: [u8; 16] = [3; 16];

        let context = Context {
            root_id,
            edges: vec![
                ContextEdge {
                    type_id: first_edge_type,
                    to_entity_id: [4; 16],
                },
                ContextEdge {
                    type_id: second_edge_type,
                    to_entity_id: [5; 16],
                },
            ],
        };

        let (_, extracted_edge_type) = extract_context(&Some(context));

        // Should use first edge's type_id
        assert_eq!(extracted_edge_type, Some(Uuid::from_bytes(first_edge_type)));
    }
}
