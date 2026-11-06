//! The per-user topic-interest plpgsql (GEO-3088, migrations 0100, 0105, 0106, 0107 and 0117) as pure
//! Rust, so the compute can leave Postgres (plan 10.21 section 4.7, family 3).
//!
//! Nothing here touches a database. Every function takes the rows it needs and returns rows, which
//! is what lets one body serve three callers: the parity tests (against the SQL's documented
//! behaviour), the flag-off shadow in `bin/topic_interest.rs` (against the SQL itself, on a bounded
//! sample), and, later, the ranking service reading graph statements and writing collection 28.
//!
//! | SQL object | Rust |
//! | -- | -- |
//! | `user_interest_events` | [`derive_events`] over a [`Graph`] |
//! | `compute_user_topic_signals` | [`compute_signals`] |
//! | `recompute_user_topic_interest` | [`replace_user_rows`] |
//! | `dirty_interest_users` | [`dirty_users`] |
//! | `sweep_user_topic_interest` | [`plan_sweep`] and [`next_cursor`] |
//! | `refit_user_topic_interest` | [`refit_compare`] |
//! | `replace_external_interest_signals` | [`plan_external_replace`] |
//! | `for_you_candidates` | [`for_you_candidates`] |
//!
//! Times are microseconds since the epoch (`i64`), as in the collection 28 mirror.
//!
//! Where the SQL leaves an order unspecified (a comment with two `Reply to` targets of equal depth),
//! the Rust picks the smaller id, so its answer is deterministic. The SQL's own answer there is one
//! of the same candidates.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use uuid::Uuid;

/// `Reply to`.
pub const REPLY_TO: Uuid = Uuid::from_u128(0x310d4a24_0e5b_451c_b215_1bfce40d0fe6);
/// `Participants`.
pub const PARTICIPANTS: Uuid = Uuid::from_u128(0x0b9b1a35_2068_4431_8f7d_2350f958a728);
/// `Claims`.
pub const CLAIMS: Uuid = Uuid::from_u128(0xe614cce1_c4ce_4586_8304_fd1237119eb2);
/// `Types`.
pub const TYPES: Uuid = Uuid::from_u128(0x8f151ba4_de20_4e3c_9cb4_99ddf96f48f1);
/// `Topics`.
pub const TOPICS: Uuid = Uuid::from_u128(0x806d52bc_27e9_4c91_93c0_57978b093351);
/// `Following`.
pub const FOLLOWING: Uuid = Uuid::from_u128(0xf374b8f2_d331_48a3_a220_ba3648992e93);
/// The `Debate` type.
pub const DEBATE_TYPE: Uuid = Uuid::from_u128(0xfd51f935_2063_4617_be39_7b672b23364c);
/// The `Topic` type.
pub const TOPIC_TYPE: Uuid = Uuid::from_u128(0x5ef5a586_0f27_4d8e_8f6c_59ae5b3e89e2);

/// `Reply to` chains are followed at most this far (the SQL's `depth < 20`).
pub const MAX_THREAD_DEPTH: usize = 20;

/// One row of `public.relations`, the columns the compute reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rel {
    pub type_id: Uuid,
    pub from: Uuid,
    pub to: Uuid,
    pub space: Uuid,
}

/// One row of `public.user_votes`, the columns the compute reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vote {
    pub user: Uuid,
    pub object: Uuid,
    pub object_type: i16,
    pub vote_kind: i16,
    pub vote_type: i16,
    pub voted_at_us: i64,
}

/// One row of `personalization.external_interest_signals`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct External {
    pub user: Uuid,
    pub object: Uuid,
    pub kind: String,
    pub occurred_us: i64,
    pub recorded_us: i64,
}

/// What the compute reads from the graph: the statements it will read once the ranking service
/// holds them. `entity_created` is `entities.created_at`, epoch seconds as text, as stored.
#[derive(Debug, Default, Clone)]
pub struct Graph {
    pub personal_spaces: HashSet<Uuid>,
    pub votes: Vec<Vote>,
    pub relations: Vec<Rel>,
    pub entity_created: HashMap<Uuid, Option<String>>,
    pub entity_updated: HashMap<Uuid, Option<String>>,
    pub external: Vec<External>,
}

impl Graph {
    fn typed(&self, entity: Uuid, ty: Uuid) -> bool {
        self.relations
            .iter()
            .any(|r| r.type_id == TYPES && r.from == entity && r.to == ty)
    }
}

/// `personalization.epoch_text_ts`: epoch seconds as text to microseconds, `None` for anything else.
pub fn epoch_text_us(t: &str) -> Option<i64> {
    let ok = (1..=12).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_digit());
    if !ok {
        return None;
    }
    let secs: i64 = t.parse().ok()?;
    (secs > 0).then(|| secs * 1_000_000)
}

fn created_us(g: &Graph, entity: Uuid) -> Option<i64> {
    g.entity_created
        .get(&entity)
        .and_then(|t| t.as_deref())
        .and_then(epoch_text_us)
}

/// One activity event: a row of `user_interest_events`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Event {
    pub user: Uuid,
    pub kind: String,
    pub source: Uuid,
    pub object: Uuid,
    pub object_is_topic: bool,
    pub occurred_us: Option<i64>,
}

fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// `personalization.user_interest_events(user_ids)` at its latest definition (0107).
pub fn derive_events(g: &Graph, user_ids: &[Uuid]) -> Vec<Event> {
    let wanted: HashSet<Uuid> = user_ids.iter().copied().collect();
    // Only Personal spaces are people.
    let users: HashSet<Uuid> = wanted
        .iter()
        .copied()
        .filter(|u| g.personal_spaces.contains(u))
        .collect();
    let mut out: Vec<Event> = Vec::new();

    // Reply-to parent of an entity: the smallest `to` among its Reply to relations.
    let mut parent: HashMap<Uuid, Uuid> = HashMap::new();
    for r in g.relations.iter().filter(|r| r.type_id == REPLY_TO) {
        parent
            .entry(r.from)
            .and_modify(|p| *p = (*p).min(r.to))
            .or_insert(r.to);
    }

    // held: a user's responses on entities, removals (vote_type 2) dropped, people only.
    struct Held {
        user: Uuid,
        object: Uuid,
        voted: i64,
        interested_in_topic: bool,
    }
    let held: Vec<Held> = g
        .votes
        .iter()
        .filter(|v| {
            wanted.contains(&v.user)
                && v.object_type == 0
                && matches!(v.vote_type, 0 | 1)
                && users.contains(&v.user)
        })
        .map(|v| Held {
            user: v.user,
            object: v.object,
            voted: v.voted_at_us,
            interested_in_topic: v.vote_kind == 3 && g.typed(v.object, TOPIC_TYPE),
        })
        .collect();

    // Votes: everything but an Interested on a Topic, one per (user, object).
    let mut votes: BTreeMap<(Uuid, Uuid), i64> = BTreeMap::new();
    let mut follows: BTreeMap<(Uuid, Uuid), i64> = BTreeMap::new();
    for h in &held {
        let m = if h.interested_in_topic {
            &mut follows
        } else {
            &mut votes
        };
        m.entry((h.user, h.object))
            .and_modify(|t| *t = (*t).max(h.voted))
            .or_insert(h.voted);
    }
    for ((user, object), t) in votes {
        out.push(Event {
            user,
            kind: "vote".into(),
            source: object,
            object,
            object_is_topic: false,
            occurred_us: Some(t),
        });
    }

    // Comments: a Reply to authored in the user's own space, walked up to the thing the thread is
    // about. One root per (user, comment); one event per (user, root).
    // DISTINCT ON (user, comment) keeps the deepest row. At equal depth the SQL orders by nothing
    // else, so its pick is whatever row comes first; the twin takes the smaller target, which
    // agrees only by chance. A comment with two equally deep roots is a known shadow mismatch.
    let mut best: BTreeMap<(Uuid, Uuid), (usize, Uuid)> = BTreeMap::new();
    for c in g
        .relations
        .iter()
        .filter(|r| r.type_id == REPLY_TO && users.contains(&r.space))
    {
        let mut target = c.to;
        let mut depth = 0;
        while depth < MAX_THREAD_DEPTH {
            match parent.get(&target) {
                Some(p) => {
                    target = *p;
                    depth += 1;
                }
                None => break,
            }
        }
        best.entry((c.space, c.from))
            .and_modify(|(d, t)| {
                if depth > *d || (depth == *d && target < *t) {
                    *d = depth;
                    *t = target;
                }
            })
            .or_insert((depth, target));
    }
    let mut comments: BTreeMap<(Uuid, Uuid), Option<i64>> = BTreeMap::new(); // (user, root) -> max ts
    for ((user, comment), (_, root)) in best {
        // JOIN entities: the comment entity must exist.
        if !g.entity_created.contains_key(&comment) {
            continue;
        }
        let ts = created_us(g, comment);
        comments
            .entry((user, root))
            .and_modify(|t| *t = max_opt(*t, ts))
            .or_insert(ts);
    }
    for ((user, target), t) in comments {
        out.push(Event {
            user,
            kind: "comment".into(),
            source: target,
            object: target,
            object_is_topic: false,
            occurred_us: t,
        });
    }

    // Debates: each participant, for each thing debated (claim debate) or the topic it is filed
    // under (topic debate: no Claims relation of its own).
    let mut debate_events: BTreeSet<Event> = BTreeSet::new();
    for p in g
        .relations
        .iter()
        .filter(|r| r.type_id == PARTICIPANTS && users.contains(&r.to))
    {
        let debate = p.from;
        if !g.entity_created.contains_key(&debate) || !g.typed(debate, DEBATE_TYPE) {
            continue;
        }
        let ts = created_us(g, debate);
        let claims: Vec<Uuid> = g
            .relations
            .iter()
            .filter(|r| r.type_id == CLAIMS && r.from == debate)
            .map(|r| r.to)
            .collect();
        for claim in &claims {
            debate_events.insert(Event {
                user: p.to,
                kind: "debate".into(),
                source: debate,
                object: *claim,
                object_is_topic: false,
                occurred_us: ts,
            });
        }
        if claims.is_empty() {
            for tp in g
                .relations
                .iter()
                .filter(|r| r.type_id == TOPICS && r.from == debate)
            {
                debate_events.insert(Event {
                    user: p.to,
                    kind: "debate".into(),
                    source: debate,
                    object: tp.to,
                    object_is_topic: true,
                    occurred_us: ts,
                });
            }
        }
    }
    out.extend(debate_events);

    for ((user, topic), t) in follows {
        out.push(Event {
            user,
            kind: "follow".into(),
            source: topic,
            object: topic,
            object_is_topic: true,
            occurred_us: Some(t),
        });
    }

    for x in g
        .external
        .iter()
        .filter(|x| wanted.contains(&x.user) && users.contains(&x.user))
    {
        out.push(Event {
            user: x.user,
            kind: x.kind.clone(),
            source: x.object,
            object: x.object,
            object_is_topic: false,
            occurred_us: Some(x.occurred_us),
        });
    }
    out.sort();
    out
}

/// A row of `personalization.interest_signal_weights`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weight {
    pub weight: f64,
    pub decays: bool,
}

/// A row of `compute_user_topic_signals`, and of `user_topic_signals` (which adds `computed_us`).
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub user: Uuid,
    pub topic: Uuid,
    pub kind: String,
    pub decayed_count: f64,
    pub event_count: i32,
    pub last_event_us: Option<i64>,
    pub top_object: Uuid,
}

