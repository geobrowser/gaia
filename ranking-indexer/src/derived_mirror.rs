//! Mirrors a block's `ranks.ranking_scores` into geo-db's derived class (plan 10.14, V10).
//!
//! Collection 11 `ranking_scores`: group the block id, key the block, entity and Space ids
//! (48 bytes), score the aggregate score, value the position as a little-endian `u32`. A block
//! recompute replaces the block wholesale in Postgres, so the mirror puts every row the new
//! compute produced and deletes the keys the block held before and no longer has. It runs after
//! the Postgres commit; failures are counted and logged, never returned (see
//! `geodb_cursor::derived`).

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

use crate::scoring::ScoreRow;

/// `ranking_scores` in geo-db's collection registry.
pub const COLLECTION: u32 = 11;

fn key(block: Uuid, entity: Uuid, space: Uuid) -> Vec<u8> {
    let mut k = Vec::with_capacity(48);
    k.extend_from_slice(block.as_bytes());
    k.extend_from_slice(entity.as_bytes());
    k.extend_from_slice(space.as_bytes());
    k
}

/// The rows to put and the keys to delete after `block_id` was recomputed to `rows`, given the
/// `(entity, space)` pairs it held before.
pub fn plan(
    block_id: Uuid,
    rows: &[ScoreRow],
    prior: &[(Uuid, Uuid)],
) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    let group = u128::from_be_bytes(*block_id.as_bytes());
    let puts: Vec<DerivedRow> = rows
        .iter()
        .map(|r| DerivedRow {
            collection: COLLECTION,
            group,
            score: r.score,
            key: key(block_id, r.entity_id, r.space_id),
            value: (r.position.max(0) as u32).to_le_bytes().to_vec(),
        })
        .collect();
    let now: std::collections::HashSet<&[u8]> = puts.iter().map(|p| p.key.as_slice()).collect();
    let deletes = prior
        .iter()
        .map(|(e, s)| key(block_id, *e, *s))
        .filter(|k| !now.contains(k.as_slice()))
        .collect();
    (puts, deletes)
}

/// Sends the plan for one block. Never fails.
pub async fn mirror_block(
    mirror: &DerivedMirror,
    block_id: Uuid,
    rows: &[ScoreRow],
    prior: Vec<(Uuid, Uuid)>,
) {
    let (puts, deletes) = plan(block_id, rows, &prior);
    if !puts.is_empty() {
        mirror.put("ranking_scores", &puts).await;
    }
    if !deletes.is_empty() {
        mirror.delete("ranking_scores", COLLECTION, &deletes).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn row(e: u8, s: u8, score: f64, position: i32) -> ScoreRow {
        ScoreRow {
            entity_id: u(e),
            space_id: u(s),
            score,
            position,
        }
    }

    #[test]
    fn rows_are_keyed_by_block_entity_space_with_position_as_value() {
        let (puts, deletes) = plan(u(1), &[row(2, 3, 9.5, 7)], &[]);
        assert!(deletes.is_empty());
        let p = &puts[0];
        assert_eq!(p.collection, 11);
        assert_eq!(p.group, u128::from_be_bytes([1; 16]));
        assert_eq!(p.key.len(), 48);
        assert_eq!(&p.key[..16], &[1; 16]);
        assert_eq!(&p.key[16..32], &[2; 16]);
        assert_eq!(&p.key[32..], &[3; 16]);
        assert_eq!(p.score, 9.5);
        assert_eq!(p.value, 7u32.to_le_bytes());
    }

    #[test]
    fn keys_the_block_no_longer_has_are_deleted_and_kept_ones_are_not() {
        let rows = [row(2, 3, 1.0, 1)];
        let prior = [(u(2), u(3)), (u(4), u(3)), (u(2), u(5))];
        let (puts, deletes) = plan(u(1), &rows, &prior);
        assert_eq!(puts.len(), 1);
        assert_eq!(deletes, vec![key(u(1), u(4), u(3)), key(u(1), u(2), u(5))]);
    }

    #[test]
    fn an_empty_recompute_deletes_everything_it_held() {
        let (puts, deletes) = plan(u(1), &[], &[(u(2), u(3))]);
        assert!(puts.is_empty());
        assert_eq!(deletes.len(), 1);
    }
}
