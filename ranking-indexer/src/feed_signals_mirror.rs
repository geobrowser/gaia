//! Mirrors `personalization.feed_item_signals_hourly` into geo-db's derived class (plan 10.14,
//! collection 22).
//!
//! Collection 22 `feed_item_signals`: group the Explore item's id as a big-endian `u128`, because
//! the readers ask "the buckets of these items since T" (`readCandidates`, `readPooledCandidates`
//! in for-you). Score the UTC hour as seconds since the epoch, so a scan of one group walks the
//! newest hour first. Key the item id (16 bytes), the hour as a big-endian `i64` (8) and a one-byte
//! part (25 bytes, unique in its collection).
//!
//! A bucket holds 72 bytes of counters and a derived value holds at most 64, so each (item, hour)
//! is two rows, told apart by the part byte, both little-endian:
//! part 0, 48 bytes, twelve `i32`: impressions, impressions_top3, impressions_4_10,
//! impressions_11_30, impressions_31_plus, opens, votes, plays, explore_plays,
//! plays_with_duration, completed_plays, debate_claim_votes.
//! part 1, 24 bytes: watch_ms `i64`, explore_watch_ms `i64`, watch_fraction_sum `f64`.
//!
//! `feed_signals_sync` replaces the window's buckets and expires old ones, so the mirror reads the
//! identities the table held before the run and the whole table after the commit (about 31 thousand
//! rows at steady state, far under `derived_rows_max`), sends every row as a put (last write wins,
//! so unchanged rows are harmless) and a delete of both parts of each identity that dropped out.
//! It never fails the job (see `geodb_cursor::derived`).

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};
use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `feed_item_signals` in geo-db's collection registry.
pub const COLLECTION: u32 = 22;

/// The SQL that reads the table in the shape [`Bucket`] takes.
pub const SELECT_SQL: &str = "SELECT item_id, hour, impressions, impressions_top3, \
    impressions_4_10, impressions_11_30, impressions_31_plus, opens, votes, plays, explore_plays, \
    plays_with_duration, completed_plays, debate_claim_votes, watch_ms, explore_watch_ms, \
    watch_fraction_sum FROM personalization.feed_item_signals_hourly";

/// The SQL that reads the identities the table holds now, before a run changes it.
pub const PRIOR_SQL: &str = "SELECT item_id, hour FROM personalization.feed_item_signals_hourly";

/// One row of `personalization.feed_item_signals_hourly`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Bucket {
    pub item_id: Uuid,
    pub hour: DateTime<Utc>,
    pub impressions: i32,
    pub impressions_top3: i32,
    pub impressions_4_10: i32,
    pub impressions_11_30: i32,
    pub impressions_31_plus: i32,
    pub opens: i32,
    pub votes: i32,
    pub plays: i32,
    pub explore_plays: i32,
    pub plays_with_duration: i32,
    pub completed_plays: i32,
    pub debate_claim_votes: i32,
    pub watch_ms: i64,
    pub explore_watch_ms: i64,
    pub watch_fraction_sum: f64,
}

/// A bucket's identity: the item and the UTC hour.
pub type Ident = (Uuid, DateTime<Utc>);

/// A row's geo-db key: the item id, the hour in epoch seconds big-endian, then the part.
pub fn key(id: &Ident, part: u8) -> Vec<u8> {
    let mut k = Vec::with_capacity(25);
    k.extend_from_slice(id.0.as_bytes());
    k.extend_from_slice(&id.1.timestamp().to_be_bytes());
    k.push(part);
    k
}

/// The two rows of one bucket.
pub fn rows(b: &Bucket) -> [DerivedRow; 2] {
    let id = (b.item_id, b.hour);
    let mut p0 = Vec::with_capacity(48);
    for n in [
        b.impressions,
        b.impressions_top3,
        b.impressions_4_10,
        b.impressions_11_30,
        b.impressions_31_plus,
        b.opens,
        b.votes,
        b.plays,
        b.explore_plays,
        b.plays_with_duration,
        b.completed_plays,
        b.debate_claim_votes,
    ] {
        p0.extend_from_slice(&n.to_le_bytes());
    }
    let mut p1 = Vec::with_capacity(24);
    p1.extend_from_slice(&b.watch_ms.to_le_bytes());
    p1.extend_from_slice(&b.explore_watch_ms.to_le_bytes());
    p1.extend_from_slice(&b.watch_fraction_sum.to_le_bytes());
    let mk = |part: u8, value: Vec<u8>| DerivedRow {
        collection: COLLECTION,
        group: b.item_id.as_u128(),
        score: b.hour.timestamp() as f64,
        key: key(&id, part),
        value,
    };
    [mk(0, p0), mk(1, p1)]
}

