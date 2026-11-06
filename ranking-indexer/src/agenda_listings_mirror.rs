//! Mirrors `trending.agenda_listings` into geo-db's derived class (plan 10.14, collection 20).
//!
//! Collection 20 `agenda_listings`: group the listed entity's id (a claim, a debate or a topic) as a
//! big-endian `u128`, because the readers ask "what is the newest agenda listing of this entity"
//! (`trending.claim_listings`, `trending.current_debate_agendas`). Key the agenda id, the Space id
//! and the entity id (48 bytes, the key maximum, and unique in its collection). Score the
//! `listed_on` day as days since 1970-01-01, so a scan of one group walks the newest listing first,
//! as `ORDER BY listed_on DESC` does. Value thirteen bytes: `listed_on` i32 days, a one-byte source
//! mask and `written_at` i64 microseconds since the epoch, little-endian.
//!
//! The Postgres primary key also holds the source, which would not fit the key. One agenda can name
//! the same entity by two sources (a block item and its Claims relation), and they share an agenda,
//! a Space and so a `listed_on`; the mirror folds them into one row whose mask has one bit per
//! source: 1 `block_item`, 2 `agenda_claims`, 4 `agenda_topics`, 8 `agenda_debates`. A claim
//! listing is a row whose mask has a bit other than 4 (`source <> 'agenda_topics'`). `written_at`
//! is the newest of the folded rows. A row with any other source is skipped, not guessed.
//!
//! `trending.refresh_agenda_listings()` inserts and deletes rows in place, so the mirror reads the
//! identities the table held before the refresh and the whole table after the commit, and sends
//! every folded row as a put (last write wins, so unchanged rows are harmless) and a delete for each
//! key that dropped out. It never fails the job (see `geodb_cursor::derived`).

use std::collections::{BTreeMap, HashSet};

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `agenda_listings` in geo-db's collection registry.
pub const COLLECTION: u32 = 20;

/// The SQL that reads the table in the shape [`Listing`] takes.
pub const SELECT_SQL: &str = "SELECT agenda_id, space_id, entity_id, source, \
    (listed_on - DATE '1970-01-01') AS listed_day, \
    (extract(epoch FROM written_at)::numeric * 1000000)::bigint AS written_us \
    FROM trending.agenda_listings";

/// The SQL that reads the identities the table holds now, before a refresh changes it.
pub const PRIOR_SQL: &str = "SELECT agenda_id, space_id, entity_id FROM trending.agenda_listings";

/// One row of `trending.agenda_listings`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Listing {
    pub agenda_id: Uuid,
    pub space_id: Uuid,
    pub entity_id: Uuid,
    pub source: String,
    pub listed_day: i32,
    pub written_us: i64,
}

/// A listing's identity without its source: the agenda, the Space and the entity.
pub type Ident = (Uuid, Uuid, Uuid);

/// The mask bit for a source, or `None` for a source this mirror does not know.
pub fn source_bit(source: &str) -> Option<u8> {
    match source {
        "block_item" => Some(1),
        "agenda_claims" => Some(2),
        "agenda_topics" => Some(4),
        "agenda_debates" => Some(8),
        _ => None,
    }
}

/// A row's geo-db key: the agenda id, the Space id, then the entity id.
pub fn key(id: &Ident) -> Vec<u8> {
    let mut k = Vec::with_capacity(48);
    k.extend_from_slice(id.0.as_bytes());
    k.extend_from_slice(id.1.as_bytes());
    k.extend_from_slice(id.2.as_bytes());
    k
}

