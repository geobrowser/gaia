//! The Rust twin of what `trending.refresh_agenda_listings()` decides to re-score, and of the trending
//! terms it hands to `public.refresh_entity_ranking_scores` (migrations 0113, 0118; GEO-3488).
//!
//! `trending.refresh_agenda_listings(full_pass)` has two halves. [`crate::agenda_listings_compute`] is the
//! first: what the agendas name now. This module is the second, the part of the re-scoring writer that is
//! logic rather than a table write:
//!
//! | Rust | SQL | migration |
//! | -- | -- | -- |
//! | [`is_full_pass`] | the `full_pass := COALESCE(...)` line of `trending.refresh_agenda_listings` | 0118 |
//! | [`rescore_ids`] | the `changed_agenda_listings` diff, the claim/debate/topic id sets, the freshness extension and the final `ids` union of `trending.refresh_agenda_listings` | 0118 |
//! | [`Graph::entity_listings`] | `trending.entity_listings` (with `newest_listings`, `claim_listings`) | 0118 |
//! | [`Graph::topic_listings`] | `trending.entity_topic_listings` | 0113 |
//! | [`Graph::debate_links`], [`Graph::claims_and_debates`] | `trending.debate_links`, `trending.claims_and_debates` | 0113 |
//! | [`Graph::trending_terms`] | the selection half of `trending.entity_trending_terms` | 0118 |
//!
//! The arithmetic of the terms (age credit, the listing and topic boosts, which of them is the source) is
//! [`crate::entity_ranking_score_compute::trending_terms`] from #1303, called here unchanged.
//!
//! Not ported, still SQL: reading the graph, the advisory lock, the `trending.agenda_listings` DELETE and
//! INSERT, the `trending.refresh_state` write, and `public.refresh_entity_ranking_scores` itself (its table
//! reads, the upsert and the two reconciles). Nothing here writes anything.
//!
//! Pinned to the SQL by `tests/fixtures/agenda_rescore_golden.txt`: the migrations' own `trending.*` function
//! bodies (as 0113 and 0118 leave them) on a scratch Postgres 16, over a seeded graph and 18 scenarios of
//! stored listings, incoming listings, stored scores, config and full-pass flag. The scratch database stubs
//! only the two boundaries: `trending.current_agenda_listings()` returns the scenario's incoming rows (that is
//! [`crate::agenda_listings_compute`]'s job) and `public.refresh_entity_ranking_scores(ids)` records the ids it
//! was handed instead of scoring them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use uuid::Uuid;

use crate::agenda_listings_compute::{Listing, Relation, Space};
use crate::entity_ranking_score_compute::{trending_terms, TrendConfig, TrendInputs, TrendTerms};
use crate::sql_helpers::{new_york_offset_secs, CLAIM_SOURCES};

const TYPES: Uuid = Uuid::from_u128(0x8f151ba4_de20_4e3c_9cb4_99ddf96f48f1);
const CLAIMS: Uuid = Uuid::from_u128(0xe614cce1_c4ce_4586_8304_fd1237119eb2);
const TOPICS: Uuid = Uuid::from_u128(0x806d52bc_27e9_4c91_93c0_57978b093351);
const CLAIM_TYPE: Uuid = Uuid::from_u128(0x96f859ef_a1ca_4b22_9372_c86ad58b694b);
const DEBATE_TYPE: Uuid = Uuid::from_u128(0xfd51f935_2063_4617_be39_7b672b23364c);

/// `last_full_pass_on IS DISTINCT FROM today`: the first run of each New York day is a full pass.
pub fn is_full_pass(last_full_pass_on: Option<NaiveDate>, today: NaiveDate) -> bool {
    last_full_pass_on != Some(today)
}

/// A row of `entity_ranking_scores` that still carries a term. Only these matter to the sweep: a missing row
/// and a zero row both read as "carries nothing".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoredTerm {
    pub entity: Uuid,
    pub trending_score: f64,
    pub age_credit: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sweep {
    pub today: NaiveDate,
    /// `entity_ranking_config.trending_horizon_days`.
    pub horizon_days: i32,
    /// `entity_ranking_config.trending_boost`.
    pub boost: f64,
    pub full_pass: bool,
}