/// `personalization.compute_user_topic_signals`: per (user, topic, kind) as of `as_of_us`, the
/// decayed count of distinct sources, the raw count, the latest event and the strongest source.
///
/// `topics_of(object)` is the object's `Topics` relation targets. Rows come back sorted by
/// (user, topic, kind).
pub fn compute_signals(
    events: &[Event],
    weights: &HashMap<String, Weight>,
    topics_of: impl Fn(Uuid) -> Vec<Uuid>,
    half_life_days: f64,
    as_of_us: i64,
) -> Vec<Signal> {
    let half_life_s = half_life_days * 86400.0;
    // (user, topic, kind, source) -> (decays, max occurred)
    type CreditKey = (Uuid, Uuid, String, Uuid);
    let mut credited: BTreeMap<CreditKey, (bool, Option<i64>)> = BTreeMap::new();
    for e in events {
        let Some(w) = weights.get(&e.kind) else {
            continue; // JOIN interest_signal_weights
        };
        if e.occurred_us.is_none() && w.decays {
            continue; // a decaying event needs a time to decay from
        }
        let topics: BTreeSet<Uuid> = if e.object_is_topic {
            BTreeSet::from([e.object])
        } else {
            topics_of(e.object).into_iter().collect()
        };
        for t in topics {
            credited
                .entry((e.user, t, e.kind.clone(), e.source))
                .and_modify(|(_, occ)| *occ = max_opt(*occ, e.occurred_us))
                .or_insert((w.decays, e.occurred_us));
        }
    }
    struct Acc {
        sum: f64,
        n: i32,
        last: Option<i64>,
        top: Uuid,
        top_factor: f64,
    }
    let mut groups: BTreeMap<(Uuid, Uuid, String), Acc> = BTreeMap::new();
    for ((user, topic, kind, source), (decays, occ)) in credited {
        let factor = if decays {
            let age_s = (as_of_us - occ.unwrap_or(as_of_us)) as f64 / 1e6;
            0.5f64.powf(age_s.max(0.0) / half_life_s)
        } else {
            1.0
        };
        groups
            .entry((user, topic, kind))
            .and_modify(|a| {
                a.sum += factor;
                a.n += 1;
                a.last = max_opt(a.last, occ);
                // factor DESC, source ASC: sources arrive in ascending order, so strictly greater wins.
                if factor > a.top_factor {
                    a.top_factor = factor;
                    a.top = source;
                }
            })
            .or_insert(Acc {
                sum: factor,
                n: 1,
                last: occ,
                top: source,
                top_factor: factor,
            });
    }
    groups
        .into_iter()
        .map(|((user, topic, kind), a)| Signal {
            user,
            topic,
            kind,
            decayed_count: a.sum,
            event_count: a.n,
            last_event_us: a.last,
            top_object: a.top,
        })
        .collect()
}

/// A stored row: a [`Signal`] and when it was computed.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub signal: Signal,
    pub computed_us: i64,
}

/// `personalization.recompute_user_topic_interest`: replace the stored state for `users` with
/// `fresh`, every row stamped `as_of_us`. A user with no activity ends with no rows. Returns the
/// number of rows written.
pub fn replace_user_rows(
    stored: &mut Vec<Stored>,
    users: &HashSet<Uuid>,
    fresh: Vec<Signal>,
    as_of_us: i64,
) -> usize {
    stored.retain(|s| !users.contains(&s.signal.user));
    let n = fresh.len();
    stored.extend(fresh.into_iter().map(|signal| Stored {
        signal,
        computed_us: as_of_us,
    }));
    n
}

/// The activity sources `dirty_interest_users` reads, beyond [`Graph`].
fn since_text(since_us: i64) -> String {
    let secs = (since_us.div_euclid(1_000_000)).max(0);
    // entities.updated_at is epoch-seconds TEXT; before 2001 means "all".
    if secs < 1_000_000_000 {
        "0".to_string()
    } else {
        secs.to_string()
    }
}

/// `personalization.dirty_interest_users(since)`: Personal users with a vote at or after `since`, a
/// comment, debate or follow relation hanging from an entity updated at or after `since`, or an
/// external signal recorded at or after `since`. Sorted, distinct.
pub fn dirty_users(g: &Graph, since_us: i64) -> Vec<Uuid> {
    let st = since_text(since_us);
    let mut out: BTreeSet<Uuid> = BTreeSet::new();
    for v in &g.votes {
        if v.voted_at_us >= since_us {
            out.insert(v.user);
        }
    }
    for r in &g.relations {
        if !matches!(r.type_id, REPLY_TO | PARTICIPANTS | FOLLOWING) {
            continue;
        }
        let Some(Some(updated)) = g.entity_updated.get(&r.from) else {
            continue; // JOIN entities, and NULL >= text is not true
        };
        if updated.as_str() >= st.as_str() {
            out.insert(match r.type_id {
                REPLY_TO => r.space,
                PARTICIPANTS => r.to,
                _ => r.from,
            });
        }
    }
    for x in &g.external {
        if x.recorded_us >= since_us {
            out.insert(x.user);
        }
    }
    out.into_iter()
        .filter(|u| g.personal_spaces.contains(u))
        .collect()
}

/// What one sweep will do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepPlan {
    /// The nightly refit holds the lock: nothing runs and the cursor does not move.
    Skip,
    /// No cursor yet: everyone is recomputed (the backfill). `cutoff_us` is the epoch.
    Backfill,
    /// Users with activity since `cutoff_us` (the last run less the lookback).
    Incremental { cutoff_us: i64 },
}

/// `personalization.sweep_user_topic_interest`'s branching.
pub fn plan_sweep(lock_taken: bool, last_run_us: Option<i64>, lookback_us: i64) -> SweepPlan {
    if !lock_taken {
        return SweepPlan::Skip;
    }
    match last_run_us {
        None => SweepPlan::Backfill,
        Some(last) => SweepPlan::Incremental {
            cutoff_us: last - lookback_us,
        },
    }
}

/// The cursor after a committed sweep: it never moves backwards.
pub fn next_cursor(previous_us: Option<i64>, as_of_us: i64) -> i64 {
    previous_us.map_or(as_of_us, |p| p.max(as_of_us))
}

/// What the refit measured.
#[derive(Debug, Clone, PartialEq)]
pub struct RefitCompare {
    pub users: i32,
    pub signal_rows: i32,
    pub disagreeing_rows: i32,
    pub max_abs_diff: f64,
}

/// `personalization.refit_user_topic_interest`'s comparison (0117): the fresh recompute against the
/// stored state decayed from its `computed_at` to `as_of`. A row counts as disagreeing when the
/// counts differ by more than `1e-9 * max(1, fresh)` or the event counts differ, or when only one
/// side has it.
pub fn refit_compare(
    fresh: &[Signal],
    stored: &[Stored],
    weights: &HashMap<String, Weight>,
    half_life_days: f64,
    as_of_us: i64,
) -> RefitCompare {
    let half_life_s = half_life_days * 86400.0;
    type K = (Uuid, Uuid, String);
    let f: BTreeMap<K, &Signal> = fresh
        .iter()
        .map(|s| ((s.user, s.topic, s.kind.clone()), s))
        .collect();
    let o: BTreeMap<K, (f64, i32)> = stored
        .iter()
        .map(|s| {
            let decays = weights.get(&s.signal.kind).is_none_or(|w| w.decays);
            let factor = if decays {
                let age = ((as_of_us - s.computed_us) as f64 / 1e6).max(0.0);
                0.5f64.powf(age / half_life_s)
            } else {
                1.0
            };
            (
                (s.signal.user, s.signal.topic, s.signal.kind.clone()),
                (s.signal.decayed_count * factor, s.signal.event_count),
            )
        })
        .collect();
    let keys: BTreeSet<&K> = f.keys().chain(o.keys()).collect();
    let (mut n, mut max_diff) = (0, 0.0f64);
    for k in keys {
        let fresh_count = f.get(k).map_or(0.0, |s| s.decayed_count);
        let now_count = o.get(k).map_or(0.0, |x| x.0);
        let diff = (fresh_count - now_count).abs();
        let count_differs = f.get(k).map(|s| s.event_count) != o.get(k).map(|x| x.1);
        if diff > 1e-9 * fresh_count.max(1.0) || count_differs {
            n += 1;
        }
        max_diff = max_diff.max(diff);
    }
    RefitCompare {
        users: fresh.iter().map(|s| s.user).collect::<BTreeSet<_>>().len() as i32,
        signal_rows: fresh.len() as i32,
        disagreeing_rows: n,
        max_abs_diff: max_diff,
    }
}

