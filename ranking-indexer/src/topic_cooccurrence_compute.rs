//! The Rust twin of `personalization.refresh_topic_cooccurrence()` (migration 0100, GEO-3140).
//!
//! The SQL reads the distinct (topic, entity) pairs of `entity_topic_ranking`, drops entities tagged
//! with too many topics, counts the entities each ordered pair of topics shares, keeps pairs with
//! enough support, scores them `shared / sqrt(size_a * size_b)` (at most 1) and keeps the closest
//! few neighbours per topic. [`compute`] does the same. `tests/fixtures/topic_cooccurrence_golden.txt`
//! is the SQL's own output (scratch postgres:16) for five parameter sets over one seeded grid, and
//! the test compares every similarity bit for bit: each is one division of integers by one
//! square root, which Rust and Postgres both do in IEEE double precision. Writing geo-db is a
//! later cutover step: today the SQL table stays the source and a run can only compare
//! ([`count_mismatches`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use uuid::Uuid;

/// The `interest_config` columns the refresh reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    pub min_support: i64,
    pub max_topics_per_entity: i64,
    pub max_neighbours: i64,
}

impl Default for Params {
    /// The table's column defaults.
    fn default() -> Self {
        Params {
            min_support: 3,
            max_topics_per_entity: 20,
            max_neighbours: 10,
        }
    }
}

/// One row of `personalization.topic_cooccurrence`.
#[derive(Debug, Clone, PartialEq)]
pub struct Neighbour {
    pub topic_id: Uuid,
    pub neighbour_id: Uuid,
    pub similarity: f64,
    pub support: i32,
}

/// The table the refresh would write, ordered by topic then closeness. `tags` are
/// `(topic, entity)` pairs and may repeat.
pub fn compute(tags: &[(Uuid, Uuid)], p: Params) -> Vec<Neighbour> {
    let pairs: BTreeSet<(Uuid, Uuid)> = tags.iter().copied().collect();
    let mut by_entity: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
    for (topic, entity) in &pairs {
        by_entity.entry(*entity).or_default().push(*topic);
    }
    // Entities tagged with more topics than the cap are left out.
    by_entity.retain(|_, topics| topics.len() as i64 <= p.max_topics_per_entity);

    let mut size: HashMap<Uuid, i64> = HashMap::new();
    let mut shared: HashMap<(Uuid, Uuid), i64> = HashMap::new();
    for topics in by_entity.values() {
        for a in topics {
            *size.entry(*a).or_insert(0) += 1;
            for b in topics.iter().filter(|b| *b != a) {
                *shared.entry((*a, *b)).or_insert(0) += 1;
            }
        }
    }

    let mut per_topic: BTreeMap<Uuid, Vec<Neighbour>> = BTreeMap::new();
    for ((a, b), n) in shared {
        if n < p.min_support {
            continue;
        }
        let (na, nb) = (size[&a] as f64, size[&b] as f64);
        let sim = (n as f64 / (na * nb).sqrt()).min(1.0);
        per_topic.entry(a).or_default().push(Neighbour {
            topic_id: a,
            neighbour_id: b,
            similarity: sim,
            support: n as i32,
        });
    }
    let keep = p.max_neighbours.max(0) as usize;
    let mut out = Vec::new();
    for (_, mut rows) in per_topic {
        rows.sort_by(|x, y| {
            y.similarity
                .total_cmp(&x.similarity)
                .then(x.neighbour_id.cmp(&y.neighbour_id))
        });
        rows.truncate(keep);
        out.extend(rows);
    }
    out
}

/// How many (topic, neighbour) keys differ, or differ in similarity bits or support.
pub fn count_mismatches(got: &[Neighbour], want: &[Neighbour]) -> usize {
    let key = |n: &Neighbour| (n.topic_id, n.neighbour_id);
    let w: HashMap<_, _> = want.iter().map(|n| (key(n), n)).collect();
    let g: HashMap<_, _> = got.iter().map(|n| (key(n), n)).collect();
    let differing = g
        .iter()
        .filter(|(k, n)| {
            w.get(k).is_none_or(|m| {
                m.similarity.to_bits() != n.similarity.to_bits() || m.support != n.support
            })
        })
        .count();
    differing + w.keys().filter(|k| !g.contains_key(k)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/topic_cooccurrence_golden.txt");

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    #[test]
    fn matches_the_sql_for_every_parameter_set() {
        let mut tags = Vec::new();
        let mut sets: BTreeMap<i32, Params> = BTreeMap::new();
        let mut want: BTreeMap<i32, Vec<Neighbour>> = BTreeMap::new();
        for line in GOLDEN.lines().filter(|l| !l.is_empty()) {
            let p: Vec<&str> = line.split('|').collect();
            match p[0] {
                "E" => tags.push((id(p[1]), id(p[2]))),
                "C" => {
                    sets.insert(
                        p[1].parse().unwrap(),
                        Params {
                            min_support: p[2].parse().unwrap(),
                            max_topics_per_entity: p[3].parse().unwrap(),
                            max_neighbours: p[4].parse().unwrap(),
                        },
                    );
                }
                "O" => want
                    .entry(p[1].parse().unwrap())
                    .or_default()
                    .push(Neighbour {
                        topic_id: id(p[2]),
                        neighbour_id: id(p[3]),
                        similarity: p[4].parse().unwrap(),
                        support: p[5].parse().unwrap(),
                    }),
                other => panic!("unknown tag {other}"),
            }
        }
        assert_eq!(sets.len(), 5);
        assert!(tags.len() > 3000);
        let mut total = 0;
        for (set, params) in &sets {
            let want = want.remove(set).unwrap_or_default();
            let got = compute(&tags, *params);
            assert_eq!(got.len(), want.len(), "set {set}: row count");
            // The SQL dump is ordered by topic then similarity descending then neighbour, as is compute.
            for (g, w) in got.iter().zip(&want) {
                assert_eq!(g.topic_id, w.topic_id, "set {set}");
                assert_eq!(g.neighbour_id, w.neighbour_id, "set {set}");
                assert_eq!(g.similarity.to_bits(), w.similarity.to_bits(), "set {set}");
                assert_eq!(g.support, w.support, "set {set}");
            }
            assert_eq!(count_mismatches(&got, &want), 0);
            total += want.len();
        }
        assert!(total > 3000, "grid too small: {total}");
        // The zero-neighbour set writes nothing.
        assert!(compute(&tags, sets[&3]).is_empty());
    }

    #[test]
    fn a_pair_needs_enough_support_and_similarity_is_capped_at_one() {
        let (a, b, c) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        let tags: Vec<_> = (10..13u128)
            .flat_map(|e| [(a, Uuid::from_u128(e)), (b, Uuid::from_u128(e))])
            .chain([(c, Uuid::from_u128(10))])
            .collect();
        let rows = compute(
            &tags,
            Params {
                min_support: 3,
                ..Params::default()
            },
        );
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.similarity == 1.0 && r.support == 3));
        assert!(compute(
            &tags,
            Params {
                min_support: 4,
                ..Params::default()
            }
        )
        .is_empty());
    }

    #[test]
    fn mismatches_count_a_changed_bit() {
        let n = Neighbour {
            topic_id: Uuid::from_u128(1),
            neighbour_id: Uuid::from_u128(2),
            similarity: 0.5,
            support: 3,
        };
        let mut m = n.clone();
        assert_eq!(
            count_mismatches(std::slice::from_ref(&n), std::slice::from_ref(&m)),
            0
        );
        m.similarity = f64::from_bits(0.5f64.to_bits() + 1);
        assert_eq!(count_mismatches(&[n], &[m]), 1);
    }
}
