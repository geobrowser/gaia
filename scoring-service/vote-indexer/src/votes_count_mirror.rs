//! Mirrors `votes_count` into geo-db's derived class (plan 10.14, collection 26).
//!
//! Collection 26 `votes_count`: one row per object, Space and vote kind, the vote tallies the API
//! and the notification indexer read. Group the object id as a big-endian `u128`, because the
//! readers ask for one object's tallies. Key the object id, the Space id, the object type and the
//! vote kind (34 bytes, the Postgres unique key). Score the net tally, `positive - negative`, as an
//! f64. Value 24 bytes, little-endian: `positive` i64, `negative` i64, then `updated_at` in
//! microseconds since the epoch as an i64.
//!
//! The vote transaction upserts absolute tallies, so each row is complete and last-write-wins; the
//! mirror runs after the commit and never fails a batch (see `geodb_cursor::derived`). `updated_at`
//! is the wall clock at the mirror write, which is within the batch's time of Postgres' own. A
//! snapshot of the whole table at start repairs any put the best-effort mirror dropped and
//! backfills the rows that predate it. Rows are never deleted: nothing deletes from `votes_count`.
//!
//! Collection 33 `votes_count_updates` (geo-db 19b75f0596, plan 10.22 section 29.6): the update-time
//! order of the same rows, for the notification poller's "changed since T" read. One row of 33 for
//! each row of 26, with the same key and the same 24-byte value. Group `object_type << 8 | vote_kind`
//! (`collection.voteUpdateGroup`), so the poller's `object_type = 0 AND vote_kind = 0` is group 0.
//! Score minus `updated_at` in microseconds (`collection.voteUpdateScore`), so a scan walks the
//! oldest update first. [`mirror`] sends both rows of a count adjacent in one call, so the live
//! write and the start-up copy both carry the pair. Collection 33 needs a geodb client at or after
//! 19b75f0596: an older node refuses the collection and, as for any refused put, the rest of the
//! call is dropped and counted.

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use std::sync::atomic::{AtomicI64, Ordering};
use uuid::Uuid;

/// Rows geo-db acknowledged, by collection (label `collection` = 26 or 33).
pub const ROWS_PUT: &str = "vote_indexer_votes_count_mirror_rows_total";
/// The newest `updated_at` (microseconds) of a count the mirror was asked to send.
pub const SOURCE_NEWEST_US: &str = "vote_indexer_votes_count_mirror_source_newest_updated_us";
/// The newest `updated_at` (microseconds) of a count whose rows geo-db fully acknowledged.
pub const LAST_APPLIED_US: &str = "vote_indexer_votes_count_mirror_last_applied_updated_us";
/// Seconds the mirror is behind the source: the source's newest `updated_at` minus the last applied.
pub const LAG_SECONDS: &str = "vote_indexer_votes_count_mirror_lag_seconds";

static SOURCE_NEWEST: AtomicI64 = AtomicI64::new(0);
static LAST_APPLIED: AtomicI64 = AtomicI64::new(0);

/// Seconds `applied_us` is behind `newest_us`; 0 when nothing was offered yet or the mirror is level
/// or ahead (a snapshot can apply an older row after a newer live one).
pub fn lag_seconds(newest_us: i64, applied_us: i64) -> f64 {
    if newest_us <= 0 {
        return 0.0;
    }
    (newest_us.saturating_sub(applied_us.max(0)).max(0)) as f64 / 1_000_000.0
}

/// How many rows of each collection the first `acked` rows of `units` hold, and the newest update
/// time of the units entirely inside `acked`. `units` is each count's collections and its time, in
/// send order (a unit is acknowledged as far as the node got, in order).
pub fn account(units: &[(Vec<u32>, i64)], acked: usize) -> (u64, u64, Option<i64>) {
    let (mut n26, mut n33, mut applied) = (0u64, 0u64, None::<i64>);
    let mut left = acked;
    for (cols, us) in units {
        let take = left.min(cols.len());
        for c in &cols[..take] {
            match *c {
                COLLECTION => n26 += 1,
                UPDATES_COLLECTION => n33 += 1,
                _ => {}
            }
        }
        left -= take;
        if take == cols.len() {
            applied = Some(applied.map_or(*us, |a| a.max(*us)));
        } else {
            break;
        }
    }
    (n26, n33, applied)
}