/// The graph as the sweep reads it: the relations that matter, the spaces and the entities that exist, with
/// their created time (`None` unless the text is all digits).
pub struct Graph {
    dao: HashSet<Uuid>,
    /// `(is_claim, is_debate)` per entity with a Types relation to Claim or Debate, from any space.
    kinds: HashMap<Uuid, (bool, bool)>,
    /// `(from, to, space)` of Claims relations.
    claim_links: Vec<(Uuid, Uuid, Uuid)>,
    /// `(from, space)` that have a Types relation to Debate in that space.
    debate_typed: HashSet<(Uuid, Uuid)>,
    /// `(from, to, space)` of Topics relations.
    topic_links: Vec<(Uuid, Uuid, Uuid)>,
    created: HashMap<Uuid, Option<i64>>,
}

/// One row of what `trending.entity_trending_terms` returns.
#[derive(Debug, Clone, PartialEq)]
pub struct TermsRow {
    pub entity: Uuid,
    pub terms: TrendTerms,
    pub last_listed_on: Option<NaiveDate>,
    pub agenda_id: Option<Uuid>,
    pub debates: Option<i64>,
    pub topic_id: Option<Uuid>,
    pub topic_listed_on: Option<NaiveDate>,
}

/// `created_at ~ '^[0-9]+$'` then `::bigint`. A value past bigint raises in SQL; here it is `None`.
pub fn created_epoch(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `last_listed_on::timestamp AT TIME ZONE 'America/New_York'` as an epoch: midnight New York on that day.
pub fn midnight_epoch(day: NaiveDate) -> i64 {
    let local = day.and_time(NaiveTime::MIN);
    for hours in [4, 5] {
        let utc = Utc.from_utc_datetime(&(local + Duration::hours(hours)));
        if utc.naive_utc() - Duration::seconds(new_york_offset_secs(utc)) == local {
            return utc.timestamp();
        }
    }
    Utc.from_utc_datetime(&(local + Duration::hours(5)))
        .timestamp()
}

fn is_claim_source(source: &str) -> bool {
    CLAIM_SOURCES.contains(&source)
}

/// `DISTINCT ON (entity) ... ORDER BY entity, listed_on DESC, agenda_id` over the arrived rows of `sources`.
fn newest_listings(
    table: &BTreeSet<Listing>,
    ids: &HashSet<Uuid>,
    sources: &[&str],
    today: NaiveDate,
) -> HashMap<Uuid, (NaiveDate, Uuid)> {
    let mut best: HashMap<Uuid, (NaiveDate, Uuid)> = HashMap::new();
    for l in table {
        if !ids.contains(&l.entity_id) || !sources.contains(&l.source) || l.listed_on > today {
            continue;
        }
        let cand = (l.listed_on, l.agenda_id);
        let e = best.entry(l.entity_id).or_insert(cand);
        if cand.0 > e.0 || (cand.0 == e.0 && cand.1 < e.1) {
            *e = cand;
        }
    }
    best
}

impl Graph {
    pub fn new(rels: &[Relation], spaces: &[Space], created: HashMap<Uuid, Option<i64>>) -> Graph {
        let mut g = Graph {
            dao: spaces.iter().filter(|s| s.dao).map(|s| s.id).collect(),
            kinds: HashMap::new(),
            claim_links: Vec::new(),
            debate_typed: HashSet::new(),
            topic_links: Vec::new(),
            created,
        };
        for r in rels {
            if r.type_id == TYPES {
                if r.to == CLAIM_TYPE {
                    g.kinds.entry(r.from).or_default().0 = true;
                }
                if r.to == DEBATE_TYPE {
                    g.kinds.entry(r.from).or_default().1 = true;
                    g.debate_typed.insert((r.from, r.space));
                }
            } else if r.type_id == CLAIMS {
                g.claim_links.push((r.from, r.to, r.space));
            } else if r.type_id == TOPICS {
                g.topic_links.push((r.from, r.to, r.space));
            }
        }
        g
    }

    /// `trending.claims_and_debates(ids)`: `(is_claim, is_debate)` for those of `ids` typed either.
    pub fn claims_and_debates<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a Uuid>,
    ) -> HashMap<Uuid, (bool, bool)> {
        ids.into_iter()
            .filter_map(|id| self.kinds.get(id).map(|k| (*id, *k)))
            .collect()
    }

    /// `trending.debate_links(claim_ids, debate_ids)` as `(debate, claim)` pairs: Claims relations written in
    /// a DAO space where the source is also typed Debate.
    pub fn debate_links(
        &self,
        claims: &HashSet<Uuid>,
        debates: &HashSet<Uuid>,
    ) -> BTreeSet<(Uuid, Uuid)> {
        self.claim_links
            .iter()
            .filter(|(from, to, _)| claims.contains(to) || debates.contains(from))
            .filter(|(from, _, space)| {
                self.dao.contains(space) && self.debate_typed.contains(&(*from, *space))
            })
            .map(|(from, to, _)| (*from, *to))
            .collect()
    }

    /// `trending.entity_listings(ids)`: per claim or debate, the newest arrived listing that counts for it, as
    /// `(last_listed_on, agenda_id, own_claim_listing)`. A claim reads its own claim-source listings; a debate
    /// its own `agenda_debates` listing or its claims' claim listings. Ties on the date go to the debate
    /// listing (`own_claim_listing` false sorts first), then the lower agenda id.
    pub fn entity_listings(
        &self,
        table: &BTreeSet<Listing>,
        ids: &HashSet<Uuid>,
        today: NaiveDate,
    ) -> HashMap<Uuid, (NaiveDate, Uuid, bool)> {
        let k = self.claims_and_debates(ids);
        let mut claim_of: BTreeSet<(Uuid, Uuid)> = BTreeSet::new();
        let mut debates: HashSet<Uuid> = HashSet::new();
        for (e, (is_claim, is_debate)) in &k {
            if *is_claim {
                claim_of.insert((*e, *e));
            }
            if *is_debate {
                debates.insert(*e);
            }
        }
        claim_of.extend(self.debate_links(&HashSet::new(), &debates));
        let claims: HashSet<Uuid> = claim_of.iter().map(|(_, c)| *c).collect();
        let claim_newest = newest_listings(table, &claims, &CLAIM_SOURCES, today);
        let debate_newest = newest_listings(table, &debates, &["agenda_debates"], today);

        let mut rows: Vec<(Uuid, NaiveDate, bool, Uuid)> = Vec::new();
        for (entity, claim) in &claim_of {
            if let Some((d, a)) = claim_newest.get(claim) {
                rows.push((*entity, *d, claim == entity, *a));
            }
        }
        for (entity, (d, a)) in &debate_newest {
            rows.push((*entity, *d, false, *a));
        }
        // DISTINCT ON: the greatest date, then false before true, then the lowest agenda.
        let mut best: HashMap<Uuid, (NaiveDate, Uuid, bool)> = HashMap::new();
        for (entity, d, own, a) in rows {
            let better = |cur: &(NaiveDate, Uuid, bool)| {
                d > cur.0 || (d == cur.0 && (!own & cur.2 || (own == cur.2 && a < cur.1)))
            };
            match best.get(&entity) {
                Some(cur) if !better(cur) => {}
                _ => {
                    best.insert(entity, (d, a, own));
                }
            }
        }
        best
    }

    /// `trending.entity_topic_listings(ids)`: per claim or debate, the newest arrived topic listing among its
    /// own Topics relations written in a DAO space, as `(topic_id, listed_on)`; the lower topic id on a tie.
    pub fn topic_listings(
        &self,
        table: &BTreeSet<Listing>,
        ids: &HashSet<Uuid>,
        today: NaiveDate,
    ) -> HashMap<Uuid, (Uuid, NaiveDate)> {
        let k = self.claims_and_debates(ids);
        let mut listed: HashMap<Uuid, Vec<NaiveDate>> = HashMap::new();
        for l in table {
            if l.source == "agenda_topics" && l.listed_on <= today {
                listed.entry(l.entity_id).or_default().push(l.listed_on);
            }
        }
        let mut best: HashMap<Uuid, (Uuid, NaiveDate)> = HashMap::new();
        for (from, topic, space) in &self.topic_links {
            if !k.contains_key(from) || !self.dao.contains(space) {
                continue;
            }
            for d in listed.get(topic).into_iter().flatten() {
                let better =
                    |cur: &(Uuid, NaiveDate)| *d > cur.1 || (*d == cur.1 && *topic < cur.0);
                match best.get(from) {
                    Some(cur) if !better(cur) => {}
                    _ => {
                        best.insert(*from, (*topic, *d));
                    }
                }
            }
        }
        best
    }

    /// `trending.entity_trending_terms(ids)`: one row per claim or debate among `ids` that a listing or a topic
    /// listing boosts (and that exists in `entities`). `now_epoch` is `extract(epoch FROM now())`.
    pub fn trending_terms(
        &self,
        table: &BTreeSet<Listing>,
        ids: &HashSet<Uuid>,
        cfg: &TrendConfig,
        debate_window_days: i32,
        today: NaiveDate,
        now_epoch: f64,
    ) -> BTreeMap<Uuid, TermsRow> {
        let li = self.entity_listings(table, ids, today);
        let tp = self.topic_listings(table, ids, today);
        let reach: HashSet<Uuid> = li.keys().chain(tp.keys()).copied().collect();
        let target = self.claims_and_debates(&reach);

        // Debates inside the window on each claim whose own listing counts.
        let counted: HashSet<Uuid> = target
            .iter()
            .filter(|(e, (is_claim, _))| *is_claim && li.get(*e).is_some_and(|l| l.2))
            .map(|(e, _)| *e)
            .collect();
        let cutoff = now_epoch - f64::from(debate_window_days) * 86400.0;
        let mut recent: HashMap<Uuid, i64> = HashMap::new();
        for (debate, claim) in self.debate_links(&counted, &HashSet::new()) {
            if let Some(Some(created)) = self.created.get(&debate) {
                if *created as f64 >= cutoff {
                    *recent.entry(claim).or_default() += 1;
                }
            }
        }

        let mut out = BTreeMap::new();
        for (entity, (is_claim, _)) in &target {
            let Some(created) = self.created.get(entity) else {
                continue; // JOIN public.entities
            };
            let l = li.get(entity);
            let t = tp.get(entity);
            let debates =
                (*is_claim && l.is_some()).then(|| recent.get(entity).copied().unwrap_or(0));
            let inputs = TrendInputs {
                listed: l.is_some(),
                days: l.map(|l| (today - l.0).num_days()),
                debates,
                topic_days: t.map(|t| (today - t.1).num_days()),
                listed_epoch: l.map(|l| midnight_epoch(l.0)),
                created_epoch: *created,
            };
            out.insert(
                *entity,
                TermsRow {
                    entity: *entity,
                    terms: trending_terms(cfg, &inputs),
                    last_listed_on: l.map(|l| l.0),
                    agenda_id: l.map(|l| l.1),
                    debates,
                    topic_id: t.map(|t| t.0),
                    topic_listed_on: t.map(|t| t.1),
                },
            );
        }
        out
    }

    /// Every entity typed Claim or Debate: the ones whose created time the terms need.
    pub fn typed_ids(&self) -> Vec<Uuid> {
        self.kinds.keys().copied().collect()
    }

    /// Whether the entity exists in `entities`.
    pub fn exists(&self, id: &Uuid) -> bool {
        self.created.contains_key(id)
    }
}

