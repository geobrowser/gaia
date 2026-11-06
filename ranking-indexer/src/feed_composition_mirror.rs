//! Mirrors `entity_feed_composition_samples` into geo-db's derived class (plan 10.14,
//! collection 25).
//!
//! Collection 25 `feed_composition_samples`: group the scope, the Space's id as a big-endian `u128`
//! (0 for the global feed), because the drift read is "the latest samples of this scope and
//! source". Score the sample's time in epoch seconds, so a scan walks the newest sample first. Key
//! 25 bytes: a source byte (0 window, 1 rendered), the sample id as a big-endian `i64`, then a
//! type's id (all zero for the sample's own row).
//!
//! A sample is one scalar row of 44 bytes, little-endian: window size `i32`, median age `i64`,
//! oldest age `i64` (each `i64::MIN` when null), participation weight, cap and tau as `f64`. Then
//! one row per type of the composition, whose value is the count as an `i32`. `type_ids` is not
//! stored; the composition's types carry it.
//!
//! The table only grows (about ten samples an hour, no retention), so the mirror sends the samples
//! of the last `RECENT_HOURS` hours as puts (last write wins, so a repeat is harmless) and deletes
//! nothing. `FEED_COMPOSITION_MIRROR_HOURS=0` sends the whole history once, for a backfill. It
//! never fails the job (see `geodb_cursor::derived`).

use chrono::{DateTime, Utc};
use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `feed_composition_samples` in geo-db's collection registry.
pub const COLLECTION: u32 = 25;

/// How far back a run re-sends by default: the run's own samples with a wide margin for a missed
/// run.
pub const RECENT_HOURS: i32 = 6;

/// The samples at most `$1` hours old, or all of them when `$1` is 0.
pub const SELECT_SQL: &str = "SELECT id, sampled_at, space_id, source, window_size, \
    median_age_seconds, oldest_age_seconds, participation_weight::float8 AS participation_weight, \
    participation_cap::float8 AS participation_cap, tau_seconds::float8 AS tau_seconds, \
    composition::text AS composition FROM entity_feed_composition_samples \
    WHERE $1 = 0 OR sampled_at >= now() - make_interval(hours => $1) ORDER BY id";

/// One row of `entity_feed_composition_samples`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Sample {
    pub id: i64,
    pub sampled_at: DateTime<Utc>,
    pub space_id: Option<Uuid>,
    pub source: String,
    pub window_size: i32,
    pub median_age_seconds: Option<i64>,
    pub oldest_age_seconds: Option<i64>,
    pub participation_weight: f64,
    pub participation_cap: f64,
    pub tau_seconds: f64,
    /// The composition object as JSON text: type id to count.
    pub composition: String,
}

/// A row's geo-db key: the source, the sample id, then the type (zero for the sample's own row).
pub fn key(source: u8, id: i64, type_id: Option<Uuid>) -> Vec<u8> {
    let mut k = Vec::with_capacity(25);
    k.push(source);
    k.extend_from_slice(&id.to_be_bytes());
    k.extend_from_slice(type_id.unwrap_or_else(Uuid::nil).as_bytes());
    k
}

fn source_byte(source: &str) -> Option<u8> {
    match source {
        "window" => Some(0),
        "rendered" => Some(1),
        _ => None,
    }
}

/// The rows of one sample: the scalars, then a row per composition type. A sample with an unknown
/// source yields none; a composition entry that is not a type id and a count is skipped.
pub fn rows(s: &Sample) -> Vec<DerivedRow> {
    let Some(source) = source_byte(&s.source) else {
        return Vec::new();
    };
    let group = s.space_id.unwrap_or_else(Uuid::nil).as_u128();
    let score = s.sampled_at.timestamp() as f64;
    let mut scalars = Vec::with_capacity(44);
    scalars.extend_from_slice(&s.window_size.to_le_bytes());
    scalars.extend_from_slice(&s.median_age_seconds.unwrap_or(i64::MIN).to_le_bytes());
    scalars.extend_from_slice(&s.oldest_age_seconds.unwrap_or(i64::MIN).to_le_bytes());
    scalars.extend_from_slice(&s.participation_weight.to_le_bytes());
    scalars.extend_from_slice(&s.participation_cap.to_le_bytes());
    scalars.extend_from_slice(&s.tau_seconds.to_le_bytes());
    let mut out = vec![DerivedRow {
        collection: COLLECTION,
        group,
        score,
        key: key(source, s.id, None),
        value: scalars,
    }];
    let composition: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&s.composition).unwrap_or_default();
    let mut counts: Vec<(Uuid, i32)> = composition
        .iter()
        .filter_map(|(t, n)| {
            let id = Uuid::parse_str(t).ok()?;
            let n = i32::try_from(n.as_i64()?).ok()?;
            Some((id, n))
        })
        .collect();
    counts.sort();
    out.extend(counts.into_iter().map(|(t, n)| DerivedRow {
        collection: COLLECTION,
        group,
        score,
        key: key(source, s.id, Some(t)),
        value: n.to_le_bytes().to_vec(),
    }));
    out
}