/// An incoming snapshot row for `replace_external_interest_signals`. `None` fields are rows the SQL
/// drops (a NULL user, object or time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Incoming {
    pub user: Option<Uuid>,
    pub object: Option<Uuid>,
    pub occurred_us: Option<i64>,
}

/// What `replace_external_interest_signals` will do, before it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacePlan {
    pub rows_in: usize,
    pub placed: usize,
    /// `(user, object, occurred_us)` rows to insert.
    pub inserts: Vec<(Uuid, Uuid, i64)>,
    /// Existing rows whose time moved: `(user, object, new occurred_us)`.
    pub updates: Vec<(Uuid, Uuid, i64)>,
    /// Existing `(user, object)` rows the snapshot no longer holds.
    pub removes: Vec<(Uuid, Uuid)>,
    /// Users to recompute: distinct, sorted.
    pub changed_users: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceError {
    UnknownKind,
    /// A non-empty snapshot placed nothing while rows exist, and `allow_unplaced` is off.
    WouldRemoveAll {
        existing: usize,
        rows_in: usize,
    },
}

/// `personalization.replace_external_interest_signals(kind, rows, allow_unplaced)` as a plan over
/// the existing rows of that kind (`existing`: `(user, object) -> occurred_us`).
pub fn plan_external_replace(
    kind_known: bool,
    existing: &BTreeMap<(Uuid, Uuid), i64>,
    incoming: &[Incoming],
    personal: &HashSet<Uuid>,
    allow_unplaced: bool,
) -> Result<ReplacePlan, ReplaceError> {
    if !kind_known {
        return Err(ReplaceError::UnknownKind);
    }
    // DISTINCT ON (user, object): the earlier mark wins.
    let mut placed: BTreeMap<(Uuid, Uuid), i64> = BTreeMap::new();
    for r in incoming {
        let (Some(u), Some(o), Some(t)) = (r.user, r.object, r.occurred_us) else {
            continue;
        };
        if !personal.contains(&u) {
            continue;
        }
        placed
            .entry((u, o))
            .and_modify(|c| *c = (*c).min(t))
            .or_insert(t);
    }
    if !incoming.is_empty() && placed.is_empty() && !existing.is_empty() && !allow_unplaced {
        return Err(ReplaceError::WouldRemoveAll {
            existing: existing.len(),
            rows_in: incoming.len(),
        });
    }
    let removes: Vec<(Uuid, Uuid)> = existing
        .keys()
        .filter(|k| !placed.contains_key(k))
        .copied()
        .collect();
    let mut updates = Vec::new();
    let mut inserts = Vec::new();
    for (&(u, o), &t) in &placed {
        match existing.get(&(u, o)) {
            Some(&old) if old != t => updates.push((u, o, t)),
            Some(_) => {}
            None => inserts.push((u, o, t)),
        }
    }
    let changed: BTreeSet<Uuid> = removes
        .iter()
        .map(|k| k.0)
        .chain(updates.iter().map(|k| k.0))
        .chain(inserts.iter().map(|k| k.0))
        .collect();
    Ok(ReplacePlan {
        rows_in: incoming.len(),
        placed: placed.len(),
        inserts,
        updates,
        removes,
        changed_users: changed.into_iter().collect(),
    })
}

/// Why `for_you_candidates` left a candidate out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Excluded {
    Voted,
    Interested,
    NotInterested,
}

impl Excluded {
    pub fn as_str(self) -> &'static str {
        match self {
            Excluded::Voted => "voted",
            Excluded::Interested => "interested",
            Excluded::NotInterested => "not_interested",
        }
    }
}

/// A row of `for_you_candidates`.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub entity: Uuid,
    pub ranking_score: Option<f64>,
    pub topic_ids: Vec<Uuid>,
    pub excluded: Option<Excluded>,
}

/// `personalization.for_you_candidates(user, candidate_ids)`: each candidate in the order given
/// (duplicates kept), with its score, its distinct sorted topics, and why it is excluded.
///
/// `score_of` is a point lookup of the entity's ranking score (collection 0 once it is the home);
/// it is not the whole-order read that holds the score-table items. A held position reads `Voted`
/// before `Interested`, and that before `NotInterested`.
pub fn for_you_candidates(
    user: Uuid,
    candidate_ids: &[Uuid],
    score_of: impl Fn(Uuid) -> Option<f64>,
    topics_of: impl Fn(Uuid) -> Vec<Uuid>,
    votes: &[Vote],
    external: &[External],
) -> Vec<Candidate> {
    candidate_ids
        .iter()
        .map(|&id| {
            let topic_ids: Vec<Uuid> = topics_of(id)
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let voted = votes.iter().any(|v| {
                v.user == user && v.object == id && v.vote_kind == 1 && matches!(v.vote_type, 0 | 1)
            });
            let ext = |kind: &str| {
                external
                    .iter()
                    .any(|s| s.user == user && s.object == id && s.kind == kind)
            };
            let excluded = if voted {
                Some(Excluded::Voted)
            } else if ext("interested") {
                Some(Excluded::Interested)
            } else if ext("not_interested") {
                Some(Excluded::NotInterested)
            } else {
                None
            };
            Candidate {
                entity: id,
                ranking_score: score_of(id),
                topic_ids,
                excluded,
            }
        })
        .collect()
}

