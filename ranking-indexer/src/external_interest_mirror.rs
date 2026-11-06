//! Mirrors `personalization.external_interest_signals` into geo-db's derived class (plan 10.14
//! V12, collection 27).
//!
//! Collection 27 `external_interest_signals`: group the user's personal space id as a big-endian
//! `u128`, so one scan is everything a person marked; key 33 bytes: the user's uuid, the object's
//! uuid and a kind byte (`kind_tag`); score `occurred_at` in epoch seconds, so a scan walks the
//! newest first; value 16 bytes, `occurred_at` and `recorded_at` as little-endian `i64`
//! microseconds.
//!
//! These rows are one person's private marks, and geo-db does not yet restrict who may read a
//! derived collection (V5). So this mirror needs a second switch beside `GEODB_DERIVED_MIRROR`:
//! `GEODB_DERIVED_MIRROR_PRIVATE` set to `1` or `true`. Turning on the shared mirror alone cannot
//! copy personal rows. Enable the second only once geo-db's restricted-read grant exists.
//!
//! `not_interested_sync` replaces every row of its kind, so the mirror reads the kind's identities
//! before the run and the kind's rows after the commit, sends every row as a put (last write wins,
//! so unchanged rows are harmless) and a delete of each key that dropped out. It never fails the
//! job (see `geodb_cursor::derived`).

use std::collections::HashSet;

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `external_interest_signals` in geo-db's collection registry.
pub const COLLECTION: u32 = 27;

/// The SQL that reads a kind's rows in the shape [`Signal`] takes (`$1` the kind).
pub const SELECT_SQL: &str = "SELECT user_id, object_id, kind, \
    (extract(epoch FROM occurred_at)::numeric * 1000000)::bigint AS occurred_us, \
    (extract(epoch FROM recorded_at)::numeric * 1000000)::bigint AS recorded_us \
    FROM personalization.external_interest_signals WHERE kind = $1";

/// The SQL that reads the identities a kind holds now, before a run changes it (`$1` the kind).
pub const PRIOR_SQL: &str = "SELECT user_id, object_id, kind \
    FROM personalization.external_interest_signals WHERE kind = $1";

/// One row of `personalization.external_interest_signals`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Signal {
    pub user_id: Uuid,
    pub object_id: Uuid,
    pub kind: String,
    pub occurred_us: i64,
    pub recorded_us: i64,
}

/// The identity of a row: what the key is made of.
#[derive(Debug, Clone, PartialEq, Eq, Hash, sqlx::FromRow)]
pub struct Ident {
    pub user_id: Uuid,
    pub object_id: Uuid,
    pub kind: String,
}

/// The kind's byte in the key, or `None` for a kind geo-db has no byte for.
pub fn kind_tag(kind: &str) -> Option<u8> {
    Some(match kind {
        "vote" => 0,
        "comment" => 1,
        "debate" => 2,
        "follow" => 3,
        "interested" => 4,
        "not_interested" => 5,
        _ => return None,
    })
}

fn key(user: &Uuid, object: &Uuid, tag: u8) -> Vec<u8> {
    let mut k = Vec::with_capacity(33);
    k.extend_from_slice(user.as_bytes());
    k.extend_from_slice(object.as_bytes());
    k.push(tag);
    k
}

/// Whether the private switch is on in `get`'s environment.
pub fn private_enabled_from(get: impl Fn(&str) -> Option<String>) -> bool {
    matches!(
        get("GEODB_DERIVED_MIRROR_PRIVATE")
            .as_deref()
            .map(str::trim),
        Some("1") | Some("true")
    )
}

/// Whether `GEODB_DERIVED_MIRROR_PRIVATE` is on in the process environment.
pub fn private_enabled() -> bool {
    private_enabled_from(|k| std::env::var(k).ok())
}

/// The rows to put and the keys to delete after the sync left `rows`, given the identities the
/// table held before it. A row or identity of a kind with no byte is left out.
pub fn plan(rows: &[Signal], prior: &[Ident]) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    let puts: Vec<DerivedRow> = rows
        .iter()
        .filter_map(|r| {
            let tag = kind_tag(&r.kind)?;
            let mut value = Vec::with_capacity(16);
            value.extend_from_slice(&r.occurred_us.to_le_bytes());
            value.extend_from_slice(&r.recorded_us.to_le_bytes());
            Some(DerivedRow {
                collection: COLLECTION,
                group: r.user_id.as_u128(),
                score: (r.occurred_us as f64) / 1_000_000.0,
                key: key(&r.user_id, &r.object_id, tag),
                value,
            })
        })
        .collect();
    let now: HashSet<(Uuid, Uuid, &str)> = rows
        .iter()
        .map(|r| (r.user_id, r.object_id, r.kind.as_str()))
        .collect();
    let deletes = prior
        .iter()
        .filter(|p| !now.contains(&(p.user_id, p.object_id, p.kind.as_str())))
        .filter_map(|p| Some(key(&p.user_id, &p.object_id, kind_tag(&p.kind)?)))
        .collect();
    (puts, deletes)
}

