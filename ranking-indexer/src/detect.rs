//! Detection of rank-relevant operations within a decoded GRC-20 edit.
//!
//! The indexer keeps only the four op patterns from the design (§5) and
//! discards everything else:
//!   - `CreateEntity` typed `Ranking Block`  -> a [`RankingBlock`]
//!   - `CreateEntity` typed `Rank`           -> a [`Ranking`] (+ its items)
//!   - `RANK_VOTES` relations                -> [`RankingItem`] rows
//!   - `CreateRelation` of type `RANK_BLOCK`  -> a rank -> block link
//!
//! Type membership is read from the `TYPES` relation, matching how the rest of
//! gaia resolves entity types. The relevant system IDs live in
//! [`sdk::core::ids`] (`RANK_TYPE_ID`, `RANKING_BLOCK_TYPE_ID`,
//! `RANK_VOTES_RELATION_TYPE_ID`, `RANK_BLOCK_RELATION_TYPE_ID`, …).

use uuid::Uuid;

use crate::models::{Ranking, RankingBlock, RankingItem};

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
/// TODO(ranking-indexer): implement the op-pattern matching against
/// `grc_20::Op` variants, mirroring the kg-indexer's `extract_entities` /
/// `extract_values` / `extract_relations` (kg-indexer/src/handlers/edits.rs).
/// This is the focused next step: it needs the grc_20 op API and the
/// `TYPES`-relation type-resolution pattern. Returning an empty result keeps
/// the pipeline correct (a no-op) until the matching lands.
pub fn detect(edit: &grc_20::Edit, _space_id: Uuid) -> DetectedEdit {
    tracing::debug!(
        op_count = edit.ops.len(),
        "ranking-indexer: detect() not yet wired — skipping rank op extraction"
    );
    DetectedEdit::default()
}
