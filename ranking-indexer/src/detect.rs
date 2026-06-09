//! Detection of rank-relevant operations within a decoded GRC-20 edit.
//!
//! The indexer keeps only the four op patterns from the design (§5) and
//! discards everything else:
//!   - `CreateEntity` typed `Rank`           -> a [`Ranking`]
//!   - `CreateEntity` typed `Ranking Block`  -> a [`RankingBlock`]
//!   - `RANK_VOTES` relations                -> [`RankingItem`] rows
//!   - `CreateRelation` of type `RANK_BLOCK`  -> a rank -> block link
//!
//! Type membership is read from the `TYPES` relation (a `CreateRelation` of
//! type `TYPE_RELATION_TYPE_ID` whose `to` is the type entity), matching how
//! the rest of gaia resolves entity types.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use grc_20::{Id as Grc20Id, Op as Grc20Op, PropertyValue, Value as Grc20Value};
use uuid::Uuid;

use crate::models::{Ranking, RankingBlock, RankingItem};

/// System IDs we match against, parsed once from their string constants.
struct DetectIds {
    type_relation: Uuid,
    name_property: Uuid,
    rank_type: Uuid,
    ranking_block_type: Uuid,
    rank_type_property: Uuid,
    rank_votes_relation: Uuid,
    rank_block_relation: Uuid,
    vote_weighted_value_property: Uuid,
}

static IDS: LazyLock<DetectIds> = LazyLock::new(|| {
    use sdk::core::ids::*;
    let uid = |s: &str| Uuid::parse_str(s).expect("invalid system ID constant");
    DetectIds {
        type_relation: uid(TYPE_RELATION_TYPE_ID),
        name_property: uid(NAME_PROPERTY_ID),
        rank_type: uid(RANK_TYPE_ID),
        ranking_block_type: uid(RANKING_BLOCK_TYPE_ID),
        rank_type_property: uid(RANK_TYPE_PROPERTY_ID),
        rank_votes_relation: uid(RANK_VOTES_RELATION_TYPE_ID),
        rank_block_relation: uid(RANK_BLOCK_RELATION_TYPE_ID),
        vote_weighted_value_property: uid(VOTE_WEIGHTED_VALUE_PROPERTY_ID),
    }
});

fn id_to_uuid(id: &Grc20Id) -> Uuid {
    Uuid::from_bytes(*id)
}

fn text_value(values: &[PropertyValue], property: Uuid) -> Option<String> {
    values.iter().find_map(|pv| {
        if id_to_uuid(&pv.property) != property {
            return None;
        }
        match &pv.value {
            Grc20Value::Text { value, .. } => Some(value.to_string()),
            _ => None,
        }
    })
}

fn float_value(values: &[PropertyValue], property: Uuid) -> Option<f64> {
    values.iter().find_map(|pv| {
        if id_to_uuid(&pv.property) != property {
            return None;
        }
        match &pv.value {
            Grc20Value::Float { value, .. } => Some(*value),
            _ => None,
        }
    })
}

/// The rank-relevant ops extracted from a single edit.
#[derive(Debug, Default)]
pub struct DetectedEdit {
    pub blocks: Vec<RankingBlock>,
    pub rankings: Vec<Ranking>,
    pub items: Vec<RankingItem>,
    /// `(ranking_id, block_id)` links from `RANK_BLOCK` relations.
    pub block_links: Vec<(Uuid, Uuid)>,
}

impl DetectedEdit {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
            && self.rankings.is_empty()
            && self.items.is_empty()
            && self.block_links.is_empty()
    }
}

/// Extract the rank-relevant ops from a decoded GRC-20 edit.
///
/// `space_id` is the edit's space (a Rank/Ranking Block lives in it).
/// `block_number` seeds the dedup update markers ("most recently updated rank
/// per (block, space) wins"); `update_index` is the op position within the edit.
pub fn detect(edit: &grc_20::Edit, space_id: Uuid, block_number: i64) -> DetectedEdit {
    let ids = &*IDS;
    let mut out = DetectedEdit::default();

    // Pass 1: index entity property-values and resolve TYPES membership, and
    // collect the rank relations (votes + block links).
    let mut entity_values: HashMap<Uuid, &[PropertyValue]> = HashMap::new();
    let mut types_of: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();

    for op in &edit.ops {
        match op {
            Grc20Op::CreateEntity(entity) => {
                entity_values.insert(id_to_uuid(&entity.id), entity.values.as_slice());
            }
            Grc20Op::CreateRelation(relation) => {
                let type_id = id_to_uuid(&relation.relation_type);
                if type_id == ids.type_relation {
                    types_of
                        .entry(id_to_uuid(&relation.from))
                        .or_default()
                        .insert(id_to_uuid(&relation.to));
                }
            }
            _ => {}
        }
    }

    // Pass 2: classify ops using the resolved types.
    for (op_index, op) in edit.ops.iter().enumerate() {
        match op {
            Grc20Op::CreateEntity(entity) => {
                let entity_id = id_to_uuid(&entity.id);
                let entity_types = types_of.get(&entity_id);

                let is_rank = entity_types.is_some_and(|t| t.contains(&ids.rank_type));
                let is_block =
                    entity_types.is_some_and(|t| t.contains(&ids.ranking_block_type));

                if is_rank {
                    out.rankings.push(Ranking {
                        id: entity_id,
                        block_id: None, // set when the RANK_BLOCK link is seen (here or later)
                        space_id,
                        author_address: None, // resolved from the personal space during aggregation
                        rank_type: text_value(&entity.values, ids.rank_type_property),
                        submitted_at: None, // TODO: derive from edit/block timestamp
                        updated_at_block: block_number,
                        update_index: op_index as i64,
                    });
                } else if is_block {
                    // NOTE: only id/space_id/name are confidently extractable today.
                    // The block's window dates, filter, and aggregation restriction
                    // emission shapes aren't in the merged SDK yet (geo-sdk#89 adds
                    // the IDs; block creation lands separately). Left as TODO so we
                    // don't guess the wrong op shape.
                    out.blocks.push(RankingBlock {
                        id: entity_id,
                        space_id,
                        name: text_value(&entity.values, ids.name_property),
                        filter: None,
                        start_date: None,
                        end_date: None,
                        restriction_id: None,
                    });
                }
            }
            Grc20Op::CreateRelation(relation) => {
                let type_id = id_to_uuid(&relation.relation_type);
                let from = id_to_uuid(&relation.from);

                if type_id == ids.rank_votes_relation {
                    // A ranked item: position is the fractional index, to_space the
                    // perspective, and the weighted value lives on the reified vote
                    // entity (ordinal ranks carry only the fractional index).
                    let reified = id_to_uuid(&relation.entity_id());
                    let weight = entity_values
                        .get(&reified)
                        .and_then(|vals| float_value(vals, ids.vote_weighted_value_property));
                    out.items.push(RankingItem {
                        ranking_id: from,
                        entity_id: id_to_uuid(&relation.to),
                        space_id: relation.to_space.map(|id| id_to_uuid(&id)).unwrap_or(space_id),
                        position: relation.position.as_ref().map(|p| p.to_string()),
                        weight,
                    });
                } else if type_id == ids.rank_block_relation {
                    out.block_links.push((from, id_to_uuid(&relation.to)));
                }
            }
            _ => {}
        }
    }

    out
}
