//! The Rust twin of the topic-ranking SQL family that `topic_ranking_reconcile` calls hourly, as pure
//! functions over rows the caller has read (zero-Postgres 10.22, item 5 remainder).
//!
//! | Rust | SQL | migration |
//! | -- | -- | -- |
//! | [`Graph::eligible`] | `public.entity_topic_ranking_eligible(entity)` | 0096 |
//! | [`Graph::kinds`] | `public.entity_topic_ranking_kinds(entity)` | 0097 |
//! | [`Graph::topic_debate_ids`] | the id selection of `public.refresh_topic_debate_scores()` | 0107 |
//! | [`Graph::topic_debate_scores`] | `public.topic_debate_scores(entity_ids)` | 0107 |
//! | [`Graph::reconcile`] | `public.reconcile_entity_topic_ranking()` (rows and return count) | 0098 |
//!
//! Not ported, still SQL:
//!
//! * `public.refresh_entity_ranking_scores(ids)`, which `refresh_topic_debate_scores()` hands its ids to.
//!   Its inputs are graph statements (GAP-3) and its output is the 48.9M-row score table (10.14 section 11).
//! * The write of `entity_topic_ranking`. The reconcile is ported as the decision (which rows are deleted,
//!   which are upserted, what the table holds after); applying it waits on the 10.14 section 11 decision for
//!   how that table lives.
//! * Reading the graph, and `public.account_vote_weight` (see [`crate::account_vote_weight`]), whose weights
//!   the caller passes in.
//!
//! Nothing here reads or writes anything. Pinned to the SQL by `tests/fixtures/topic_ranking_golden.txt`:
//! the migrations' own function bodies (as they stand after 0098, 0097, 0107 and 0101) run on a scratch
//! Postgres 16 over 12 seeded synthetic scenarios, with `refresh_entity_ranking_scores` stubbed to record the
//! ids it is handed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use uuid::Uuid;

use crate::account_vote_weight::vote_weight;
pub use crate::agenda_rescore_compute::created_epoch;

const TYPES: Uuid = Uuid::from_u128(0x8f151ba4_de20_4e3c_9cb4_99ddf96f48f1);
const TOPICS: Uuid = Uuid::from_u128(0x806d52bc_27e9_4c91_93c0_57978b093351);
const SYSTEM_TYPE: Uuid = Uuid::from_u128(0x88b3d6ad_288c_529c_a212_0e1c24819185);
const NAME: Uuid = Uuid::from_u128(0xa126ca53_0c8e_48d5_b888_82c734c38935);
const CLAIMS: Uuid = Uuid::from_u128(0xe614cce1_c4ce_4586_8304_fd1237119eb2);
const CLAIM_TYPE: Uuid = Uuid::from_u128(0x96f859ef_a1ca_4b22_9372_c86ad58b694b);
const DEBATE_TYPE: Uuid = Uuid::from_u128(0xfd51f935_2063_4617_be39_7b672b23364c);
/// The Debate tag: a claim carrying it gets feed rows under this id, in place of a type.
const DEBATE_TAG: Uuid = Uuid::from_u128(0x55c95b26_26f8_482c_b973_9ea99dfde438);
const TAG_RELATION: Uuid = Uuid::from_u128(0x25709034_1ba5_406f_94e4_d4af90042fba);

/// One `public.relations` row. The space does not matter to any of these functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rel {
    pub from: Uuid,
    pub to: Uuid,
    pub type_id: Uuid,
}

/// One `public.values` row, for the Name property only (the caller may pass others; they are ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    pub entity: Uuid,
    pub property: Uuid,
    pub text: Option<String>,
}

/// `entity_topic_ranking_eligible`'s `length(trim(v.text)) > 0`: Postgres `trim` strips spaces only, not
/// tabs, newlines or non-breaking spaces.
fn has_text(t: &str) -> bool {
    !t.trim_matches(' ').is_empty()
}

/// One row of `entity_topic_ranking`, without its key: `(ranking_score, created_epoch)`.
pub type FeedRow = (f64, Option<i64>);
/// `(topic_id, type_id, entity_id)`.
pub type FeedKey = (Uuid, Uuid, Uuid);