/// The puts, and the keys to delete, after the refresh left `rows`, given the identities the table
/// held before it. Rows of one identity fold into one put.
pub fn plan(rows: &[Listing], prior: &[Ident]) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    // identity -> (listed_day, mask, newest written_us)
    let mut folded: BTreeMap<Ident, (i32, u8, i64)> = BTreeMap::new();
    for r in rows {
        let Some(bit) = source_bit(&r.source) else {
            continue;
        };
        let e = folded
            .entry((r.agenda_id, r.space_id, r.entity_id))
            .or_insert((r.listed_day, 0, r.written_us));
        e.1 |= bit;
        e.2 = e.2.max(r.written_us);
    }
    let puts = folded
        .iter()
        .map(|(id, (day, mask, written))| {
            let mut v = Vec::with_capacity(13);
            v.extend_from_slice(&day.to_le_bytes());
            v.push(*mask);
            v.extend_from_slice(&written.to_le_bytes());
            DerivedRow {
                collection: COLLECTION,
                group: id.2.as_u128(),
                score: f64::from(*day),
                key: key(id),
                value: v,
            }
        })
        .collect();
    let seen: HashSet<Ident> = prior.iter().copied().collect();
    let deletes = seen
        .iter()
        .filter(|id| !folded.contains_key(*id))
        .map(key)
        .collect();
    (puts, deletes)
}

/// Sends the plan for one refresh. Never fails.
pub async fn mirror(mirror: &DerivedMirror, rows: &[Listing], prior: &[Ident]) {
    let (puts, mut deletes) = plan(rows, prior);
    if !puts.is_empty() {
        mirror.put("agenda_listings", &puts).await;
    }
    if !deletes.is_empty() {
        deletes.sort();
        mirror.delete("agenda_listings", COLLECTION, &deletes).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn row(agenda: u8, entity: u8, source: &str, day: i32) -> Listing {
        Listing {
            agenda_id: u(agenda),
            space_id: u(7),
            entity_id: u(entity),
            source: source.to_string(),
            listed_day: day,
            written_us: 1_700_000_000_000_000 + i64::from(day),
        }
    }

    #[test]
    fn a_row_is_grouped_by_entity_and_keyed_by_agenda_space_entity() {
        let (puts, deletes) = plan(&[row(1, 3, "block_item", 20_000)], &[]);
        assert!(deletes.is_empty());
        let p = &puts[0];
        assert_eq!(p.collection, 20);
        assert_eq!(p.group, u(3).as_u128());
        assert_eq!(p.score, 20_000.0);
        let mut key = vec![1u8; 16];
        key.extend_from_slice(&[7u8; 16]);
        key.extend_from_slice(&[3u8; 16]);
        assert_eq!(p.key, key);
        assert_eq!(p.key.len(), 48);
        assert_eq!(p.value.len(), 13);
        assert_eq!(&p.value[0..4], &20_000i32.to_le_bytes());
        assert_eq!(p.value[4], 1);
    }

    #[test]
    fn two_sources_of_one_listing_fold_into_one_row_with_both_bits() {
        let (puts, _) = plan(
            &[
                row(1, 3, "block_item", 20_000),
                row(1, 3, "agenda_claims", 20_000),
            ],
            &[],
        );
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].value[4], 3);
    }

    #[test]
    fn a_topic_listing_carries_only_the_topic_bit() {
        let (puts, _) = plan(&[row(1, 3, "agenda_topics", 20_000)], &[]);
        assert_eq!(puts[0].value[4], 4);
    }

    #[test]
    fn a_newer_day_scores_higher_so_a_descending_scan_walks_it_first() {
        let (puts, _) = plan(
            &[
                row(1, 3, "block_item", 20_000),
                row(2, 3, "block_item", 20_001),
            ],
            &[],
        );
        assert!(puts[1].score > puts[0].score);
        assert_eq!(puts[0].group, puts[1].group);
    }

    #[test]
    fn an_unknown_source_is_skipped() {
        let (puts, _) = plan(&[row(1, 3, "something_new", 20_000)], &[]);
        assert!(puts.is_empty());
    }

    #[test]
    fn identities_the_table_no_longer_has_are_deleted_once_and_kept_ones_are_not() {
        let rows = [row(1, 3, "block_item", 20_000)];
        let gone = (u(2), u(7), u(3));
        let (puts, deletes) = plan(&rows, &[(u(1), u(7), u(3)), gone, gone, (u(1), u(7), u(4))]);
        assert_eq!(puts.len(), 1);
        assert_eq!(deletes.len(), 2);
        assert!(deletes.contains(&key(&gone)));
    }

    #[test]
    fn an_empty_table_deletes_everything_it_held() {
        let (puts, deletes) = plan(&[], &[(u(1), u(7), u(3))]);
        assert!(puts.is_empty());
        assert_eq!(deletes.len(), 1);
    }
}
