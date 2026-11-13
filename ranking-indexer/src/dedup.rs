//! Deduplication: keep only the most-recently-updated submission per
//! (block, personal space). A user who submits twice to the same block counts
//! once, with the later submission superseding the earlier (design §7).

use std::collections::HashMap;
use uuid::Uuid;

use crate::models::Ranking;

/// Reduce a set of submissions to the latest per (block_id, space_id).
///
/// "Latest" is ordered by the update markers `(updated_at_block, update_index)`,
/// and on a tie the lowest ranking id wins, as in 0124's
/// `trending.ranking_member_ballots` (`ORDER BY updated_at_block DESC,
/// update_index DESC, id`). Without it a tie went to whichever row the
/// unordered read returned first, and since an empty ballot counts for nothing
/// a tie can decide whether the ranker counts at all (recovered Ranks all have
/// `update_index` 0).
/// Submissions whose `block_id` is still null contribute to no block and are
/// dropped here (they're held until their link arrives).
pub fn dedup_latest(rankings: Vec<Ranking>) -> Vec<Ranking> {
    let mut latest: HashMap<(Uuid, Uuid), Ranking> = HashMap::new();
    for r in rankings {
        let Some(block_id) = r.block_id else { continue };
        let key = (block_id, r.space_id);
        // Larger is later; on equal markers the lower id is "later".
        let order = |x: &Ranking| (x.updated_at_block, x.update_index, std::cmp::Reverse(x.id));
        match latest.get(&key) {
            Some(existing) if order(existing) >= order(&r) => {}
            _ => {
                latest.insert(key, r);
            }
        }
    }
    latest.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn ranking(id: u128, block: Option<u128>, space: u128, blk: i64, idx: i64) -> Ranking {
        Ranking {
            id: Uuid::from_u128(id),
            block_id: block.map(Uuid::from_u128),
            space_id: Uuid::from_u128(space),
            author_address: None,
            rank_type: None,
            submitted_at: Some(Utc::now()),
            updated_at_block: blk,
            update_index: idx,
            block_relation_id: None,
        }
    }

    #[test]
    fn keeps_latest_per_block_and_space() {
        let earlier = ranking(1, Some(100), 200, 10, 0);
        let later = ranking(2, Some(100), 200, 20, 0);
        let out = dedup_latest(vec![earlier, later]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, Uuid::from_u128(2));
    }

    #[test]
    fn tie_breaks_on_update_index() {
        let a = ranking(1, Some(100), 200, 20, 1);
        let b = ranking(2, Some(100), 200, 20, 5);
        let out = dedup_latest(vec![a, b]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, Uuid::from_u128(2));
    }

    #[test]
    fn a_full_tie_keeps_the_lowest_id_in_any_order() {
        for input in [
            vec![
                ranking(7, Some(100), 200, 20, 0),
                ranking(3, Some(100), 200, 20, 0),
            ],
            vec![
                ranking(3, Some(100), 200, 20, 0),
                ranking(7, Some(100), 200, 20, 0),
            ],
        ] {
            let out = dedup_latest(input);
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].id, Uuid::from_u128(3));
        }
    }

    #[test]
    fn different_spaces_both_kept() {
        let a = ranking(1, Some(100), 200, 10, 0);
        let b = ranking(2, Some(100), 201, 10, 0);
        let out = dedup_latest(vec![a, b]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn unlinked_rankings_dropped() {
        let unlinked = ranking(1, None, 200, 10, 0);
        let out = dedup_latest(vec![unlinked]);
        assert!(out.is_empty());
    }
}