/// What the reconcile does: the three counts it sums into its return value, and the table afterwards.
#[derive(Debug, Clone, PartialEq)]
pub struct Reconciled {
    /// Rows deleted because the entity is no longer eligible or has no score row.
    pub ineligible: usize,
    /// Rows deleted because the tag is gone or the kind is no longer one of the entity's.
    pub removed: usize,
    /// Rows inserted, plus rows whose score or created epoch changed.
    pub upserted: usize,
    pub table: BTreeMap<FeedKey, FeedRow>,
}

impl Reconciled {
    /// What `reconcile_entity_topic_ranking()` returns.
    pub fn changed(&self) -> usize {
        self.ineligible + self.removed + self.upserted
    }
}

/// `entity_ranking_config`'s topic columns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopicConfig {
    pub interested_weight: f64,
    pub debate_weight: f64,
    pub cap: f64,
}

/// One `user_votes` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vote {
    pub user: Uuid,
    pub object: Uuid,
    pub object_type: i16,
    pub vote_kind: i16,
    pub vote_type: i16,
}

/// A row of `topic_debate_scores`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebateScore {
    pub entity: Uuid,
    pub topic: Uuid,
    pub interested: f64,
    pub debates_held: i32,
    pub topic_score: f64,
}

/// The rows the family reads, indexed. `created` holds one key per `entities` row, with its `created_at`
/// as an epoch (`None` unless the text is all digits); an entity without a row is absent.
pub struct Graph {
    types: HashMap<Uuid, BTreeSet<Uuid>>,
    topics: HashMap<Uuid, BTreeSet<Uuid>>,
    topic_members: HashMap<Uuid, BTreeSet<Uuid>>,
    system: HashSet<Uuid>,
    has_claims: HashSet<Uuid>,
    tagged: HashSet<(Uuid, Uuid)>,
    named: HashSet<Uuid>,
    excluded: HashSet<Uuid>,
    created: HashMap<Uuid, Option<i64>>,
}

impl Graph {
    pub fn new(
        rels: &[Rel],
        values: &[Value],
        excluded_types: &[Uuid],
        created: HashMap<Uuid, Option<i64>>,
    ) -> Self {
        let mut g = Graph {
            types: HashMap::new(),
            topics: HashMap::new(),
            topic_members: HashMap::new(),
            system: HashSet::new(),
            has_claims: HashSet::new(),
            tagged: HashSet::new(),
            named: HashSet::new(),
            excluded: excluded_types.iter().copied().collect(),
            created,
        };
        for r in rels {
            match r.type_id {
                TYPES => {
                    g.types.entry(r.from).or_default().insert(r.to);
                }
                TOPICS => {
                    g.topics.entry(r.from).or_default().insert(r.to);
                    g.topic_members.entry(r.to).or_default().insert(r.from);
                }
                SYSTEM_TYPE => {
                    g.system.insert(r.from);
                }
                CLAIMS => {
                    g.has_claims.insert(r.from);
                }
                TAG_RELATION => {
                    g.tagged.insert((r.from, r.to));
                }
                _ => {}
            }
        }
        for v in values {
            if v.property == NAME && v.text.as_deref().is_some_and(has_text) {
                g.named.insert(v.entity);
            }
        }
        g
    }

    /// Named somewhere, not a System entity, and no editorially excluded type.
    pub fn eligible(&self, entity: Uuid) -> bool {
        self.named.contains(&entity)
            && !self.system.contains(&entity)
            && !self
                .types
                .get(&entity)
                .is_some_and(|t| t.iter().any(|x| self.excluded.contains(x)))
    }

    /// The entity's types, plus the Debate tag for a claim that carries it.
    pub fn kinds(&self, entity: Uuid) -> BTreeSet<Uuid> {
        let mut k = self.types.get(&entity).cloned().unwrap_or_default();
        if k.contains(&CLAIM_TYPE) && self.tagged.contains(&(entity, DEBATE_TAG)) {
            k.insert(DEBATE_TAG);
        }
        k
    }

    fn is_debate(&self, entity: Uuid) -> bool {
        self.types
            .get(&entity)
            .is_some_and(|t| t.contains(&DEBATE_TYPE))
    }