/// Whether two signal sets agree the way the shadow counts agreement: same keys, same event
/// counts and `last_event`/`top_object`, counts within `1e-9` relative. Returns the number of
/// differing rows.
pub fn count_signal_mismatches(a: &[Signal], b: &[Signal]) -> usize {
    type K = (Uuid, Uuid, String);
    let am: BTreeMap<K, &Signal> = a
        .iter()
        .map(|s| ((s.user, s.topic, s.kind.clone()), s))
        .collect();
    let bm: BTreeMap<K, &Signal> = b
        .iter()
        .map(|s| ((s.user, s.topic, s.kind.clone()), s))
        .collect();
    let keys: BTreeSet<&K> = am.keys().chain(bm.keys()).collect();
    keys.into_iter()
        .filter(|k| match (am.get(*k), bm.get(*k)) {
            (Some(x), Some(y)) => {
                (x.decayed_count - y.decayed_count).abs() > 1e-9 * x.decayed_count.abs().max(1.0)
                    || x.event_count != y.event_count
                    || x.last_event_us != y.last_event_us
                    || x.top_object != y.top_object
            }
            _ => true,
        })
        .count()
}

/// Whether `USER_INTEREST_RUST_SHADOW` is `1` or `true` in `get`'s environment.
pub fn shadow_enabled_from(get: impl Fn(&str) -> Option<String>) -> bool {
    matches!(
        get("USER_INTEREST_RUST_SHADOW").as_deref().map(str::trim),
        Some("1") | Some("true")
    )
}

