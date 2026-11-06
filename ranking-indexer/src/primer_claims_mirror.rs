//! Mirrors the rest of the primer family into geo-db's derived class (plan 10.14, collections 15-19).
//!
//! A key is unique in its collection whatever its group, so every key below carries the part that
//! the group would otherwise have held. Values are little-endian. `refresh_primer_claim_stats()`
//! deletes and re-inserts its whole table in one transaction, so the mirror reads the table before
//! it (the prior membership pairs) and after it (the rows), puts every row (last write wins, so
//! unchanged rows are harmless) and deletes each key the table held before and no longer has.
//! Anchor sets and their members are only ever added, so they are put and never deleted. The
//! mirror never fails the job (see `geodb_cursor::derived`).
//!
//! - 15 `anchor_claim_sets`: group 0, key the set id as a big-endian `i32` (4 bytes), score
//!   `effective_from` in seconds since the epoch, value 20 bytes: `effective_from` (i64 us),
//!   `created_at` (i64 us), `claim_count` (i32). The `params` jsonb is not mirrored.
//! - 16 `anchor_claim_members`: group the set id, key the set id (4 bytes, big-endian) then the
//!   claim id (20 bytes), score the rank (so a scan walks rank 1 first), value 28 bytes: `score`,
//!   `weighted_voters`, `agree_share` (f64 each) then `rank` (i32).
//! - 17 `primer_claim_stats`: group 0, key the claim id (16 bytes), score the claim's `score`,
//!   value 57 bytes: `voters` (i32), `name_length` (i32), `weighted_voters`, `weighted_agree`,
//!   `agree_share`, `information`, `engagement` (f64 each), `computed_at` (i64 us), `contested`
//!   (one byte, 0 or 1). `space_ids` and `topic_ids` are the two membership collections below.
//! - 18 `primer_claim_topics`: group the topic id, key the topic id then the claim id (32 bytes),
//!   score the claim's `score`, empty value.
//! - 19 `primer_claim_spaces`: group the Space id, key the Space id then the claim id (32 bytes),
//!   score the claim's `score`, empty value.

use std::collections::HashSet;

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `anchor_claim_sets` in geo-db's collection registry.
pub const ANCHOR_SETS: u32 = 15;
/// `anchor_claim_members`.
pub const ANCHOR_MEMBERS: u32 = 16;
/// `primer_claim_stats`, the scalar columns keyed by claim.
pub const CLAIM_STATS: u32 = 17;
/// `(topic, claim)` membership from `primer_claim_stats.topic_ids`.
pub const CLAIM_TOPICS: u32 = 18;
/// `(Space, claim)` membership from `primer_claim_stats.space_ids`.
pub const CLAIM_SPACES: u32 = 19;

pub const ANCHOR_SET_VALUE_LEN: usize = 20;
pub const ANCHOR_MEMBER_VALUE_LEN: usize = 28;
pub const CLAIM_STATS_VALUE_LEN: usize = 57;

pub const SETS_SQL: &str = "SELECT id, \
    (extract(epoch FROM effective_from)::numeric * 1000000)::bigint AS effective_us, \
    (extract(epoch FROM created_at)::numeric * 1000000)::bigint AS created_us, claim_count \
    FROM public.anchor_claim_sets ORDER BY id";

pub const MEMBERS_SQL: &str = "SELECT set_id, claim_id, rank, score, weighted_voters, agree_share \
    FROM public.anchor_claim_members";

pub const STATS_SQL: &str = "SELECT claim_id, space_ids, topic_ids, voters, weighted_voters, \
    weighted_agree, agree_share, information, score, contested, engagement, name_length, \
    (extract(epoch FROM computed_at)::numeric * 1000000)::bigint AS computed_us \
    FROM public.primer_claim_stats";