    /// The ids `refresh_topic_debate_scores()` hands to the score writer: typed Debate, tagged with a topic,
    /// and not holding a Claims relation.
    pub fn topic_debate_ids(&self) -> BTreeSet<Uuid> {
        self.types
            .iter()
            .filter(|(e, t)| {
                t.contains(&DEBATE_TYPE)
                    && self.topics.contains_key(e)
                    && !self.has_claims.contains(e)
            })
            .map(|(e, _)| *e)
            .collect()
    }

    /// `topic_debate_scores(entity_ids)`: one row per topic debate among `ids` (its strongest topic, the
    /// lower topic id on a tie). `weights` is `account_weights` by user id; a voter missing from it counts 0.
    pub fn topic_debate_scores(
        &self,
        ids: &BTreeSet<Uuid>,
        cfg: &TopicConfig,
        votes: &[Vote],
        weights: &BTreeMap<String, f64>,
    ) -> BTreeMap<Uuid, DebateScore> {
        // Current Interested on a topic, one per person: the same person can hold it in several spaces.
        let mut followers: HashMap<Uuid, BTreeSet<Uuid>> = HashMap::new();
        for v in votes {
            if v.object_type == 0 && v.vote_kind == 3 && v.vote_type == 0 {
                followers.entry(v.object).or_default().insert(v.user);
            }
        }
        let mut out: BTreeMap<Uuid, DebateScore> = BTreeMap::new();
        for &d in ids {
            let typed = self.is_debate(d) && !self.has_claims.contains(&d);
            let (Some(topics), Some(&epoch)) = (self.topics.get(&d), self.created.get(&d)) else {
                continue;
            };
            if !typed {
                continue;
            }
            for &topic in topics {
                let interested: f64 = followers
                    .get(&topic)
                    .map(|f| f.iter().map(|u| vote_weight(weights, &u.to_string())).sum())
                    .unwrap_or(0.0);
                // Debates tagged with the topic and made no later than this one. A debate without a
                // created time is never "no later" (the comparison is NULL).
                let held = self
                    .topic_members
                    .get(&topic)
                    .map(|m| {
                        m.iter()
                            .filter(|&&o| o != d && self.is_debate(o))
                            .filter(|o| {
                                matches!((self.created.get(o), epoch),
                                    (Some(Some(c)), Some(e)) if *c <= e)
                            })
                            .count() as i32
                    })
                    .unwrap_or(0);
                let score = cfg.cap.min(
                    cfg.interested_weight * (1.0 + interested.max(0.0)).ln()
                        + cfg.debate_weight * (1.0 + f64::from(held.max(0))).ln(),
                );
                let row = DebateScore {
                    entity: d,
                    topic,
                    interested,
                    debates_held: held,
                    topic_score: score,
                };
                // Topics iterate in id order, so a strictly greater score is the only replacement.
                match out.get(&d) {
                    Some(best) if best.topic_score >= score => {}
                    _ => {
                        out.insert(d, row);
                    }
                }
            }
        }
        out
    }