fn record(units: &[(Vec<u32>, i64)], acked: usize) {
    let (n26, n33, applied) = account(units, acked);
    metrics::counter!(ROWS_PUT, "collection" => "26").increment(n26);
    metrics::counter!(ROWS_PUT, "collection" => "33").increment(n33);
    if let Some(newest) = units.iter().map(|u| u.1).max() {
        SOURCE_NEWEST.fetch_max(newest, Ordering::Relaxed);
    }
    if let Some(a) = applied {
        LAST_APPLIED.fetch_max(a, Ordering::Relaxed);
    }
    let (newest, applied) = (
        SOURCE_NEWEST.load(Ordering::Relaxed),
        LAST_APPLIED.load(Ordering::Relaxed),
    );
    metrics::gauge!(SOURCE_NEWEST_US).set(newest as f64);
    metrics::gauge!(LAST_APPLIED_US).set(applied as f64);
    metrics::gauge!(LAG_SECONDS).set(lag_seconds(newest, applied));
}

/// Describes the mirror's metrics and registers them at zero.
pub fn register_metrics() {
    metrics::describe_counter!(
        ROWS_PUT,
        "Rows of votes_count (collection 26) and votes_count_updates (33) geo-db acknowledged"
    );
    metrics::describe_gauge!(
        SOURCE_NEWEST_US,
        "Newest votes_count updated_at, in microseconds, the geo-db mirror was asked to send"
    );
    metrics::describe_gauge!(
        LAST_APPLIED_US,
        "Newest votes_count updated_at, in microseconds, whose rows geo-db fully acknowledged"
    );
    metrics::describe_gauge!(
        LAG_SECONDS,
        "Seconds the votes_count geo-db mirror is behind the newest source row"
    );
    metrics::counter!(ROWS_PUT, "collection" => "26").absolute(0);
    metrics::counter!(ROWS_PUT, "collection" => "33").absolute(0);
    metrics::gauge!(SOURCE_NEWEST_US).set(0.0);
    metrics::gauge!(LAST_APPLIED_US).set(0.0);
    metrics::gauge!(LAG_SECONDS).set(0.0);
}

/// `votes_count` in geo-db's collection registry.
pub const COLLECTION: u32 = 26;

/// `votes_count_updates` in geo-db's collection registry.
pub const UPDATES_COLLECTION: u32 = 33;

/// The largest update time, in microseconds either side of the epoch, collection 33's score holds
/// (geo-db `vote_update_us_max`, 2^53: an f64 holds every integer up to it exactly).
pub const UPDATE_US_MAX: i64 = 1 << 53;

/// The label the mirror's metrics carry.
pub const TABLE: &str = "votes_count";

/// The SQL that reads the whole table in the shape [`Count`] takes.
pub const SELECT_SQL: &str = "SELECT object_id, space_id, object_type, vote_kind, positive, \
    negative, (extract(epoch FROM updated_at)::numeric * 1000000)::bigint AS updated_us \
    FROM votes_count";

/// One row of `votes_count`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Count {
    pub object_id: Uuid,
    pub space_id: Uuid,
    pub object_type: i16,
    pub vote_kind: i16,
    pub positive: i64,
    pub negative: i64,
    pub updated_us: i64,
}

/// A row's geo-db key: the object id, the Space id, the object type, then the vote kind.
pub fn key(c: &Count) -> Vec<u8> {
    key_of(c.object_id, c.space_id, c.object_type, c.vote_kind)
}

/// The same key from its parts, for a reader that holds no [`Count`].
pub fn key_of(object_id: Uuid, space_id: Uuid, object_type: i16, vote_kind: i16) -> Vec<u8> {
    let mut k = Vec::with_capacity(34);
    k.extend_from_slice(object_id.as_bytes());
    k.extend_from_slice(space_id.as_bytes());
    k.push(object_type as u8);
    k.push(vote_kind as u8);
    k
}

/// The geo-db row for one count.
pub fn row(c: &Count) -> DerivedRow {
    let mut v = Vec::with_capacity(24);
    v.extend_from_slice(&c.positive.to_le_bytes());
    v.extend_from_slice(&c.negative.to_le_bytes());
    v.extend_from_slice(&c.updated_us.to_le_bytes());
    DerivedRow {
        collection: COLLECTION,
        group: c.object_id.as_u128(),
        score: (c.positive as f64) - (c.negative as f64),
        key: key(c),
        value: v,
    }
}

/// Collection 33's group for a (type, kind): the type byte, then the kind byte
/// (geo-db `voteUpdateGroup`).
pub fn update_group(object_type: i16, vote_kind: i16) -> u128 {
    (u128::from(object_type as u8) << 8) | u128::from(vote_kind as u8)
}