/// Sends the plan for one sync. Never fails.
pub async fn mirror(mirror: &DerivedMirror, rows: &[Signal], prior: &[Ident]) {
    let (puts, deletes) = plan(rows, prior);
    if !puts.is_empty() {
        mirror.put("external_interest_signals", &puts).await;
    }
    if !deletes.is_empty() {
        mirror
            .delete("external_interest_signals", COLLECTION, &deletes)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn sig(user: u8, object: u8, kind: &str, at: i64) -> Signal {
        Signal {
            user_id: u(user),
            object_id: u(object),
            kind: kind.to_string(),
            occurred_us: at,
            recorded_us: at + 5_000_000,
        }
    }

    fn id(user: u8, object: u8, kind: &str) -> Ident {
        Ident {
            user_id: u(user),
            object_id: u(object),
            kind: kind.to_string(),
        }
    }

    #[test]
    fn a_row_is_grouped_by_user_with_a_33_byte_key_and_a_16_byte_value() {
        let (puts, deletes) = plan(&[sig(1, 2, "not_interested", 90_000_000)], &[]);
        assert!(deletes.is_empty());
        let p = &puts[0];
        assert_eq!(p.collection, 27);
        assert_eq!(p.group, u(1).as_u128());
        assert_eq!(p.score, 90.0);
        let mut want = vec![1u8; 16];
        want.extend_from_slice(&[2u8; 16]);
        want.push(5);
        assert_eq!(p.key, want);
        assert_eq!(p.value.len(), 16);
        assert_eq!(&p.value[0..8], &90_000_000i64.to_le_bytes());
        assert_eq!(&p.value[8..16], &95_000_000i64.to_le_bytes());
    }

    #[test]
    fn two_users_are_two_groups() {
        let (puts, _) = plan(
            &[
                sig(1, 3, "not_interested", 1),
                sig(2, 3, "not_interested", 1),
            ],
            &[],
        );
        assert_ne!(puts[0].group, puts[1].group);
        assert_ne!(puts[0].key, puts[1].key);
    }

    #[test]
    fn marks_the_table_no_longer_has_are_deleted_and_kept_ones_are_not() {
        let rows = [sig(1, 2, "not_interested", 1)];
        let prior = [
            id(1, 2, "not_interested"),
            id(1, 3, "not_interested"),
            id(2, 2, "not_interested"),
        ];
        let (puts, deletes) = plan(&rows, &prior);
        assert_eq!(puts.len(), 1);
        assert_eq!(deletes.len(), 2);
        assert!(!deletes.contains(&puts[0].key));
    }

    #[test]
    fn a_snapshot_that_cleared_everyone_deletes_everything_it_held() {
        let (puts, deletes) = plan(&[], &[id(1, 2, "not_interested")]);
        assert!(puts.is_empty());
        assert_eq!(deletes.len(), 1);
    }

    #[test]
    fn the_same_object_under_two_kinds_is_two_keys() {
        let (puts, _) = plan(
            &[sig(1, 2, "not_interested", 1), sig(1, 2, "interested", 1)],
            &[],
        );
        assert_ne!(puts[0].key, puts[1].key);
    }

    #[test]
    fn a_kind_without_a_byte_is_left_out() {
        assert_eq!(kind_tag("not_interested"), Some(5));
        assert_eq!(kind_tag("mystery"), None);
        let (puts, deletes) = plan(&[sig(1, 2, "mystery", 1)], &[id(1, 3, "mystery")]);
        assert!(puts.is_empty() && deletes.is_empty());
    }

    #[test]
    fn the_private_switch_is_its_own_variable_and_off_by_default() {
        assert!(!private_enabled_from(|_| None));
        // The shared switch alone does not turn it on.
        assert!(!private_enabled_from(|k| {
            (k == "GEODB_DERIVED_MIRROR").then(|| "1".to_string())
        }));
        assert!(private_enabled_from(|k| {
            (k == "GEODB_DERIVED_MIRROR_PRIVATE").then(|| "true".to_string())
        }));
        assert!(!private_enabled_from(|k| {
            (k == "GEODB_DERIVED_MIRROR_PRIVATE").then(|| "0".to_string())
        }));
    }
}