    /// `reconcile_entity_topic_ranking()` over `current` (the table now) and `scores` (`ranking_score` by
    /// entity, for entities that have a score row).
    pub fn reconcile(
        &self,
        scores: &HashMap<Uuid, f64>,
        current: &BTreeMap<FeedKey, FeedRow>,
    ) -> Reconciled {
        // Judged once per distinct tagged entity: tagged, scored, eligible.
        let keep: HashSet<Uuid> = self
            .topics
            .keys()
            .copied()
            .filter(|e| scores.contains_key(e) && self.eligible(*e))
            .collect();
        let mut table = BTreeMap::new();
        let (mut ineligible, mut removed) = (0, 0);
        for (key @ (topic, kind, entity), row) in current {
            if !keep.contains(entity) {
                ineligible += 1;
            } else if !self.topics.get(entity).is_some_and(|t| t.contains(topic))
                || !self.kinds(*entity).contains(kind)
            {
                removed += 1;
            } else {
                table.insert(*key, *row);
            }
        }
        let mut upserted = 0;
        for &entity in &keep {
            let Some(created) = self.created.get(&entity) else {
                continue;
            };
            let score = scores[&entity];
            let kinds = self.kinds(entity);
            for &topic in &self.topics[&entity] {
                for &kind in &kinds {
                    let want = (score, *created);
                    if table.insert((topic, kind, entity), want) != Some(want) {
                        upserted += 1;
                    }
                }
            }
        }
        Reconciled {
            ineligible,
            removed,
            upserted,
            table,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity_ranking_score_compute::close;

    const GOLDEN: &str = include_str!("../tests/fixtures/topic_ranking_golden.txt");

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }
    fn unescape(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars();
        while let Some(c) = it.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                other => panic!("bad escape {other:?}"),
            }
        }
        out
    }

    #[derive(Default)]
    struct Scenario {
        cfg: Vec<f64>,
        rels: Vec<Rel>,
        values: Vec<Value>,
        excluded: Vec<Uuid>,
        created: HashMap<Uuid, Option<i64>>,
        scores: HashMap<Uuid, f64>,
        table: BTreeMap<FeedKey, FeedRow>,
        votes: Vec<Vote>,
        weights: BTreeMap<String, f64>,
        eligible: BTreeMap<Uuid, bool>,
        kinds: BTreeMap<Uuid, BTreeSet<Uuid>>,
        debate_scores: Vec<Vec<String>>,
        refreshed: BTreeSet<Uuid>,
        changed: usize,
        after: BTreeMap<FeedKey, FeedRow>,
        changed_again: usize,
    }

    fn epoch(s: &str) -> Option<i64> {
        (s != "~").then(|| s.parse().unwrap())
    }

    fn load() -> Vec<Scenario> {
        let mut all: BTreeMap<usize, Scenario> = BTreeMap::new();
        for line in GOLDEN.lines().filter(|l| !l.is_empty()) {
            let p: Vec<&str> = line.split('|').collect();
            let s = all.entry(p[1].parse().unwrap()).or_default();
            match p[0] {
                "SC" => s.cfg = p[2..5].iter().map(|x| x.parse().unwrap()).collect(),
                "X" => s.excluded.push(id(p[2])),
                "E" => {
                    s.created.insert(id(p[2]), created_epoch(p[3]));
                }
                "R" => s.rels.push(Rel {
                    from: id(p[2]),
                    to: id(p[3]),
                    type_id: id(p[4]),
                }),
                "V" => s.values.push(Value {
                    entity: id(p[2]),
                    property: id(p[3]),
                    text: (p[4] != "~").then(|| unescape(p[4])),
                }),
                "SCORE" => {
                    s.scores.insert(id(p[2]), p[3].parse().unwrap());
                }
                "TR" => {
                    s.table.insert(
                        (id(p[2]), id(p[3]), id(p[4])),
                        (p[5].parse().unwrap(), epoch(p[6])),
                    );
                }
                "AW" => {
                    s.weights.insert(p[2].to_string(), p[3].parse().unwrap());
                }
                "UV" => s.votes.push(Vote {
                    user: id(p[2]),
                    object: id(p[3]),
                    object_type: p[4].parse().unwrap(),
                    vote_kind: p[5].parse().unwrap(),
                    vote_type: p[6].parse().unwrap(),
                }),
                "OE" => {
                    s.eligible.insert(id(p[2]), p[3] == "true");
                }
                "OK" => {
                    s.kinds.insert(
                        id(p[2]),
                        p[3].split(',').filter(|x| !x.is_empty()).map(id).collect(),
                    );
                }
                "OD" => s
                    .debate_scores
                    .push(p.iter().map(|x| x.to_string()).collect()),
                "OI" => {
                    s.refreshed = p[2].split(',').filter(|x| !x.is_empty()).map(id).collect();
                }
                "OC" => s.changed = p[2].parse().unwrap(),
                "OC2" => s.changed_again = p[2].parse().unwrap(),
                "OT" => {
                    s.after.insert(
                        (id(p[2]), id(p[3]), id(p[4])),
                        (p[5].parse().unwrap(), epoch(p[6])),
                    );
                }
                other => panic!("unknown row {other}"),
            }
        }
        all.into_values().collect()
    }

    fn graph(s: &Scenario) -> Graph {
        Graph::new(&s.rels, &s.values, &s.excluded, s.created.clone())
    }

    #[test]
    fn fixture_shape() {
        let all = load();
        assert_eq!(all.len(), 12);
        assert!(all.iter().map(|s| s.eligible.len()).sum::<usize>() > 400);
    }

    #[test]
    fn eligible_and_kinds_match_the_sql() {
        let (mut yes, mut no, mut tagged) = (0, 0, 0);
        for (n, s) in load().iter().enumerate() {
            let g = graph(s);
            for (e, want) in &s.eligible {
                assert_eq!(g.eligible(*e), *want, "scenario {n} eligible {e}");
                if *want {
                    yes += 1
                } else {
                    no += 1
                }
            }
            for (e, want) in &s.kinds {
                let got = g.kinds(*e);
                assert_eq!(&got, want, "scenario {n} kinds {e}");
                tagged += usize::from(got.contains(&DEBATE_TAG));
            }
        }
        assert!(yes > 100 && no > 100, "{yes} {no}");
        assert!(tagged > 0, "no debate-tagged claim in the fixture");
    }

    #[test]
    fn topic_debate_scores_match_the_sql() {
        let mut total = 0;
        for (n, s) in load().iter().enumerate() {
            let g = graph(s);
            let cfg = TopicConfig {
                interested_weight: s.cfg[0],
                debate_weight: s.cfg[1],
                cap: s.cfg[2],
            };
            let ids: BTreeSet<Uuid> = s
                .created
                .keys()
                .chain(s.rels.iter().map(|r| &r.from))
                .copied()
                .collect();
            let got = g.topic_debate_scores(&ids, &cfg, &s.votes, &s.weights);
            assert_eq!(got.len(), s.debate_scores.len(), "scenario {n}: row count");
            for w in &s.debate_scores {
                let e = id(&w[2]);
                let row = got
                    .get(&e)
                    .unwrap_or_else(|| panic!("scenario {n}: missing {e}"));
                assert_eq!(row.topic, id(&w[3]), "scenario {n} {e} topic");
                assert_eq!(
                    row.interested,
                    w[4].parse::<f64>().unwrap(),
                    "scenario {n} {e} interested"
                );
                assert_eq!(
                    row.debates_held,
                    w[5].parse::<i32>().unwrap(),
                    "scenario {n} {e} held"
                );
                assert!(
                    close(row.topic_score, w[6].parse().unwrap()),
                    "scenario {n} {e} score {} vs {}",
                    row.topic_score,
                    w[6]
                );
                total += 1;
            }
        }
        assert!(total > 100, "the fixture lost its rows: {total}");
    }

    #[test]
    fn topic_debate_ids_match_the_sql() {
        let mut total = 0;
        for (n, s) in load().iter().enumerate() {
            let got = graph(s).topic_debate_ids();
            assert_eq!(got, s.refreshed, "scenario {n}");
            total += got.len();
        }
        assert!(total > 50, "the fixture lost its rows: {total}");
    }

    #[test]
    fn reconcile_matches_the_sql() {
        let (mut rows, mut dropped) = (0, 0);
        for (n, s) in load().iter().enumerate() {
            let g = graph(s);
            let r = g.reconcile(&s.scores, &s.table);
            assert_eq!(r.changed(), s.changed, "scenario {n}: return count");
            assert_eq!(r.table, s.after, "scenario {n}: table after");
            // A second pass over what the first left changes nothing.
            let again = g.reconcile(&s.scores, &r.table);
            assert_eq!(
                again.changed(),
                s.changed_again,
                "scenario {n}: second pass"
            );
            assert_eq!(again.table, r.table, "scenario {n}: second pass table");
            rows += r.table.len();
            dropped += r.ineligible + r.removed;
        }
        assert!(rows > 300, "the fixture lost its rows: {rows}");
        assert!(dropped > 50, "the fixture deletes too little: {dropped}");
    }

    #[test]
    fn name_needs_a_non_space_character() {
        assert!(has_text("a"));
        assert!(has_text("\t"));
        assert!(has_text("\u{a0}"));
        assert!(!has_text(""));
        assert!(!has_text("   "));
    }
}