/// The puts, and the keys to delete, after the run left `buckets`, given the identities the table
/// held before it.
pub fn plan(buckets: &[Bucket], prior: &[Ident]) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    let mut by_id: BTreeMap<Ident, &Bucket> = BTreeMap::new();
    for b in buckets {
        by_id.insert((b.item_id, b.hour), b);
    }
    let puts = by_id.values().flat_map(|b| rows(b)).collect();
    let seen: HashSet<Ident> = prior.iter().copied().collect();
    let deletes = seen
        .iter()
        .filter(|id| !by_id.contains_key(*id))
        .flat_map(|id| [key(id, 0), key(id, 1)])
        .collect();
    (puts, deletes)
}

/// Sends the plan for one run. Never fails.
pub async fn mirror(mirror: &DerivedMirror, buckets: &[Bucket], prior: &[Ident]) {
    let (puts, mut deletes) = plan(buckets, prior);
    if !puts.is_empty() {
        mirror.put("feed_item_signals", &puts).await;
    }
    if !deletes.is_empty() {
        deletes.sort();
        mirror
            .delete("feed_item_signals", COLLECTION, &deletes)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn hour(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, h, 0, 0).unwrap()
    }

    fn bucket(item: u8, h: u32) -> Bucket {
        Bucket {
            item_id: u(item),
            hour: hour(h),
            impressions: 10,
            impressions_top3: 1,
            impressions_4_10: 2,
            impressions_11_30: 3,
            impressions_31_plus: 4,
            opens: 5,
            votes: 6,
            plays: 7,
            explore_plays: 8,
            plays_with_duration: 9,
            completed_plays: 11,
            debate_claim_votes: 12,
            watch_ms: 123_456_789_012,
            explore_watch_ms: 42,
            watch_fraction_sum: 2.5,
        }
    }

    #[test]
    fn a_bucket_is_two_rows_grouped_by_item_and_scored_by_hour() {
        let (puts, deletes) = plan(&[bucket(3, 5)], &[]);
        assert!(deletes.is_empty());
        assert_eq!(puts.len(), 2);
        for p in &puts {
            assert_eq!(p.collection, 22);
            assert_eq!(p.group, u(3).as_u128());
            assert_eq!(p.score, hour(5).timestamp() as f64);
            assert_eq!(p.key.len(), 25);
            assert_eq!(&p.key[0..16], &[3u8; 16]);
            assert_eq!(&p.key[16..24], &hour(5).timestamp().to_be_bytes());
        }
        assert_eq!(puts[0].key[24], 0);
        assert_eq!(puts[1].key[24], 1);
    }

    #[test]
    fn part_zero_holds_the_twelve_counters_in_order() {
        let (puts, _) = plan(&[bucket(3, 5)], &[]);
        let v = &puts[0].value;
        assert_eq!(v.len(), 48);
        let n: Vec<i32> = v
            .chunks(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(n, vec![10, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12]);
    }

    #[test]
    fn part_one_holds_the_watch_totals() {
        let (puts, _) = plan(&[bucket(3, 5)], &[]);
        let v = &puts[1].value;
        assert_eq!(v.len(), 24);
        assert_eq!(
            i64::from_le_bytes(v[0..8].try_into().unwrap()),
            123_456_789_012
        );
        assert_eq!(i64::from_le_bytes(v[8..16].try_into().unwrap()), 42);
        assert_eq!(f64::from_le_bytes(v[16..24].try_into().unwrap()), 2.5);
    }

    #[test]
    fn a_later_hour_scores_higher() {
        let (puts, _) = plan(&[bucket(3, 5), bucket(3, 6)], &[]);
        assert!(puts[2].score > puts[0].score);
    }

    #[test]
    fn dropped_identities_delete_both_parts_once() {
        let gone = (u(2), hour(1));
        let prior = [(u(3), hour(5)), gone, gone];
        let (puts, deletes) = plan(&[bucket(3, 5)], &prior);
        assert_eq!(puts.len(), 2);
        assert_eq!(deletes.len(), 2);
        assert!(deletes.contains(&key(&gone, 0)));
        assert!(deletes.contains(&key(&gone, 1)));
    }

    #[test]
    fn an_empty_table_deletes_everything_it_held() {
        let (puts, deletes) = plan(&[], &[(u(3), hour(5))]);
        assert!(puts.is_empty());
        assert_eq!(deletes.len(), 2);
    }
}
