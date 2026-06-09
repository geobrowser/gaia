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

use chrono::{DateTime, Utc};
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
/// `block_timestamp` is the edit's on-chain time (Unix seconds), recorded as the
/// submission timestamp for the window check.
pub fn detect(
    edit: &grc_20::Edit,
    space_id: Uuid,
    block_number: i64,
    block_timestamp: i64,
) -> DetectedEdit {
    let ids = &*IDS;
    let submitted_at = DateTime::<Utc>::from_timestamp(block_timestamp, 0);
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
                        submitted_at,
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
                    //
                    // `to_space` is the ranked entity's perspective and is required:
                    // the SDK sets it on every vote (buildVoteOps keys uniqueness on
                    // (entityId, spaceId)), and both aggregation and the published
                    // projection key on it. It is normally *not* the ranking's own
                    // (personal) space — it's wherever the ranked entity lives. A
                    // missing one is a malformed vote we can't place in a perspective,
                    // so skip the item rather than mis-bucketing it into the ranking's
                    // space (which would create an aggregate row readers can't resolve).
                    let Some(item_space) = relation.to_space.map(|id| id_to_uuid(&id)) else {
                        tracing::warn!(
                            ranking = %from,
                            entity = %id_to_uuid(&relation.to),
                            "RANK_VOTES relation missing to_space; skipping item"
                        );
                        continue;
                    };
                    let reified = id_to_uuid(&relation.entity_id());
                    let weight = entity_values
                        .get(&reified)
                        .and_then(|vals| float_value(vals, ids.vote_weighted_value_property));
                    out.items.push(RankingItem {
                        ranking_id: from,
                        entity_id: id_to_uuid(&relation.to),
                        space_id: item_space,
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

#[cfg(test)]
mod tests {
    use super::*;
    use grc_20::model::builder::EditBuilder;
    use sdk::core::ids::*;

    /// grc_20 Id ([u8; 16]) from an arbitrary u128.
    fn gid(n: u128) -> [u8; 16] {
        *Uuid::from_u128(n).as_bytes()
    }
    /// grc_20 Id from a system-ID string constant.
    fn sid(s: &str) -> [u8; 16] {
        *Uuid::parse_str(s).unwrap().as_bytes()
    }

    const SPACE: u128 = 7000;
    const RANK: u128 = 1;
    const BLOCK: u128 = 2;
    const RANKED_ENTITY: u128 = 3;
    const VOTE_ENTITY: u128 = 4;
    const PERSPECTIVE: u128 = 8000;

    fn space_uuid() -> Uuid {
        Uuid::from_u128(SPACE)
    }

    #[test]
    fn detects_weighted_rank_with_item_and_submitted_at() {
        let edit = EditBuilder::new(gid(0))
            // entity RANK is typed `Rank` via a TYPES relation
            .create_relation(|r| {
                r.id(gid(10))
                    .relation_type(sid(TYPE_RELATION_TYPE_ID))
                    .from(gid(RANK))
                    .to(sid(RANK_TYPE_ID))
            })
            .create_entity(gid(RANK), |e| {
                e.text(sid(RANK_TYPE_PROPERTY_ID), "WEIGHTED", None)
            })
            // a RANK_VOTES item: rank -> ranked entity, with perspective + position
            // and an explicit reified vote entity
            .create_relation(|r| {
                r.id(gid(11))
                    .relation_type(sid(RANK_VOTES_RELATION_TYPE_ID))
                    .from(gid(RANK))
                    .to(gid(RANKED_ENTITY))
                    .to_space(gid(PERSPECTIVE))
                    .position("a0")
                    .entity(gid(VOTE_ENTITY))
            })
            // the reified vote entity carries the weighted value
            .create_entity(gid(VOTE_ENTITY), |e| {
                e.float(sid(VOTE_WEIGHTED_VALUE_PROPERTY_ID), 0.9, None)
            })
            .build();

        let detected = detect(&edit, space_uuid(), 100, 1_700_000_000);

        assert_eq!(detected.rankings.len(), 1);
        let r = &detected.rankings[0];
        assert_eq!(r.id, Uuid::from_u128(RANK));
        assert_eq!(r.rank_type.as_deref(), Some("WEIGHTED"));
        assert_eq!(r.space_id, space_uuid());
        assert_eq!(
            r.submitted_at,
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0)
        );

        assert_eq!(detected.items.len(), 1);
        let it = &detected.items[0];
        assert_eq!(it.ranking_id, Uuid::from_u128(RANK));
        assert_eq!(it.entity_id, Uuid::from_u128(RANKED_ENTITY));
        assert_eq!(it.space_id, Uuid::from_u128(PERSPECTIVE));
        assert_eq!(it.position.as_deref(), Some("a0"));
        assert_eq!(it.weight, Some(0.9));
    }

    #[test]
    fn detects_ranking_block() {
        let edit = EditBuilder::new(gid(0))
            .create_relation(|r| {
                r.id(gid(20))
                    .relation_type(sid(TYPE_RELATION_TYPE_ID))
                    .from(gid(BLOCK))
                    .to(sid(RANKING_BLOCK_TYPE_ID))
            })
            .create_entity(gid(BLOCK), |e| e.text(sid(NAME_PROPERTY_ID), "Top Films", None))
            .build();

        let detected = detect(&edit, space_uuid(), 100, 0);
        assert_eq!(detected.blocks.len(), 1);
        assert_eq!(detected.blocks[0].id, Uuid::from_u128(BLOCK));
        assert_eq!(detected.blocks[0].name.as_deref(), Some("Top Films"));
        assert!(detected.rankings.is_empty());
    }

    #[test]
    fn detects_rank_block_link() {
        let edit = EditBuilder::new(gid(0))
            .create_relation(|r| {
                r.id(gid(30))
                    .relation_type(sid(RANK_BLOCK_RELATION_TYPE_ID))
                    .from(gid(RANK))
                    .to(gid(BLOCK))
            })
            .build();

        let detected = detect(&edit, space_uuid(), 100, 0);
        assert_eq!(
            detected.block_links,
            vec![(Uuid::from_u128(RANK), Uuid::from_u128(BLOCK))]
        );
    }

    #[test]
    fn ignores_unrelated_ops() {
        let edit = EditBuilder::new(gid(0))
            .create_entity(gid(999), |e| {
                e.text(sid(NAME_PROPERTY_ID), "Just an entity", None)
            })
            .build();
        let detected = detect(&edit, space_uuid(), 100, 0);
        assert!(detected.is_empty());
    }

    #[test]
    fn untyped_rank_entity_is_not_detected() {
        // A Rank-shaped entity WITHOUT the TYPES relation isn't classified as a rank.
        let edit = EditBuilder::new(gid(0))
            .create_entity(gid(RANK), |e| {
                e.text(sid(RANK_TYPE_PROPERTY_ID), "ORDINAL", None)
            })
            .build();
        let detected = detect(&edit, space_uuid(), 100, 0);
        assert!(detected.rankings.is_empty());
    }
}