/// The ids `trending.refresh_agenda_listings` passes to `public.refresh_entity_ranking_scores`.
///
/// `stored` is `trending.agenda_listings` before the call, `incoming` what the agendas name now (the table
/// after it). `stored_terms` holds the `entity_ranking_scores` rows with `trending_score <> 0 OR age_credit <> 0`.
pub fn rescore_ids(
    graph: &Graph,
    stored: &BTreeSet<Listing>,
    incoming: &BTreeSet<Listing>,
    stored_terms: &[StoredTerm],
    sweep: &Sweep,
) -> BTreeSet<Uuid> {
    let today = sweep.today;
    let in_horizon =
        |d: NaiveDate| d >= today - Duration::days(i64::from(sweep.horizon_days)) && d <= today;

    // changed_agenda_listings: rows removed (in the table, not incoming) and added (incoming, not in it).
    let rows = stored
        .difference(incoming)
        .chain(incoming.difference(stored))
        .map(|l| (l, true))
        .chain(incoming.iter().map(|l| (l, false)));

    let mut claim_ids: BTreeSet<Uuid> = BTreeSet::new();
    let mut debate_ids: BTreeSet<Uuid> = BTreeSet::new();
    let mut topic_ids: BTreeSet<Uuid> = BTreeSet::new();
    for (l, was_changed) in rows {
        if is_claim_source(l.source) && (was_changed || sweep.full_pass) {
            claim_ids.insert(l.entity_id);
        }
        if l.source == "agenda_debates" && (was_changed || sweep.full_pass) {
            debate_ids.insert(l.entity_id);
        }
        if l.source == "agenda_topics"
            && (was_changed || (sweep.full_pass && in_horizon(l.listed_on)))
        {
            topic_ids.insert(l.entity_id);
        }
    }

    // Claims listed inside the horizon with a debate that does not carry the boost yet. Only while the boost
    // is on: at 0 every such debate would match every hour.
    if sweep.boost > 0.0 {
        let carrying: HashSet<Uuid> = stored_terms
            .iter()
            .filter(|s| s.trending_score != 0.0)
            .map(|s| s.entity)
            .collect();
        let listed: HashSet<Uuid> = incoming
            .iter()
            .filter(|l| is_claim_source(l.source) && in_horizon(l.listed_on))
            .map(|l| l.entity_id)
            .collect();
        for (debate, claim) in graph.debate_links(&listed, &HashSet::new()) {
            if !carrying.contains(&debate) {
                claim_ids.insert(claim);
            }
        }
    }

    let claims: HashSet<Uuid> = claim_ids.iter().copied().collect();
    let mut ids = claim_ids;
    ids.extend(
        graph
            .debate_links(&claims, &HashSet::new())
            .into_iter()
            .map(|(debate, _)| debate),
    );
    ids.extend(debate_ids);
    // Claims and debates tagged, in a DAO space, with one of the topics.
    let tagged: HashSet<Uuid> = graph
        .topic_links
        .iter()
        .filter(|(_, to, space)| topic_ids.contains(to) && graph.dao.contains(space))
        .map(|(from, _, _)| *from)
        .collect();
    ids.extend(graph.claims_and_debates(&tagged).into_keys());
    if sweep.full_pass {
        ids.extend(
            stored_terms
                .iter()
                .filter(|s| s.trending_score != 0.0 || s.age_credit != 0.0)
                .map(|s| s.entity),
        );
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity_ranking_score_compute::close;

    const GOLDEN: &str = include_str!("../tests/fixtures/agenda_rescore_golden.txt");

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }
    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }
    fn src(s: &str) -> &'static str {
        [
            "block_item",
            "agenda_claims",
            "agenda_debates",
            "agenda_topics",
        ]
        .into_iter()
        .find(|k| *k == s)
        .unwrap_or_else(|| panic!("unknown source {s}"))
    }
    fn opt<T>(s: &str, f: impl Fn(&str) -> T) -> Option<T> {
        (s != "~").then(|| f(s))
    }

    #[derive(Default)]
    struct Scenario {
        full: bool,
        /// boost, topic boost, day factor, debate factor, tau
        cfg: Vec<f64>,
        horizon: i32,
        window: i32,
        stored: BTreeSet<Listing>,
        incoming: BTreeSet<Listing>,
        terms: Vec<StoredTerm>,
        ids: BTreeSet<Uuid>,
        want: Vec<Vec<String>>,
    }

    struct Fixture {
        today: NaiveDate,
        now: f64,
        graph: Graph,
        scenarios: Vec<(String, Scenario)>,
    }

    fn listing(p: &[&str]) -> Listing {
        Listing {
            agenda_id: id(p[2]),
            space_id: id(p[3]),
            entity_id: id(p[4]),
            listed_on: day(p[5]),
            source: src(p[6]),
        }
    }

    fn load() -> Fixture {
        let (mut today, mut now) = (NaiveDate::MIN, 0.0);
        let (mut rels, mut spaces, mut created) = (vec![], vec![], HashMap::new());
        let mut scenarios: Vec<(String, Scenario)> = vec![];
        for line in GOLDEN.lines().filter(|l| !l.is_empty()) {
            let p: Vec<&str> = line.split('|').collect();
            match p[0] {
                "N" => {
                    today = day(p[1]);
                    now = p[2].parse().unwrap();
                }
                "S" => spaces.push(Space {
                    id: id(p[1]),
                    dao: p[2] == "DAO",
                }),
                "E" => {
                    created.insert(id(p[1]), opt(p[2], created_epoch).flatten());
                }
                "R" => rels.push(Relation {
                    from: id(p[1]),
                    to: id(p[2]),
                    type_id: id(p[3]),
                    space: id(p[4]),
                }),
                "SC" => {
                    let s = Scenario {
                        full: p[2] == "t",
                        horizon: p[3].parse().unwrap(),
                        window: p[8].parse().unwrap(),
                        cfg: [4, 5, 6, 7, 9]
                            .iter()
                            .map(|i| p[*i].parse().unwrap())
                            .collect(),
                        ..Default::default()
                    };
                    scenarios.push((p[1].to_string(), s));
                }
                _ => {
                    let s = &mut scenarios.iter_mut().find(|(n, _)| n == p[1]).unwrap().1;
                    match p[0] {
                        "L" => {
                            s.stored.insert(listing(&p));
                        }
                        "I" => {
                            s.incoming.insert(listing(&p));
                        }
                        "T" => s.terms.push(StoredTerm {
                            entity: id(p[2]),
                            trending_score: p[3].parse().unwrap(),
                            age_credit: p[4].parse().unwrap(),
                        }),
                        "IDS" => {
                            s.ids.insert(id(p[2]));
                        }
                        "TT" => s.want.push(p.iter().map(|x| x.to_string()).collect()),
                        other => panic!("unknown row {other}"),
                    }
                }
            }
        }
        Fixture {
            today,
            now,
            graph: Graph::new(&rels, &spaces, created),
            scenarios,
        }
    }

    #[test]
    fn rescore_ids_match_the_sql() {
        let f = load();
        assert_eq!(f.scenarios.len(), 18);
        let mut total = 0;
        for (name, s) in &f.scenarios {
            let sweep = Sweep {
                today: f.today,
                horizon_days: s.horizon,
                boost: s.cfg[0],
                full_pass: s.full,
            };
            let got = rescore_ids(&f.graph, &s.stored, &s.incoming, &s.terms, &sweep);
            assert_eq!(got, s.ids, "scenario {name}");
            total += got.len();
        }
        assert!(total > 800, "the fixture lost its rows: {total}");
    }

    #[test]
    fn trending_terms_match_the_sql() {
        let f = load();
        let mut total = 0;
        for (name, s) in &f.scenarios {
            let cfg = TrendConfig {
                boost: s.cfg[0],
                topic_boost: s.cfg[1],
                day_factor: s.cfg[2],
                debate_factor: s.cfg[3],
                horizon_days: s.horizon,
                tau_seconds: s.cfg[4],
            };
            let all: HashSet<Uuid> = f.graph.created.keys().copied().collect();
            let got = f
                .graph
                .trending_terms(&s.incoming, &all, &cfg, s.window, f.today, f.now);
            assert_eq!(got.len(), s.want.len(), "scenario {name}: row count");
            for w in &s.want {
                let row = got
                    .get(&id(&w[2]))
                    .unwrap_or_else(|| panic!("scenario {name}: missing {}", w[2]));
                let what = format!("scenario {name} entity {}", w[2]);
                let num = |s: &str| s.parse::<f64>().unwrap();
                assert!(
                    close(row.terms.age_credit, num(&w[3])),
                    "{what} age {} vs {}",
                    row.terms.age_credit,
                    w[3]
                );
                assert!(
                    close(row.terms.trending_score, num(&w[4])),
                    "{what} trending"
                );
                assert_eq!(row.terms.source, opt(&w[5], src_kind), "{what} source");
                assert!(close(row.terms.listing_score, num(&w[6])), "{what} listing");
                assert_eq!(row.last_listed_on, opt(&w[7], day), "{what} last_listed_on");
                assert_eq!(row.agenda_id, opt(&w[8], id), "{what} agenda");
                assert_eq!(
                    row.debates,
                    opt(&w[9], |s| s.parse::<i64>().unwrap()),
                    "{what} debates"
                );
                assert!(
                    close(row.terms.topic_score, num(&w[10])),
                    "{what} topic score"
                );
                assert_eq!(row.topic_id, opt(&w[11], id), "{what} topic");
                assert_eq!(row.topic_listed_on, opt(&w[12], day), "{what} topic day");
                total += 1;
            }
        }
        assert!(total > 700, "the fixture lost its rows: {total}");
    }

    fn src_kind(s: &str) -> &'static str {
        match s {
            "listing" => "listing",
            "topic" => "topic",
            other => panic!("unknown trending source {other}"),
        }
    }

    #[test]
    fn full_pass_is_the_first_run_of_the_day() {
        assert!(is_full_pass(None, day("2026-11-06")));
        assert!(is_full_pass(Some(day("2026-11-05")), day("2026-11-06")));
        assert!(!is_full_pass(Some(day("2026-11-06")), day("2026-11-06")));
    }

    #[test]
    fn midnight_new_york_epochs() {
        // EST, EDT, and the two changeover days (midnight is before the 02:00 change on both).
        assert_eq!(midnight_epoch(day("2026-01-15")), 1768453200);
        assert_eq!(midnight_epoch(day("2026-07-01")), 1782878400);
        assert_eq!(midnight_epoch(day("2026-03-08")), 1772946000);
        assert_eq!(midnight_epoch(day("2026-11-01")), 1793505600);
    }

    #[test]
    fn created_epoch_is_digits_only() {
        assert_eq!(created_epoch("1700000000"), Some(1700000000));
        assert_eq!(created_epoch(""), None);
        assert_eq!(created_epoch("12x"), None);
        assert_eq!(created_epoch("-5"), None);
        assert_eq!(created_epoch("99999999999999999999"), None);
    }
}
