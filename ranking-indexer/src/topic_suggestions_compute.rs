//! The Rust twin of `refresh_space_topic_suggestions()` (migration 0095, GEO-3079).
//!
//! The SQL counts, per (space, topic), the distinct claims and debates linking to the topic through
//! the Topics property, keeps real named topics that are not the space's own, keeps one topic per
//! normalised name and ranks by links. [`compute`] does the same over rows read from the source
//! tables. `tests/fixtures/topic_suggestions_golden.txt` is the SQL's own output (scratch
//! postgres:16) over a seeded grid; the test compares to it. Writing to geo-db is a later cutover
//! step: today the SQL table stays the source and a run can only compare ([`count_mismatches`]).

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

pub const TOPICS_PROP: Uuid = Uuid::from_u128(0x806d52bc_27e9_4c91_93c0_57978b093351);
pub const TYPES_PROP: Uuid = Uuid::from_u128(0x8f151ba4_de20_4e3c_9cb4_99ddf96f48f1);
pub const NAME_PROP: Uuid = Uuid::from_u128(0xa126ca53_0c8e_48d5_b888_82c734c38935);
pub const CLAIM_TYPE: Uuid = Uuid::from_u128(0x96f859ef_a1ca_4b22_9372_c86ad58b694b);
pub const DEBATE_TYPE: Uuid = Uuid::from_u128(0xfd51f935_2063_4617_be39_7b672b23364c);
pub const TOPIC_TYPE: Uuid = Uuid::from_u128(0x5ef5a586_0f27_4d8e_8f6c_59ae5b3e89e2);

/// A row of `relations`.
#[derive(Debug, Clone, Copy)]
pub struct Relation {
    pub from: Uuid,
    pub to: Uuid,
    pub type_id: Uuid,
    pub space: Uuid,
}

/// A Name row of `values`.
#[derive(Debug, Clone)]
pub struct Name {
    pub entity: Uuid,
    pub space: Uuid,
    pub text: String,
}

/// A row of `spaces`.
#[derive(Debug, Clone, Copy)]
pub struct Space {
    pub id: Uuid,
    pub topic_id: Option<Uuid>,
}

/// One row of `space_topic_suggestions` (minus `refreshed_at`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub space_id: Uuid,
    pub topic_id: Uuid,
    pub name_key: String,
    pub links: i32,
    pub rank: i32,
}

/// `regexp_replace(lower(btrim(text)), '\s+', ' ', 'g')`, or None when `btrim(text) = ''`.
/// `btrim` trims spaces only; `\s` is space, tab, newline, CR, form feed and vertical tab.
pub fn name_key(text: &str) -> Option<String> {
    let t = text.trim_matches(' ');
    if t.is_empty() {
        return None;
    }
    let lower = t.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut in_ws = false;
    for c in lower.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\x0b') {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            in_ws = false;
            out.push(c);
        }
    }
    Some(out)
}

/// The key of the entity's name from the lowest-numbered space that has a non-blank one.
fn key_by_entity(names: &[Name]) -> HashMap<Uuid, String> {
    let mut best: HashMap<Uuid, (Uuid, String)> = HashMap::new();
    for n in names {
        let Some(k) = name_key(&n.text) else { continue };
        match best.get(&n.entity) {
            Some((s, _)) if *s <= n.space => {}
            _ => {
                best.insert(n.entity, (n.space, k));
            }
        }
    }
    best.into_iter().map(|(e, (_, k))| (e, k)).collect()
}