/// The membership columns the table holds now, before a refresh replaces it.
pub const PRIOR_SQL: &str = "SELECT claim_id, space_ids, topic_ids FROM public.primer_claim_stats";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AnchorSet {
    pub id: i32,
    pub effective_us: i64,
    pub created_us: i64,
    pub claim_count: i32,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AnchorMember {
    pub set_id: i32,
    pub claim_id: Uuid,
    pub rank: i32,
    pub score: f64,
    pub weighted_voters: f64,
    pub agree_share: f64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ClaimStats {
    pub claim_id: Uuid,
    pub space_ids: Vec<Uuid>,
    pub topic_ids: Vec<Uuid>,
    pub voters: i32,
    pub weighted_voters: f64,
    pub weighted_agree: f64,
    pub agree_share: f64,
    pub information: f64,
    pub score: f64,
    pub contested: bool,
    pub engagement: f64,
    pub name_length: i32,
    pub computed_us: i64,
}

/// A claim's membership columns, read before a refresh.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PriorClaim {
    pub claim_id: Uuid,
    pub space_ids: Vec<Uuid>,
    pub topic_ids: Vec<Uuid>,
}

fn finite(x: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// `group` bytes then the claim id: the key of both membership collections.
pub fn pair_key(group: Uuid, claim: Uuid) -> Vec<u8> {
    let mut k = Vec::with_capacity(32);
    k.extend_from_slice(group.as_bytes());
    k.extend_from_slice(claim.as_bytes());
    k
}

/// The member key: the set id then the claim id.
pub fn member_key(set_id: i32, claim: Uuid) -> Vec<u8> {
    let mut k = Vec::with_capacity(20);
    k.extend_from_slice(&set_id.to_be_bytes());
    k.extend_from_slice(claim.as_bytes());
    k
}

fn dedup(ids: &[Uuid]) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    ids.iter().copied().filter(|i| seen.insert(*i)).collect()
}

/// Puts for the anchor sets.
pub fn plan_sets(sets: &[AnchorSet]) -> Vec<DerivedRow> {
    sets.iter()
        .map(|s| {
            let mut v = Vec::with_capacity(ANCHOR_SET_VALUE_LEN);
            v.extend_from_slice(&s.effective_us.to_le_bytes());
            v.extend_from_slice(&s.created_us.to_le_bytes());
            v.extend_from_slice(&s.claim_count.to_le_bytes());
            DerivedRow {
                collection: ANCHOR_SETS,
                group: 0,
                score: (s.effective_us / 1_000_000) as f64,
                key: s.id.to_be_bytes().to_vec(),
                value: v,
            }
        })
        .collect()
}

/// Puts for the anchor members.
pub fn plan_members(members: &[AnchorMember]) -> Vec<DerivedRow> {
    members
        .iter()
        .map(|m| {
            let mut v = Vec::with_capacity(ANCHOR_MEMBER_VALUE_LEN);
            v.extend_from_slice(&finite(m.score).to_le_bytes());
            v.extend_from_slice(&finite(m.weighted_voters).to_le_bytes());
            v.extend_from_slice(&finite(m.agree_share).to_le_bytes());
            v.extend_from_slice(&m.rank.to_le_bytes());
            DerivedRow {
                collection: ANCHOR_MEMBERS,
                group: m.set_id as u128,
                score: -f64::from(m.rank),
                key: member_key(m.set_id, m.claim_id),
                value: v,
            }
        })
        .collect()
}

/// What one refresh sends for `primer_claim_stats`.
#[derive(Debug, Default, PartialEq)]
pub struct StatsPlan {
    pub puts: Vec<DerivedRow>,
    pub deletes_stats: Vec<Vec<u8>>,
    pub deletes_topics: Vec<Vec<u8>>,
    pub deletes_spaces: Vec<Vec<u8>>,
}

/// The rows to put and the keys to delete after the refresh left `rows`, given what the table held
/// before it.
pub fn plan_stats(rows: &[ClaimStats], prior: &[PriorClaim]) -> StatsPlan {
    let mut plan = StatsPlan::default();
    let mut topics: HashSet<(Uuid, Uuid)> = HashSet::new();
    let mut spaces: HashSet<(Uuid, Uuid)> = HashSet::new();
    let mut claims: HashSet<Uuid> = HashSet::new();
    for r in rows {
        claims.insert(r.claim_id);
        let score = finite(r.score);
        let mut v = Vec::with_capacity(CLAIM_STATS_VALUE_LEN);
        v.extend_from_slice(&r.voters.to_le_bytes());
        v.extend_from_slice(&r.name_length.to_le_bytes());
        for x in [
            r.weighted_voters,
            r.weighted_agree,
            r.agree_share,
            r.information,
            r.engagement,
        ] {
            v.extend_from_slice(&finite(x).to_le_bytes());
        }
        v.extend_from_slice(&r.computed_us.to_le_bytes());
        v.push(u8::from(r.contested));
        plan.puts.push(DerivedRow {
            collection: CLAIM_STATS,
            group: 0,
            score,
            key: r.claim_id.as_bytes().to_vec(),
            value: v,
        });
        for t in dedup(&r.topic_ids) {
            topics.insert((t, r.claim_id));
            plan.puts.push(DerivedRow {
                collection: CLAIM_TOPICS,
                group: t.as_u128(),
                score,
                key: pair_key(t, r.claim_id),
                value: Vec::new(),
            });
        }
        for s in dedup(&r.space_ids) {
            spaces.insert((s, r.claim_id));
            plan.puts.push(DerivedRow {
                collection: CLAIM_SPACES,
                group: s.as_u128(),
                score,
                key: pair_key(s, r.claim_id),
                value: Vec::new(),
            });
        }
    }
    let mut gone_stats = HashSet::new();
    let mut gone_topics = HashSet::new();
    let mut gone_spaces = HashSet::new();
    for p in prior {
        if !claims.contains(&p.claim_id) && gone_stats.insert(p.claim_id) {
            plan.deletes_stats.push(p.claim_id.as_bytes().to_vec());
        }
        for t in dedup(&p.topic_ids) {
            if !topics.contains(&(t, p.claim_id)) && gone_topics.insert((t, p.claim_id)) {
                plan.deletes_topics.push(pair_key(t, p.claim_id));
            }
        }
        for s in dedup(&p.space_ids) {
            if !spaces.contains(&(s, p.claim_id)) && gone_spaces.insert((s, p.claim_id)) {
                plan.deletes_spaces.push(pair_key(s, p.claim_id));
            }
        }
    }
    plan
}

/// Sends the anchor sets and members. Never fails.
pub async fn mirror_anchors(m: &DerivedMirror, sets: &[AnchorSet], members: &[AnchorMember]) {
    let puts = plan_sets(sets);
    if !puts.is_empty() {
        m.put("anchor_claim_sets", &puts).await;
    }
    let puts = plan_members(members);
    if !puts.is_empty() {
        m.put("anchor_claim_members", &puts).await;
    }
}

/// Sends the plan for one refresh of `primer_claim_stats`. Never fails.
pub async fn mirror_stats(m: &DerivedMirror, rows: &[ClaimStats], prior: &[PriorClaim]) {
    let plan = plan_stats(rows, prior);
    if !plan.puts.is_empty() {
        m.put("primer_claim_stats", &plan.puts).await;
    }
    for (name, collection, keys) in [
        ("primer_claim_stats", CLAIM_STATS, &plan.deletes_stats),
        ("primer_claim_topics", CLAIM_TOPICS, &plan.deletes_topics),
        ("primer_claim_spaces", CLAIM_SPACES, &plan.deletes_spaces),
    ] {
        if !keys.is_empty() {
            m.delete(name, collection, keys).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn stats(claim: u8, topics: &[u8], spaces: &[u8], score: f64) -> ClaimStats {
        ClaimStats {
            claim_id: u(claim),
            space_ids: spaces.iter().map(|s| u(*s)).collect(),
            topic_ids: topics.iter().map(|t| u(*t)).collect(),
            voters: 9,
            weighted_voters: 4.5,
            weighted_agree: 2.25,
            agree_share: 0.5,
            information: 1.0,
            score,
            contested: true,
            engagement: 6.0,
            name_length: 40,
            computed_us: 1_700_000_000_000_000,
        }
    }

    fn prior(claim: u8, topics: &[u8], spaces: &[u8]) -> PriorClaim {
        PriorClaim {
            claim_id: u(claim),
            space_ids: spaces.iter().map(|s| u(*s)).collect(),
            topic_ids: topics.iter().map(|t| u(*t)).collect(),
        }
    }

    #[test]
    fn the_collections_are_15_to_19() {
        assert_eq!(
            [
                ANCHOR_SETS,
                ANCHOR_MEMBERS,
                CLAIM_STATS,
                CLAIM_TOPICS,
                CLAIM_SPACES
            ],
            [15, 16, 17, 18, 19]
        );
    }

    #[test]
    fn an_anchor_set_is_keyed_by_id_and_drops_params() {
        let p = &plan_sets(&[AnchorSet {
            id: 7,
            effective_us: 1_700_000_000_000_000,
            created_us: 1_700_000_005_000_000,
            claim_count: 40,
        }])[0];
        assert_eq!(p.collection, 15);
        assert_eq!(p.group, 0);
        assert_eq!(p.key, 7i32.to_be_bytes().to_vec());
        assert_eq!(p.score, 1_700_000_000.0);
        assert_eq!(p.value.len(), ANCHOR_SET_VALUE_LEN);
        assert_eq!(&p.value[0..8], &1_700_000_000_000_000i64.to_le_bytes());
        assert_eq!(&p.value[8..16], &1_700_000_005_000_000i64.to_le_bytes());
        assert_eq!(&p.value[16..20], &40i32.to_le_bytes());
    }

    #[test]
    fn an_anchor_member_is_grouped_by_set_and_keyed_by_set_then_claim() {
        let m = AnchorMember {
            set_id: 7,
            claim_id: u(3),
            rank: 2,
            score: 1.5,
            weighted_voters: 8.0,
            agree_share: 0.4,
        };
        let p = &plan_members(&[m])[0];
        assert_eq!(p.collection, 16);
        assert_eq!(p.group, 7);
        assert_eq!(p.score, -2.0);
        let mut key = 7i32.to_be_bytes().to_vec();
        key.extend_from_slice(&[3u8; 16]);
        assert_eq!(p.key, key);
        assert_eq!(p.value.len(), ANCHOR_MEMBER_VALUE_LEN);
        assert_eq!(&p.value[0..8], &1.5f64.to_le_bytes());
        assert_eq!(&p.value[8..16], &8.0f64.to_le_bytes());
        assert_eq!(&p.value[16..24], &0.4f64.to_le_bytes());
        assert_eq!(&p.value[24..28], &2i32.to_le_bytes());
    }

    #[test]
    fn a_claim_puts_one_stats_row_and_one_empty_row_per_topic_and_space() {
        let plan = plan_stats(&[stats(1, &[10, 11], &[20], 2.5)], &[]);
        assert_eq!(plan.puts.len(), 4);
        let s = &plan.puts[0];
        assert_eq!((s.collection, s.group, s.score), (17, 0, 2.5));
        assert_eq!(s.key, vec![1u8; 16]);
        assert_eq!(s.value.len(), CLAIM_STATS_VALUE_LEN);
        assert_eq!(&s.value[0..4], &9i32.to_le_bytes());
        assert_eq!(&s.value[4..8], &40i32.to_le_bytes());
        assert_eq!(&s.value[8..16], &4.5f64.to_le_bytes());
        assert_eq!(&s.value[48..56], &1_700_000_000_000_000i64.to_le_bytes());
        assert_eq!(s.value[56], 1);
        let t = &plan.puts[1];
        assert_eq!((t.collection, t.group, t.score), (18, u(10).as_u128(), 2.5));
        assert_eq!(t.key, pair_key(u(10), u(1)));
        assert!(t.value.is_empty());
        let sp = &plan.puts[3];
        assert_eq!((sp.collection, sp.group), (19, u(20).as_u128()));
        assert_eq!(sp.key, pair_key(u(20), u(1)));
        assert!(sp.value.is_empty());
    }

    #[test]
    fn a_repeated_id_in_an_array_is_one_row() {
        let plan = plan_stats(&[stats(1, &[10, 10], &[20, 20], 1.0)], &[]);
        assert_eq!(plan.puts.len(), 3);
    }

    #[test]
    fn pairs_that_left_are_deleted_and_kept_ones_are_not() {
        let rows = [stats(1, &[10], &[20], 1.0)];
        let plan = plan_stats(
            &rows,
            &[prior(1, &[10, 11], &[20, 21]), prior(2, &[10], &[20])],
        );
        assert_eq!(plan.deletes_stats, vec![vec![2u8; 16]]);
        assert_eq!(
            plan.deletes_topics,
            vec![pair_key(u(11), u(1)), pair_key(u(10), u(2))]
        );
        assert_eq!(
            plan.deletes_spaces,
            vec![pair_key(u(21), u(1)), pair_key(u(20), u(2))]
        );
    }

    #[test]
    fn an_empty_table_deletes_everything_it_held_and_an_empty_prior_nothing() {
        let plan = plan_stats(&[], &[prior(1, &[10], &[20])]);
        assert!(plan.puts.is_empty());
        assert_eq!(plan.deletes_stats.len(), 1);
        assert_eq!(plan.deletes_topics.len(), 1);
        assert_eq!(plan.deletes_spaces.len(), 1);
        let none = plan_stats(&[stats(1, &[10], &[], 1.0)], &[]);
        assert!(none.deletes_stats.is_empty() && none.deletes_topics.is_empty());
    }

    #[test]
    fn a_non_finite_score_is_zero_and_every_key_and_value_fits_the_limits() {
        let plan = plan_stats(&[stats(1, &[10], &[20], f64::NAN)], &[]);
        assert!(plan.puts.iter().all(|p| p.score == 0.0));
        assert!(plan.puts.iter().all(|p| p.put_line().is_ok()));
        let m = plan_members(&[AnchorMember {
            set_id: i32::MAX,
            claim_id: u(1),
            rank: 1,
            score: 1.0,
            weighted_voters: 1.0,
            agree_share: 1.0,
        }]);
        assert!(m[0].put_line().is_ok());
    }
}