/// Whether the shadow is on in the process environment.
pub fn shadow_enabled() -> bool {
    shadow_enabled_from(|k| std::env::var(k).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }
    const DAY: i64 = 86_400 * 1_000_000;
    const NOW: i64 = 1_800_000_000 * 1_000_000;

    fn weights() -> HashMap<String, Weight> {
        [
            ("vote", 1.0, true),
            ("comment", 1.5, true),
            ("debate", 2.0, true),
            ("follow", 3.0, false),
            ("interested", 2.0, true),
            ("not_interested", -3.0, true),
        ]
        .into_iter()
        .map(|(k, w, d)| {
            (
                k.to_string(),
                Weight {
                    weight: w,
                    decays: d,
                },
            )
        })
        .collect()
    }

    fn rel(t: Uuid, from: u128, to: u128, space: u128) -> Rel {
        Rel {
            type_id: t,
            from: id(from),
            to: id(to),
            space: id(space),
        }
    }

    fn vote(user: u128, object: u128, kind: i16, ty: i16, at: i64) -> Vote {
        Vote {
            user: id(user),
            object: id(object),
            object_type: 0,
            vote_kind: kind,
            vote_type: ty,
            voted_at_us: at,
        }
    }

    fn graph() -> Graph {
        let mut g = Graph::default();
        g.personal_spaces.insert(id(1));
        g.personal_spaces.insert(id(2));
        g
    }

    #[test]
    fn epoch_text_is_null_for_anything_but_positive_epoch_seconds() {
        assert_eq!(epoch_text_us("1700000000"), Some(1_700_000_000_000_000));
        assert_eq!(epoch_text_us("0"), None);
        assert_eq!(epoch_text_us(""), None);
        assert_eq!(epoch_text_us("17000000000000"), None); // 14 digits
        assert_eq!(epoch_text_us("17e5"), None);
        assert_eq!(epoch_text_us("-5"), None);
    }

    #[test]
    fn a_vote_is_one_event_per_object_and_a_removal_is_none() {
        let mut g = graph();
        g.votes = vec![
            vote(1, 10, 0, 0, NOW - 3 * DAY),
            vote(1, 10, 1, 1, NOW - DAY), // second kind on the same object: one event, latest time
            vote(1, 11, 0, 2, NOW),       // removal
            Vote {
                object_type: 1,
                ..vote(1, 12, 0, 0, NOW)
            }, // not an entity vote
            vote(9, 13, 0, 0, NOW),       // not a Personal space
        ];
        let ev = derive_events(&g, &[id(1), id(9)]);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, "vote");
        assert_eq!(ev[0].object, id(10));
        assert_eq!(ev[0].occurred_us, Some(NOW - DAY));
    }

    #[test]
    fn interested_on_a_topic_is_a_follow_not_a_vote_and_on_anything_else_a_vote() {
        let mut g = graph();
        g.relations.push(rel(TYPES, 20, TOPIC_TYPE.as_u128(), 0));
        g.votes = vec![vote(1, 20, 3, 0, NOW), vote(1, 21, 3, 0, NOW)];
        let ev = derive_events(&g, &[id(1)]);
        let follow = ev.iter().find(|e| e.kind == "follow").unwrap();
        assert_eq!((follow.object, follow.object_is_topic), (id(20), true));
        let vote = ev.iter().find(|e| e.kind == "vote").unwrap();
        assert_eq!(vote.object, id(21));
        assert_eq!(ev.len(), 2);
    }

    #[test]
    fn a_comment_walks_up_to_what_the_thread_is_about() {
        let mut g = graph();
        // comment 30 replies to comment 31, which replies to claim 32.
        g.relations.push(rel(REPLY_TO, 30, 31, 1));
        g.relations.push(rel(REPLY_TO, 31, 32, 2));
        g.entity_created.insert(id(30), Some("1700000000".into()));
        g.entity_created.insert(id(31), Some("1700000500".into()));
        let ev = derive_events(&g, &[id(1)]);
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].kind.as_str(), ev[0].object), ("comment", id(32)));
        assert_eq!(ev[0].occurred_us, Some(1_700_000_000_000_000));
    }

    #[test]
    fn ten_comments_on_one_claim_are_one_event_at_the_latest_time() {
        let mut g = graph();
        for (i, t) in [
            (40u128, "1700000100"),
            (41, "1700000900"),
            (42, "1700000300"),
        ] {
            g.relations.push(rel(REPLY_TO, i, 50, 1));
            g.entity_created.insert(id(i), Some(t.into()));
        }
        let ev = derive_events(&g, &[id(1)]);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].occurred_us, Some(1_700_000_900_000_000));
    }

    #[test]
    fn a_comment_chain_stops_at_depth_twenty_and_a_cycle_does_not_hang() {
        let mut g = graph();
        // 1 -> 2 -> 1 cycle of entity ids 60/61.
        g.relations.push(rel(REPLY_TO, 60, 61, 1));
        g.relations.push(rel(REPLY_TO, 61, 60, 1));
        g.entity_created.insert(id(60), Some("1700000000".into()));
        g.entity_created.insert(id(61), Some("1700000000".into()));
        let ev = derive_events(&g, &[id(1)]);
        // Both comments are authored in space 1 and each walks 20 hops around the cycle.
        assert_eq!(ev.len(), 2);
        let objects: BTreeSet<Uuid> = ev.iter().map(|e| e.object).collect();
        assert_eq!(objects, BTreeSet::from([id(60), id(61)]));
    }

    #[test]
    fn a_comment_whose_entity_is_missing_is_dropped() {
        let mut g = graph();
        g.relations.push(rel(REPLY_TO, 70, 71, 1));
        assert!(derive_events(&g, &[id(1)]).is_empty());
    }

    #[test]
    fn a_claim_debate_credits_each_claim_and_a_topic_debate_its_topic() {
        let mut g = graph();
        // claim debate 80 with two claims; topic debate 81 filed under topic 90.
        for d in [80u128, 81] {
            g.relations.push(rel(TYPES, d, DEBATE_TYPE.as_u128(), 0));
            g.relations.push(rel(PARTICIPANTS, d, 1, 0));
            g.entity_created.insert(id(d), Some("1700000000".into()));
        }
        g.relations.push(rel(CLAIMS, 80, 82, 0));
        g.relations.push(rel(CLAIMS, 80, 83, 0));
        g.relations.push(rel(TOPICS, 81, 90, 0));
        // a Topics relation on the claim debate must not make it a topic debate
        g.relations.push(rel(TOPICS, 80, 91, 0));
        let ev = derive_events(&g, &[id(1)]);
        let d: Vec<_> = ev.iter().filter(|e| e.kind == "debate").collect();
        assert_eq!(d.len(), 3);
        assert!(d
            .iter()
            .any(|e| e.source == id(80) && e.object == id(82) && !e.object_is_topic));
        assert!(d.iter().any(|e| e.source == id(80) && e.object == id(83)));
        assert!(d
            .iter()
            .any(|e| e.source == id(81) && e.object == id(90) && e.object_is_topic));
        assert!(!d.iter().any(|e| e.object == id(91)));
    }

    #[test]
    fn an_entity_that_is_not_typed_debate_credits_nobody() {
        let mut g = graph();
        g.relations.push(rel(PARTICIPANTS, 85, 1, 0));
        g.relations.push(rel(CLAIMS, 85, 86, 0));
        g.entity_created.insert(id(85), Some("1700000000".into()));
        assert!(derive_events(&g, &[id(1)]).is_empty());
    }

    #[test]
    fn external_signals_flow_through_for_personal_users_only() {
        let mut g = graph();
        g.external = vec![
            External {
                user: id(1),
                object: id(5),
                kind: "not_interested".into(),
                occurred_us: NOW,
                recorded_us: NOW,
            },
            External {
                user: id(9),
                object: id(5),
                kind: "not_interested".into(),
                occurred_us: NOW,
                recorded_us: NOW,
            },
        ];
        let ev = derive_events(&g, &[id(1), id(9)]);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, "not_interested");
    }

    fn topics(map: &[(u128, &[u128])]) -> impl Fn(Uuid) -> Vec<Uuid> {
        let m: HashMap<Uuid, Vec<Uuid>> = map
            .iter()
            .map(|(k, v)| (id(*k), v.iter().map(|t| id(*t)).collect()))
            .collect();
        move |o| m.get(&o).cloned().unwrap_or_default()
    }

    fn ev(
        user: u128,
        kind: &str,
        source: u128,
        object: u128,
        is_topic: bool,
        at: Option<i64>,
    ) -> Event {
        Event {
            user: id(user),
            kind: kind.into(),
            source: id(source),
            object: id(object),
            object_is_topic: is_topic,
            occurred_us: at,
        }
    }

    #[test]
    fn decay_halves_every_half_life_and_a_follow_never_decays() {
        let events = vec![
            ev(1, "vote", 10, 10, false, Some(NOW - 30 * DAY)),
            ev(1, "follow", 7, 7, true, Some(NOW - 900 * DAY)),
        ];
        let out = compute_signals(&events, &weights(), topics(&[(10, &[7])]), 30.0, NOW);
        let vote = out.iter().find(|s| s.kind == "vote").unwrap();
        assert!((vote.decayed_count - 0.5).abs() < 1e-12);
        let follow = out.iter().find(|s| s.kind == "follow").unwrap();
        assert_eq!(follow.decayed_count, 1.0);
    }

    #[test]
    fn a_future_event_does_not_count_more_than_one() {
        let events = vec![ev(1, "vote", 10, 10, false, Some(NOW + 5 * DAY))];
        let out = compute_signals(&events, &weights(), topics(&[(10, &[7])]), 30.0, NOW);
        assert_eq!(out[0].decayed_count, 1.0);
    }

    #[test]
    fn two_claims_sharing_a_topic_count_once_for_one_debate() {
        let events = vec![
            ev(1, "debate", 80, 82, false, Some(NOW)),
            ev(1, "debate", 80, 83, false, Some(NOW - DAY)),
        ];
        let out = compute_signals(
            &events,
            &weights(),
            topics(&[(82, &[7]), (83, &[7])]),
            30.0,
            NOW,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].event_count, 1);
        assert_eq!(out[0].last_event_us, Some(NOW));
    }

    #[test]
    fn an_event_with_no_time_is_dropped_unless_it_does_not_decay() {
        let events = vec![
            ev(1, "vote", 10, 10, false, None),
            ev(1, "follow", 7, 7, true, None),
        ];
        let out = compute_signals(&events, &weights(), topics(&[(10, &[7])]), 30.0, NOW);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "follow");
        assert_eq!(out[0].last_event_us, None);
    }

    #[test]
    fn an_unknown_kind_and_an_untagged_object_produce_nothing() {
        let events = vec![
            ev(1, "mystery", 10, 10, false, Some(NOW)),
            ev(1, "vote", 11, 11, false, Some(NOW)),
        ];
        assert!(compute_signals(&events, &weights(), topics(&[(10, &[7])]), 30.0, NOW).is_empty());
    }

    #[test]
    fn the_top_object_is_the_freshest_source_ties_to_the_smaller_id() {
        let events = vec![
            ev(1, "vote", 12, 12, false, Some(NOW - 10 * DAY)),
            ev(1, "vote", 11, 11, false, Some(NOW - 10 * DAY)),
            ev(1, "vote", 13, 13, false, Some(NOW - 20 * DAY)),
        ];
        let t = topics(&[(11, &[7]), (12, &[7]), (13, &[7])]);
        let out = compute_signals(&events, &weights(), t, 30.0, NOW);
        assert_eq!(out[0].event_count, 3);
        assert_eq!(out[0].top_object, id(11));
        let fresher = vec![
            ev(1, "vote", 12, 12, false, Some(NOW - 10 * DAY)),
            ev(1, "vote", 11, 11, false, Some(NOW - 20 * DAY)),
        ];
        let t = topics(&[(11, &[7]), (12, &[7])]);
        assert_eq!(
            compute_signals(&fresher, &weights(), t, 30.0, NOW)[0].top_object,
            id(12)
        );
    }

    #[test]
    fn replace_user_rows_removes_a_user_who_has_no_activity_any_more() {
        let sig = |u: u128, t: u128| Signal {
            user: id(u),
            topic: id(t),
            kind: "vote".into(),
            decayed_count: 1.0,
            event_count: 1,
            last_event_us: Some(NOW),
            top_object: id(9),
        };
        let mut stored = vec![
            Stored {
                signal: sig(1, 7),
                computed_us: 1,
            },
            Stored {
                signal: sig(2, 7),
                computed_us: 1,
            },
        ];
        let users: HashSet<Uuid> = [id(1)].into();
        let n = replace_user_rows(&mut stored, &users, vec![], NOW);
        assert_eq!(n, 0);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].signal.user, id(2));
        let n = replace_user_rows(&mut stored, &users, vec![sig(1, 8)], NOW);
        assert_eq!((n, stored.len()), (1, 2));
        assert!(stored
            .iter()
            .any(|s| s.signal.topic == id(8) && s.computed_us == NOW));
    }

    #[test]
    fn dirty_users_reads_votes_relations_and_external_signals_and_only_personal_users() {
        let mut g = graph();
        let since = 1_700_000_000_i64 * 1_000_000;
        g.votes = vec![
            vote(1, 10, 0, 0, since),     // at the bound counts
            vote(2, 10, 0, 0, since - 1), // before it does not
        ];
        // a comment authored in space 2 on an entity updated after the bound
        g.relations.push(rel(REPLY_TO, 30, 31, 2));
        g.entity_updated.insert(id(30), Some("1700000001".into()));
        // a debate whose participant is 9: not a Personal space, so left out
        g.relations.push(rel(PARTICIPANTS, 32, 9, 0));
        g.entity_updated.insert(id(32), Some("1700000001".into()));
        let d = dirty_users(&g, since);
        assert_eq!(d, vec![id(1), id(2)]);
        // an updated_at that is not newer than the bound, or is NULL, marks nobody
        g.entity_updated.insert(id(30), Some("1699999999".into()));
        g.entity_updated.insert(id(32), None);
        assert_eq!(dirty_users(&g, since), vec![id(1)]);
        g.external.push(External {
            user: id(2),
            object: id(1),
            kind: "interested".into(),
            occurred_us: 0,
            recorded_us: since + 5,
        });
        assert_eq!(dirty_users(&g, since), vec![id(1), id(2)]);
    }

    #[test]
    fn a_follower_is_the_from_entity_and_a_debater_the_to_entity() {
        let mut g = graph();
        g.relations.push(rel(FOLLOWING, 1, 99, 1));
        g.relations.push(rel(PARTICIPANTS, 40, 2, 0));
        g.entity_updated.insert(id(1), Some("1700000100".into()));
        g.entity_updated.insert(id(40), Some("1700000100".into()));
        assert_eq!(dirty_users(&g, 1_700_000_000_000_000), vec![id(1), id(2)]);
    }

    #[test]
    fn before_2001_since_means_every_updated_entity() {
        assert_eq!(since_text(0), "0");
        assert_eq!(since_text(-5), "0");
        assert_eq!(since_text(999_999_999 * 1_000_000), "0");
        assert_eq!(since_text(1_700_000_000_500_000), "1700000000");
    }

    #[test]
    fn the_sweep_skips_on_the_lock_backfills_without_a_cursor_and_looks_back_otherwise() {
        assert_eq!(plan_sweep(false, Some(5), 1), SweepPlan::Skip);
        assert_eq!(plan_sweep(false, None, 1), SweepPlan::Skip);
        assert_eq!(plan_sweep(true, None, 100), SweepPlan::Backfill);
        assert_eq!(
            plan_sweep(true, Some(1000), 100),
            SweepPlan::Incremental { cutoff_us: 900 }
        );
    }

    #[test]
    fn the_cursor_never_moves_backwards() {
        assert_eq!(next_cursor(None, 10), 10);
        assert_eq!(next_cursor(Some(20), 10), 20);
        assert_eq!(next_cursor(Some(5), 10), 10);
    }

    fn signal(u: u128, t: u128, kind: &str, count: f64, n: i32) -> Signal {
        Signal {
            user: id(u),
            topic: id(t),
            kind: kind.into(),
            decayed_count: count,
            event_count: n,
            last_event_us: Some(NOW),
            top_object: id(9),
        }
    }

    #[test]
    fn refit_agrees_when_the_stored_state_decayed_to_now_equals_the_recompute() {
        // Stored one half-life ago with count 2.0 decays to 1.0 now.
        let stored = vec![Stored {
            signal: signal(1, 7, "vote", 2.0, 3),
            computed_us: NOW - 30 * DAY,
        }];
        let fresh = vec![signal(1, 7, "vote", 1.0, 3)];
        let r = refit_compare(&fresh, &stored, &weights(), 30.0, NOW);
        assert_eq!((r.users, r.signal_rows, r.disagreeing_rows), (1, 1, 0));
        assert!(r.max_abs_diff < 1e-9);
    }

    #[test]
    fn refit_counts_a_missing_extra_or_changed_row() {
        let stored = vec![
            Stored {
                signal: signal(1, 7, "vote", 1.0, 1),
                computed_us: NOW,
            },
            Stored {
                signal: signal(1, 8, "vote", 1.0, 1),
                computed_us: NOW,
            }, // only stored
        ];
        let fresh = vec![
            signal(1, 7, "vote", 1.0, 2), // count differs
            signal(1, 9, "vote", 1.0, 1), // only fresh
        ];
        let r = refit_compare(&fresh, &stored, &weights(), 30.0, NOW);
        assert_eq!(r.disagreeing_rows, 3);
        assert_eq!(r.max_abs_diff, 1.0);
        assert_eq!(r.users, 1);
    }

    #[test]
    fn a_follow_row_is_not_decayed_in_the_comparison() {
        let stored = vec![Stored {
            signal: signal(1, 7, "follow", 1.0, 1),
            computed_us: NOW - 300 * DAY,
        }];
        let fresh = vec![signal(1, 7, "follow", 1.0, 1)];
        assert_eq!(
            refit_compare(&fresh, &stored, &weights(), 30.0, NOW).disagreeing_rows,
            0
        );
    }

    fn personal() -> HashSet<Uuid> {
        [id(1), id(2)].into()
    }

    #[test]
    fn a_snapshot_inserts_updates_removes_and_names_the_users_to_recompute() {
        let existing: BTreeMap<(Uuid, Uuid), i64> = [
            ((id(1), id(10)), 100),
            ((id(1), id(11)), 100),
            ((id(2), id(12)), 100),
        ]
        .into();
        let incoming = [
            Incoming {
                user: Some(id(1)),
                object: Some(id(10)),
                occurred_us: Some(100),
            }, // unchanged
            Incoming {
                user: Some(id(1)),
                object: Some(id(11)),
                occurred_us: Some(200),
            }, // moved
            Incoming {
                user: Some(id(2)),
                object: Some(id(13)),
                occurred_us: Some(50),
            }, // new
            // (2,12) is gone
            Incoming {
                user: Some(id(9)),
                object: Some(id(14)),
                occurred_us: Some(1),
            }, // not personal
            Incoming {
                user: None,
                object: Some(id(15)),
                occurred_us: Some(1),
            }, // null user
        ];
        let p = plan_external_replace(true, &existing, &incoming, &personal(), false).unwrap();
        assert_eq!((p.rows_in, p.placed), (5, 3));
        assert_eq!(p.inserts, vec![(id(2), id(13), 50)]);
        assert_eq!(p.updates, vec![(id(1), id(11), 200)]);
        assert_eq!(p.removes, vec![(id(2), id(12))]);
        assert_eq!(p.changed_users, vec![id(1), id(2)]);
    }

    #[test]
    fn the_earlier_mark_wins_within_a_snapshot() {
        let incoming = [
            Incoming {
                user: Some(id(1)),
                object: Some(id(10)),
                occurred_us: Some(300),
            },
            Incoming {
                user: Some(id(1)),
                object: Some(id(10)),
                occurred_us: Some(100),
            },
        ];
        let p =
            plan_external_replace(true, &BTreeMap::new(), &incoming, &personal(), false).unwrap();
        assert_eq!(p.inserts, vec![(id(1), id(10), 100)]);
    }

    #[test]
    fn a_snapshot_that_would_remove_everything_is_refused_unless_allowed() {
        let existing: BTreeMap<(Uuid, Uuid), i64> = [((id(1), id(10)), 100)].into();
        let incoming = [Incoming {
            user: Some(id(9)),
            object: Some(id(10)),
            occurred_us: Some(1),
        }];
        assert_eq!(
            plan_external_replace(true, &existing, &incoming, &personal(), false),
            Err(ReplaceError::WouldRemoveAll {
                existing: 1,
                rows_in: 1
            })
        );
        let p = plan_external_replace(true, &existing, &incoming, &personal(), true).unwrap();
        assert_eq!(p.removes.len(), 1);
        // An empty snapshot is a real "nobody", not a refusal.
        assert!(plan_external_replace(true, &existing, &[], &personal(), false).is_ok());
    }

    #[test]
    fn an_unknown_kind_is_an_error() {
        assert_eq!(
            plan_external_replace(false, &BTreeMap::new(), &[], &personal(), false),
            Err(ReplaceError::UnknownKind)
        );
    }

    #[test]
    fn for_you_candidates_keeps_order_and_duplicates_and_orders_the_reasons() {
        let user = id(1);
        let votes = vec![
            vote(1, 10, 1, 0, NOW), // held stance vote
            vote(1, 11, 1, 2, NOW), // removal: not held
            vote(1, 12, 0, 0, NOW), // not a stance vote
            vote(2, 13, 1, 0, NOW), // someone else's
        ];
        let ext = vec![
            External {
                user: id(1),
                object: id(10),
                kind: "interested".into(),
                occurred_us: 0,
                recorded_us: 0,
            },
            External {
                user: id(1),
                object: id(14),
                kind: "interested".into(),
                occurred_us: 0,
                recorded_us: 0,
            },
            External {
                user: id(1),
                object: id(15),
                kind: "not_interested".into(),
                occurred_us: 0,
                recorded_us: 0,
            },
            External {
                user: id(1),
                object: id(15),
                kind: "interested".into(),
                occurred_us: 0,
                recorded_us: 0,
            },
        ];
        let ids = [id(15), id(10), id(11), id(10), id(14), id(13), id(16)];
        let t = topics(&[(11, &[8, 7, 8])]);
        let out = for_you_candidates(
            user,
            &ids,
            |e| (e == id(11)).then_some(2.5),
            t,
            &votes,
            &ext,
        );
        let order: Vec<_> = out.iter().map(|c| c.entity).collect();
        assert_eq!(order, ids.to_vec());
        let ex: Vec<_> = out.iter().map(|c| c.excluded).collect();
        assert_eq!(
            ex,
            vec![
                Some(Excluded::Interested), // interested outranks not_interested
                Some(Excluded::Voted),
                None,
                Some(Excluded::Voted),
                Some(Excluded::Interested),
                None,
                None
            ]
        );
        assert_eq!(out[2].ranking_score, Some(2.5));
        assert_eq!(out[2].topic_ids, vec![id(7), id(8)]); // distinct, sorted
        assert_eq!(out[0].ranking_score, None);
    }

    #[test]
    fn mismatch_counting_is_tolerant_of_float_order_only() {
        let a = vec![signal(1, 7, "vote", 1.0, 1)];
        let mut b = a.clone();
        b[0].decayed_count = 1.0 + 1e-12;
        assert_eq!(count_signal_mismatches(&a, &b), 0);
        b[0].decayed_count = 1.001;
        assert_eq!(count_signal_mismatches(&a, &b), 1);
        assert_eq!(count_signal_mismatches(&a, &[]), 1);
    }

    #[test]
    fn the_shadow_flag_is_on_only_for_1_or_true() {
        let on = |v: &'static str| shadow_enabled_from(move |_| Some(v.to_string()));
        assert!(on("1") && on("true") && on(" 1 "));
        assert!(!on("") && !on("0") && !on("yes") && !on("TRUE"));
        assert!(!shadow_enabled_from(|_| None));
    }
}