/// The puts for these samples. Nothing is deleted: the table is append-only.
pub fn plan(samples: &[Sample]) -> Vec<DerivedRow> {
    samples.iter().flat_map(rows).collect()
}

/// Sends the plan for one run. Never fails.
pub async fn mirror(mirror: &DerivedMirror, samples: &[Sample]) {
    let puts = plan(samples);
    if !puts.is_empty() {
        mirror.put("feed_composition_samples", &puts).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn sample(id: i64, space: Option<Uuid>, source: &str) -> Sample {
        Sample {
            id,
            sampled_at: Utc.with_ymd_and_hms(2026, 11, 6, 9, 17, 0).unwrap(),
            space_id: space,
            source: source.to_string(),
            window_size: 66,
            median_age_seconds: Some(3600),
            oldest_age_seconds: None,
            participation_weight: 7.0,
            participation_cap: 30.0,
            tau_seconds: 86_400.0,
            composition: format!("{{\"{}\": 24, \"{}\": 8}}", u(2), u(1)),
        }
    }

    #[test]
    fn a_sample_is_a_scalar_row_then_a_row_per_type() {
        let rows = rows(&sample(9, Some(u(3)), "window"));
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert_eq!(r.collection, 25);
            assert_eq!(r.group, u(3).as_u128());
            assert_eq!(r.score, 1_793_956_620.0);
            assert_eq!(r.key.len(), 25);
            assert_eq!(r.key[0], 0);
            assert_eq!(&r.key[1..9], &9i64.to_be_bytes());
        }
        assert_eq!(&rows[0].key[9..25], &[0u8; 16]);
        assert_eq!(&rows[1].key[9..25], &[1u8; 16]);
        assert_eq!(&rows[2].key[9..25], &[2u8; 16]);
        assert_eq!(rows[1].value, 8i32.to_le_bytes());
        assert_eq!(rows[2].value, 24i32.to_le_bytes());
    }

    #[test]
    fn the_scalars_are_44_bytes_with_min_for_null() {
        let v = &rows(&sample(9, None, "rendered"))[0].value;
        assert_eq!(v.len(), 44);
        assert_eq!(i32::from_le_bytes(v[0..4].try_into().unwrap()), 66);
        assert_eq!(i64::from_le_bytes(v[4..12].try_into().unwrap()), 3600);
        assert_eq!(i64::from_le_bytes(v[12..20].try_into().unwrap()), i64::MIN);
        assert_eq!(f64::from_le_bytes(v[20..28].try_into().unwrap()), 7.0);
        assert_eq!(f64::from_le_bytes(v[28..36].try_into().unwrap()), 30.0);
        assert_eq!(f64::from_le_bytes(v[36..44].try_into().unwrap()), 86_400.0);
    }

    #[test]
    fn the_global_scope_is_group_zero_and_rendered_is_source_one() {
        let rows = rows(&sample(9, None, "rendered"));
        assert_eq!(rows[0].group, 0);
        assert_eq!(rows[0].key[0], 1);
    }

    #[test]
    fn an_unknown_source_and_a_bad_composition_are_tolerated() {
        assert!(rows(&sample(9, None, "other")).is_empty());
        let mut s = sample(9, None, "window");
        s.composition = "not json".to_string();
        assert_eq!(rows(&s).len(), 1);
        s.composition = format!("{{\"{}\": 3, \"x\": 4, \"{}\": \"y\"}}", u(5), u(6));
        assert_eq!(rows(&s).len(), 2);
    }

    #[test]
    fn nothing_is_deleted_and_samples_do_not_collide() {
        let puts = plan(&[sample(9, None, "window"), sample(9, None, "rendered")]);
        assert_eq!(puts.len(), 6);
        let keys: std::collections::HashSet<_> = puts.iter().map(|p| p.key.clone()).collect();
        assert_eq!(keys.len(), 6);
    }
}