/// The table `refresh_space_topic_suggestions()` would write, ordered by space then rank.
pub fn compute(rels: &[Relation], names: &[Name], spaces: &[Space]) -> Vec<Row> {
    let mut claimish: HashSet<Uuid> = HashSet::new();
    let mut topics: HashSet<Uuid> = HashSet::new();
    for r in rels.iter().filter(|r| r.type_id == TYPES_PROP) {
        if r.to == CLAIM_TYPE || r.to == DEBATE_TYPE {
            claimish.insert(r.from);
        }
        if r.to == TOPIC_TYPE {
            topics.insert(r.from);
        }
    }
    let mut counted: HashMap<(Uuid, Uuid), HashSet<Uuid>> = HashMap::new();
    for r in rels
        .iter()
        .filter(|r| r.type_id == TOPICS_PROP && claimish.contains(&r.from))
    {
        counted.entry((r.space, r.to)).or_default().insert(r.from);
    }
    let keys = key_by_entity(names);
    let own: HashMap<Uuid, (Option<Uuid>, Option<&String>)> = spaces
        .iter()
        .map(|s| (s.id, (s.topic_id, s.topic_id.and_then(|t| keys.get(&t)))))
        .collect();

    // One topic per (space, name_key): most links, then lowest topic id.
    let mut best: HashMap<(Uuid, String), (i32, Uuid)> = HashMap::new();
    for ((space, topic), from) in &counted {
        if !topics.contains(topic) {
            continue;
        }
        let Some(k) = keys.get(topic) else { continue };
        if let Some((own_topic, own_key)) = own.get(space) {
            if *own_topic == Some(*topic) || own_key.is_some_and(|ok| ok == k) {
                continue;
            }
        }
        let links = from.len() as i32;
        let cand = (links, *topic);
        match best.get(&(*space, k.clone())) {
            Some(&(l, t)) if (l, std::cmp::Reverse(t)) >= (links, std::cmp::Reverse(*topic)) => {}
            _ => {
                best.insert((*space, k.clone()), cand);
            }
        }
    }
    let mut rows: Vec<Row> = best
        .into_iter()
        .map(|((space_id, name_key), (links, topic_id))| Row {
            space_id,
            topic_id,
            name_key,
            links,
            rank: 0,
        })
        .collect();
    rows.sort_by(|a, b| {
        (a.space_id, std::cmp::Reverse(a.links), a.topic_id).cmp(&(
            b.space_id,
            std::cmp::Reverse(b.links),
            b.topic_id,
        ))
    });
    let mut prev: Option<Uuid> = None;
    let mut n = 0;
    for r in rows.iter_mut() {
        n = if prev == Some(r.space_id) { n + 1 } else { 1 };
        prev = Some(r.space_id);
        r.rank = n;
    }
    rows
}

/// How many rows differ between two tables, as sets of (space, topic) keys with their columns.
pub fn count_mismatches(got: &[Row], want: &[Row]) -> usize {
    let w: HashMap<(Uuid, Uuid), &Row> =
        want.iter().map(|r| ((r.space_id, r.topic_id), r)).collect();
    let g: HashMap<(Uuid, Uuid), &Row> =
        got.iter().map(|r| ((r.space_id, r.topic_id), r)).collect();
    let differing = g.iter().filter(|(k, r)| w.get(k) != Some(r)).count();
    differing + w.keys().filter(|k| !g.contains_key(k)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/topic_suggestions_golden.txt");

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    struct Fixture {
        rels: Vec<Relation>,
        names: Vec<Name>,
        spaces: Vec<Space>,
        want: Vec<Row>,
    }

    fn load() -> Fixture {
        let mut f = Fixture {
            rels: vec![],
            names: vec![],
            spaces: vec![],
            want: vec![],
        };
        for line in GOLDEN.lines().filter(|l| !l.is_empty()) {
            let p: Vec<&str> = line.split('|').collect();
            match p[0] {
                "R" => f.rels.push(Relation {
                    from: id(p[1]),
                    to: id(p[2]),
                    type_id: id(p[3]),
                    space: id(p[4]),
                }),
                // The dump escapes tabs as the two characters \t and turns | into /.
                "V" => {
                    f.names.push(Name {
                        entity: id(p[1]),
                        space: id(p[2]),
                        text: p[3..].join("|").replace("\\t", "\t"),
                    });
                }
                "S" => f.spaces.push(Space {
                    id: id(p[1]),
                    topic_id: (p[2] != "N").then(|| id(p[2])),
                }),
                "O" => f.want.push(Row {
                    space_id: id(p[1]),
                    topic_id: id(p[2]),
                    name_key: p[3].replace("\\t", "\t"),
                    links: p[4].parse().unwrap(),
                    rank: p[5].parse().unwrap(),
                }),
                other => panic!("unknown tag {other}"),
            }
        }
        f
    }

    #[test]
    fn matches_the_sql_over_the_grid() {
        let f = load();
        assert!(f.want.len() > 200, "grid too small: {}", f.want.len());
        let got = compute(&f.rels, &f.names, &f.spaces);
        assert_eq!(got.len(), f.want.len());
        for (g, w) in got.iter().zip(&f.want) {
            assert_eq!(g, w);
        }
        assert_eq!(count_mismatches(&got, &f.want), 0);
    }

    #[test]
    fn name_key_normalises_like_the_sql() {
        assert_eq!(
            name_key("  Mental \t Health "),
            Some("mental health".into())
        );
        assert_eq!(name_key("ÉCONOMIE"), Some("économie".into()));
        assert_eq!(name_key("   "), None);
        assert_eq!(name_key(""), None);
        // btrim trims spaces only, so a lone tab is a (blank-looking) name.
        assert_eq!(name_key("\t"), Some(" ".into()));
    }

    #[test]
    fn mismatches_are_counted_both_ways() {
        let f = load();
        let mut got = compute(&f.rels, &f.names, &f.spaces);
        let dropped = got.pop().unwrap();
        got[0].links += 1;
        assert_eq!(count_mismatches(&got, &f.want), 2);
        assert_eq!(count_mismatches(&[], &f.want), f.want.len());
        let _ = dropped;
    }
}