/// Collection 33's score for an update time in microseconds: its negation (geo-db
/// `voteUpdateScore`), because a scan walks the highest score first.
pub fn update_score(updated_us: i64) -> f64 {
    -(updated_us as f64)
}

/// The update time a collection 33 score holds, the inverse of [`update_score`].
pub fn update_us_of(score: f64) -> i64 {
    (-score) as i64
}

/// The collection 33 row for one count: the key and value of [`row`], grouped by (type, kind) and
/// scored by minus the update time. `None` when the time is outside what the score holds, so the
/// count is still written to 26 and only its update-order row is left out.
pub fn update_row(c: &Count) -> Option<DerivedRow> {
    if !(-UPDATE_US_MAX..=UPDATE_US_MAX).contains(&c.updated_us) {
        return None;
    }
    let base = row(c);
    Some(DerivedRow {
        collection: UPDATES_COLLECTION,
        group: update_group(c.object_type, c.vote_kind),
        score: update_score(c.updated_us),
        key: base.key,
        value: base.value,
    })
}

/// Sends `counts` as puts: for each count its row of 26 and then its row of 33, in one call. A
/// count's rows are one unit, so no request, budget stop or refused request splits the pair
/// (geo-db commits a request as one frame; the poller reads 33, so a 26 row without its 33 row
/// would go unseen until that object's next put). Never fails.
pub async fn mirror(mirror: &DerivedMirror, counts: &[Count]) {
    let units: Vec<Vec<DerivedRow>> = counts
        .iter()
        .map(|c| {
            let mut unit = vec![row(c)];
            unit.extend(update_row(c));
            unit
        })
        .collect();
    let refs: Vec<&[DerivedRow]> = units.iter().map(|u| u.as_slice()).collect();
    if !refs.is_empty() {
        let acked = mirror.put_units(TABLE, &refs).await;
        let shape: Vec<(Vec<u32>, i64)> = units
            .iter()
            .zip(counts)
            .map(|(u, c)| (u.iter().map(|r| r.collection).collect(), c.updated_us))
            .collect();
        record(&shape, acked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn count() -> Count {
        Count {
            object_id: u(3),
            space_id: u(7),
            object_type: 0,
            vote_kind: 1,
            positive: 5,
            negative: 2,
            updated_us: 1_793_000_000_000_000,
        }
    }

    #[test]
    fn a_count_is_grouped_by_object_and_keyed_by_object_space_type_kind() {
        let r = row(&count());
        assert_eq!(r.collection, 26);
        assert_eq!(r.group, u(3).as_u128());
        assert_eq!(r.key.len(), 34);
        assert_eq!(&r.key[0..16], &[3u8; 16]);
        assert_eq!(&r.key[16..32], &[7u8; 16]);
        assert_eq!(r.key[32], 0);
        assert_eq!(r.key[33], 1);
    }

    #[test]
    fn the_score_is_the_net_tally() {
        assert_eq!(row(&count()).score, 3.0);
        let down = Count {
            positive: 1,
            negative: 4,
            ..count()
        };
        assert_eq!(row(&down).score, -3.0);
    }

    #[test]
    fn the_value_holds_positive_negative_and_updated_at() {
        let v = row(&count()).value;
        assert_eq!(v.len(), 24);
        assert_eq!(i64::from_le_bytes(v[0..8].try_into().unwrap()), 5);
        assert_eq!(i64::from_le_bytes(v[8..16].try_into().unwrap()), 2);
        assert_eq!(
            i64::from_le_bytes(v[16..24].try_into().unwrap()),
            1_793_000_000_000_000
        );
    }

    #[test]
    fn the_update_row_mirrors_key_and_value_and_orders_by_minus_time() {
        let c = count();
        let base = row(&c);
        let u = update_row(&c).unwrap();
        assert_eq!(u.collection, 33);
        assert_eq!(u.key, base.key);
        assert_eq!(u.value, base.value);
        assert_eq!(u.group, 1);
        assert_eq!(u.score, -1_793_000_000_000_000.0);
        // Object type 0, kind 0 (the poller's predicate) is group 0; type 1 kind 2 is 0x102.
        assert_eq!(update_group(0, 0), 0);
        assert_eq!(update_group(1, 2), 0x102);
        // A later update has a lower score, so a scan (highest score first) reads it later.
        let later = Count {
            updated_us: c.updated_us + 1,
            ..c.clone()
        };
        assert!(update_row(&later).unwrap().score < u.score);
        assert_eq!(update_us_of(u.score), c.updated_us);
    }

    #[test]
    fn a_time_outside_the_score_range_leaves_only_the_update_row_out() {
        let c = Count {
            updated_us: UPDATE_US_MAX + 1,
            ..count()
        };
        assert!(update_row(&c).is_none());
        let edge = Count {
            updated_us: UPDATE_US_MAX,
            ..count()
        };
        assert!(update_row(&edge).is_some());
    }

    #[tokio::test]
    async fn one_put_call_carries_both_rows_of_each_count() {
        use geodb_cursor::Config;
        use std::time::Duration;
        let log = std::env::temp_dir().join(format!("votes-count-mirror-{}", std::process::id()));
        let _ = std::fs::remove_file(&log);
        // The derived pipe answers each request with a `+` frame and logs the request.
        let script = format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '+10\\napplied: 1'; done",
            log.display()
        );
        let m = DerivedMirror::new(
            Config {
                program: "sh".into(),
                args: vec!["-c".into(), script.into()],
                collection: 0,
                timeout: Duration::from_millis(1500),
                cooldown: Duration::from_secs(60),
            },
            Duration::from_secs(5),
        );
        let a = count();
        let b = Count {
            object_id: u(4),
            updated_us: a.updated_us + 5,
            ..count()
        };
        mirror(&m, &[a.clone(), b.clone()]).await;
        let sent = std::fs::read_to_string(&log).unwrap();
        let _ = std::fs::remove_file(&log);
        let lines: Vec<&str> = sent.lines().collect();
        let expect: Vec<String> = [&a, &b]
            .iter()
            .flat_map(|c| [row(c), update_row(c).unwrap()])
            .map(|r| r.put_line().unwrap().trim_end().to_string())
            .collect();
        assert_eq!(lines, expect);
        assert!(lines[0].starts_with("put\t26\t"));
        assert!(lines[1].starts_with("put\t33\t1\t"));
    }

    #[test]
    fn lag_is_the_gap_in_seconds_and_never_negative() {
        assert_eq!(lag_seconds(0, 0), 0.0);
        assert_eq!(lag_seconds(10_000_000, 4_000_000), 6.0);
        assert_eq!(lag_seconds(10_000_000, 10_000_000), 0.0);
        assert_eq!(lag_seconds(10_000_000, 12_000_000), 0.0);
        // Nothing applied yet: the whole age of the newest row.
        assert_eq!(lag_seconds(1_500_000, 0), 1.5);
    }

    #[test]
    fn account_counts_rows_by_collection_and_the_newest_whole_unit() {
        let pair = |us| (vec![26, 33], us);
        let units = vec![pair(100), pair(300), (vec![26], 200), pair(400)];
        // Everything acknowledged.
        assert_eq!(account(&units, 7), (4, 3, Some(400)));
        // Stops after the first pair and half the second: only the first unit counts as applied.
        assert_eq!(account(&units, 3), (2, 1, Some(100)));
        // Nothing acknowledged.
        assert_eq!(account(&units, 0), (0, 0, None));
        // A count with no collection 33 row is a unit of one.
        assert_eq!(account(&units, 5), (3, 2, Some(300)));
        assert_eq!(account(&units, 6), (4, 2, Some(300)));
    }

    #[test]
    fn metrics_render_rows_and_lag() {
        use metrics_exporter_prometheus::PrometheusBuilder;
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            register_metrics();
            SOURCE_NEWEST.store(0, Ordering::Relaxed);
            LAST_APPLIED.store(0, Ordering::Relaxed);
            // Unit 2 is cut: applied stays at the first pair's time, newest is the second's.
            record(&[(vec![26, 33], 2_000_000), (vec![26, 33], 9_000_000)], 3);
        });
        let out = handle.render();
        for line in [
            "vote_indexer_votes_count_mirror_rows_total{collection=\"26\"} 2",
            "vote_indexer_votes_count_mirror_rows_total{collection=\"33\"} 1",
            "vote_indexer_votes_count_mirror_last_applied_updated_us 2000000",
            "vote_indexer_votes_count_mirror_lag_seconds 7",
        ] {
            assert!(out.contains(line), "missing `{line}` in:\n{out}");
        }
    }

    #[test]
    fn two_kinds_of_one_object_are_two_keys() {
        let other = Count {
            vote_kind: 2,
            ..count()
        };
        assert_ne!(key(&count()), key(&other));
    }
}
