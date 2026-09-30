//! Retiring per-space documents whose entity has been emptied out of the space (GEO-2548).
//!
//! Cleanup proposals ("Delete space data", "Batch merge duplicates", "Delete 100 news
//! stories…") remove an entity from a space by unsetting its values and deleting its
//! relations, with no `DeleteEntity`. kg-indexer applies that correctly. search-indexer only
//! ever cleared the indexed name fields, so the `{entity_id}_{space_id}` document stayed
//! behind with nothing in it: 290,441 of them on 2026-09-30, about 12% of the index.
//!
//! The indexer cannot decide emptiness from the edit alone. It indexes a handful of fields
//! and four relation types, while an entity belongs to a space as long as it has *any* value
//! or outgoing relation there. So the edit only nominates candidates, and Postgres decides,
//! with the rule `search-admin reconcile-orphans` uses:
//!
//! - an `UpdateEntity` that unsets any value nominates `(entity, space)`;
//! - a `DeleteRelation` nominates the relation, resolved later to its `from` entity.
//!
//! **Waiting for kg-indexer.** The two indexers consume `knowledge.edits` independently, and
//! kg-indexer is usually behind. Checking Postgres too early would see an entity whose values
//! kg-indexer has not written yet and retire a live document. So each candidate carries the
//! highest block search-indexer has seen touching that document, and is only checked once
//! kg-indexer's committed block (`max(edit_versions.block_number)`; it applies blocks in order,
//! one transaction each) has reached it. Any later event on a pending document raises the bar,
//! so a document emptied at block N and refilled at N+1 is never checked against N's state.
//!
//! **Ordering with the loader.** The processor runs the sweep between batches and sends the
//! retirements down the same channel as everything else, so they land after every earlier
//! write and before every later one. A later upsert recreates the document normally.
//!
//! **Bounded and lossy by design.** Candidates live in memory, capped at
//! [`RetireConfig::max_pending`], and expire after [`RetireConfig::ttl`] if kg-indexer never
//! reaches their block (a lost block, GEO-2884). A restart forgets them. Every loss is counted,
//! and the daily reconcile finds whatever this misses.

use std::collections::HashMap;
use std::env;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::consumer::{EntityEvent, EntityEventType};

/// Configuration for retiring emptied documents.
#[derive(Debug, Clone)]
pub struct RetireConfig {
    /// Whether to retire emptied documents at all (`RETIRE_EMPTY_DOCS`, default on).
    pub enabled: bool,
    /// How often the processor checks ready candidates (`RETIRE_SWEEP_INTERVAL_SECS`).
    pub sweep_interval: Duration,
    /// How long a candidate may wait for kg-indexer (`RETIRE_PENDING_TTL_SECS`).
    pub ttl: Duration,
    /// Most candidates held at once (`RETIRE_MAX_PENDING`). The largest cleanup seen so far
    /// emptied 55k documents from one space.
    pub max_pending: usize,
}

impl Default for RetireConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sweep_interval: Duration::from_secs(15),
            ttl: Duration::from_secs(3600),
            max_pending: 200_000,
        }
    }
}

impl RetireConfig {
    pub fn from_env() -> Self {
        let d = Self::default();
        let secs = |name: &str, default: Duration| {
            env::var(name)
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|s| *s > 0)
                .map(Duration::from_secs)
                .unwrap_or(default)
        };
        Self {
            enabled: env::var("RETIRE_EMPTY_DOCS")
                .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "false" | "0" | "off"))
                .unwrap_or(d.enabled),
            sweep_interval: secs("RETIRE_SWEEP_INTERVAL_SECS", d.sweep_interval),
            ttl: secs("RETIRE_PENDING_TTL_SECS", d.ttl),
            max_pending: env::var("RETIRE_MAX_PENDING")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(d.max_pending),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    /// Postgres may only be trusted for this candidate once kg-indexer has reached this block.
    block: u64,
    since: Instant,
}

#[derive(Debug, Default)]
struct State {
    pairs: HashMap<(Uuid, Uuid), Pending>,
    relations: HashMap<Uuid, Pending>,
    /// Highest block seen on any entity event. Stands in for events without edit metadata,
    /// and for documents only discovered at sweep time (resolved relations), which may have
    /// been touched by anything seen so far.
    max_seen_block: u64,
    overflow: u64,
}

impl State {
    fn len(&self) -> usize {
        self.pairs.len() + self.relations.len()
    }
}

/// Candidates that kg-indexer has caught up with, taken out of the tracker by a sweep.
#[derive(Debug, Default, PartialEq)]
pub struct Ready {
    /// Documents to check against Postgres.
    pub pairs: Vec<(Uuid, Uuid)>,
    /// Deleted relations to resolve to their document first.
    pub relations: Vec<Uuid>,
    /// Candidates dropped because kg-indexer never reached their block within the TTL.
    pub expired: u64,
    /// Candidates refused since the last sweep because the tracker was full.
    pub overflow: u64,
}

/// In-memory set of documents that may have been emptied, waiting for kg-indexer.
#[derive(Debug)]
pub struct RetireTracker {
    config: RetireConfig,
    state: Mutex<State>,
}

impl RetireTracker {
    pub fn new(config: RetireConfig) -> Self {
        Self {
            config,
            state: Mutex::new(State::default()),
        }
    }

    pub fn config(&self) -> &RetireConfig {
        &self.config
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // The state is plain data; a panic elsewhere cannot leave it half-updated.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Number of candidates waiting.
    pub fn pending(&self) -> usize {
        self.lock().len()
    }

    /// Record one entity event, in stream order.
    ///
    /// `resolved_relation` is the `(from_entity, space)` of a `DeleteRelation` when the local
    /// relation map already knows it, which saves resolving it through Postgres later.
    pub fn observe(
        &self,
        event: &EntityEvent,
        resolved_relation: Option<(Uuid, Uuid)>,
        now: Instant,
    ) {
        let mut st = self.lock();
        let block = event.block_number.unwrap_or(st.max_seen_block);
        st.max_seen_block = st.max_seen_block.max(block);
        let max_pending = self.config.max_pending;

        match event.event_type {
            EntityEventType::ValuesUnset => {
                enqueue_pair(
                    &mut st,
                    (event.entity_id, event.space_id),
                    block,
                    now,
                    max_pending,
                );
            }
            EntityEventType::DeleteRelation => match (resolved_relation, event.relation_id) {
                (Some(pair), _) => enqueue_pair(&mut st, pair, block, now, max_pending),
                (None, Some(relation_id)) => {
                    if let Some(p) = st.relations.get_mut(&relation_id) {
                        p.block = p.block.max(block);
                    } else if st.len() < max_pending {
                        st.relations
                            .insert(relation_id, Pending { block, since: now });
                    } else {
                        st.overflow += 1;
                    }
                }
                (None, None) => {}
            },
            // Anything else that writes the document: if it is pending, Postgres must now
            // also reflect this later edit before it can be judged.
            EntityEventType::Upsert
            | EntityEventType::UnsetProperties
            | EntityEventType::CreateRelation
            | EntityEventType::Delete
            | EntityEventType::Restore => {
                if let Some(p) = st.pairs.get_mut(&(event.entity_id, event.space_id)) {
                    p.block = p.block.max(block);
                }
            }
        }
    }

    /// Take every candidate kg-indexer has caught up with, and drop the ones that have waited
    /// longer than the TTL.
    pub fn take_ready(&self, kg_block: u64, now: Instant) -> Ready {
        let ttl = self.config.ttl;
        let mut st = self.lock();
        let mut ready = Ready {
            overflow: std::mem::take(&mut st.overflow),
            ..Ready::default()
        };

        st.pairs.retain(|pair, p| {
            if p.block <= kg_block {
                ready.pairs.push(*pair);
                false
            } else if now.duration_since(p.since) > ttl {
                ready.expired += 1;
                false
            } else {
                true
            }
        });
        st.relations.retain(|relation_id, p| {
            if p.block <= kg_block {
                ready.relations.push(*relation_id);
                false
            } else if now.duration_since(p.since) > ttl {
                ready.expired += 1;
                false
            } else {
                true
            }
        });
        // Deterministic order, so a sweep's Postgres statements and logs are reproducible.
        ready.pairs.sort_unstable();
        ready.relations.sort_unstable();
        ready
    }

    /// Queue documents found by resolving deleted relations. Nothing is known about which
    /// events touched them, so they wait for everything seen so far; the ones kg-indexer has
    /// already covered are returned to be checked in this sweep.
    pub fn add_resolved(
        &self,
        pairs: impl IntoIterator<Item = (Uuid, Uuid)>,
        kg_block: u64,
        now: Instant,
    ) -> Vec<(Uuid, Uuid)> {
        let mut st = self.lock();
        let block = st.max_seen_block;
        let max_pending = self.config.max_pending;
        let mut now_ready = Vec::new();
        for pair in pairs {
            // A document already pending keeps its own bar, raised to this one.
            if let Some(p) = st.pairs.get_mut(&pair) {
                p.block = p.block.max(block);
            } else if block <= kg_block {
                now_ready.push(pair);
            } else {
                enqueue_pair(&mut st, pair, block, now, max_pending);
            }
        }
        now_ready.sort_unstable();
        now_ready.dedup();
        now_ready
    }

    /// Put candidates back after a Postgres error, to be retried on the next sweep.
    pub fn requeue(&self, pairs: &[(Uuid, Uuid)], relations: &[Uuid], kg_block: u64, now: Instant) {
        let mut st = self.lock();
        let max_pending = self.config.max_pending;
        for pair in pairs {
            enqueue_pair(&mut st, *pair, kg_block, now, max_pending);
        }
        for relation_id in relations {
            if st.relations.contains_key(relation_id) {
                continue;
            }
            if st.len() < max_pending {
                st.relations.insert(
                    *relation_id,
                    Pending {
                        block: kg_block,
                        since: now,
                    },
                );
            } else {
                st.overflow += 1;
            }
        }
    }
}

fn enqueue_pair(st: &mut State, pair: (Uuid, Uuid), block: u64, now: Instant, max_pending: usize) {
    if pair.0.is_nil() || pair.1.is_nil() {
        return;
    }
    if let Some(p) = st.pairs.get_mut(&pair) {
        p.block = p.block.max(block);
    } else if st.len() < max_pending {
        st.pairs.insert(pair, Pending { block, since: now });
    } else {
        st.overflow += 1;
    }
}

/// The OpenSearch document id for a pair.
pub fn doc_id(pair: &(Uuid, Uuid)) -> String {
    format!("{}_{}", pair.0, pair.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> RetireTracker {
        RetireTracker::new(RetireConfig::default())
    }

    fn ids() -> (Uuid, Uuid) {
        (Uuid::new_v4(), Uuid::new_v4())
    }

    fn unset(pair: (Uuid, Uuid), block: u64) -> EntityEvent {
        EntityEvent::values_unset(pair.0, pair.1).at_block(Some(block))
    }

    fn upsert(pair: (Uuid, Uuid), block: u64) -> EntityEvent {
        EntityEvent::upsert(pair.0, pair.1, Some("n".into()), None, None, None, None)
            .at_block(Some(block))
    }

    #[test]
    fn an_unset_waits_for_kg_indexer_to_reach_its_block() {
        let t = tracker();
        let now = Instant::now();
        let pair = ids();
        t.observe(&unset(pair, 100), None, now);

        // kg-indexer is behind: Postgres may not have the entity's values yet, so a check now
        // could retire a live document.
        assert!(t.take_ready(99, now).pairs.is_empty());
        assert_eq!(t.pending(), 1);

        assert_eq!(t.take_ready(100, now).pairs, vec![pair]);
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn a_later_write_raises_the_bar() {
        // Emptied at 100, given a name again at 105. Checking once kg-indexer reaches 100 would
        // see the empty state and delete the document search-indexer has already rewritten.
        let t = tracker();
        let now = Instant::now();
        let pair = ids();
        t.observe(&unset(pair, 100), None, now);
        t.observe(&upsert(pair, 105), None, now);

        assert!(t.take_ready(104, now).pairs.is_empty());
        assert_eq!(t.take_ready(105, now).pairs, vec![pair]);
    }

    #[test]
    fn writes_to_documents_that_are_not_pending_are_ignored() {
        let t = tracker();
        let now = Instant::now();
        t.observe(&upsert(ids(), 5), None, now);
        let rel = EntityEvent::create_relation(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        )
        .at_block(Some(6));
        t.observe(&rel, None, now);
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn repeated_unsets_keep_one_candidate() {
        let t = tracker();
        let now = Instant::now();
        let pair = ids();
        for block in [10, 12, 11] {
            t.observe(&unset(pair, block), None, now);
        }
        assert_eq!(t.pending(), 1);
        assert!(t.take_ready(11, now).pairs.is_empty());
        assert_eq!(t.take_ready(12, now).pairs, vec![pair]);
    }

    #[test]
    fn a_deleted_relation_the_map_knows_nominates_its_document() {
        let t = tracker();
        let now = Instant::now();
        let pair = ids();
        let ev = EntityEvent::delete_relation(Uuid::new_v4()).at_block(Some(7));
        t.observe(&ev, Some(pair), now);
        let ready = t.take_ready(7, now);
        assert_eq!(ready.pairs, vec![pair]);
        assert!(ready.relations.is_empty());
    }

    #[test]
    fn an_unknown_deleted_relation_is_resolved_at_sweep_time() {
        let t = tracker();
        let now = Instant::now();
        let relation_id = Uuid::new_v4();
        t.observe(
            &EntityEvent::delete_relation(relation_id).at_block(Some(7)),
            None,
            now,
        );
        let ready = t.take_ready(7, now);
        assert_eq!(ready.relations, vec![relation_id]);
        assert!(ready.pairs.is_empty());
    }

    #[test]
    fn a_resolved_document_waits_for_everything_seen_so_far() {
        // Its own history is unknown until now, so it cannot be judged before kg-indexer has
        // reached the newest block search-indexer has processed.
        let t = tracker();
        let now = Instant::now();
        let relation_id = Uuid::new_v4();
        t.observe(
            &EntityEvent::delete_relation(relation_id).at_block(Some(7)),
            None,
            now,
        );
        t.observe(&upsert(ids(), 20), None, now);
        let ready = t.take_ready(7, now);
        assert_eq!(ready.relations, vec![relation_id]);

        let pair = ids();
        assert!(t.add_resolved([pair], 7, now).is_empty());
        assert!(t.take_ready(19, now).pairs.is_empty());
        assert_eq!(t.take_ready(20, now).pairs, vec![pair]);

        // Once kg-indexer has caught up with everything, a resolved document is checked at once.
        let other = ids();
        assert_eq!(t.add_resolved([other, other], 20, now), vec![other]);
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn events_without_a_block_use_the_highest_block_seen() {
        let t = tracker();
        let now = Instant::now();
        t.observe(&upsert(ids(), 50), None, now);
        let pair = ids();
        t.observe(&EntityEvent::values_unset(pair.0, pair.1), None, now);
        assert!(t.take_ready(49, now).pairs.is_empty());
        assert_eq!(t.take_ready(50, now).pairs, vec![pair]);
    }

    #[test]
    fn candidates_expire_when_kg_indexer_never_arrives() {
        let t = RetireTracker::new(RetireConfig {
            ttl: Duration::from_secs(60),
            ..RetireConfig::default()
        });
        let start = Instant::now();
        t.observe(&unset(ids(), 100), None, start);
        t.observe(
            &EntityEvent::delete_relation(Uuid::new_v4()).at_block(Some(100)),
            None,
            start,
        );

        let ready = t.take_ready(50, start + Duration::from_secs(30));
        assert_eq!((ready.expired, t.pending()), (0, 2));

        let ready = t.take_ready(50, start + Duration::from_secs(61));
        assert_eq!(ready.expired, 2);
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn a_full_tracker_counts_what_it_refuses() {
        let t = RetireTracker::new(RetireConfig {
            max_pending: 2,
            ..RetireConfig::default()
        });
        let now = Instant::now();
        let kept = ids();
        t.observe(&unset(kept, 1), None, now);
        t.observe(&unset(ids(), 1), None, now);
        t.observe(&unset(ids(), 1), None, now);
        // A document already pending can still be updated when full.
        t.observe(&unset(kept, 2), None, now);
        assert_eq!(t.pending(), 2);

        let ready = t.take_ready(10, now);
        assert_eq!(ready.overflow, 1);
        assert_eq!(ready.pairs.len(), 2);
        assert_eq!(t.take_ready(10, now).overflow, 0);
    }

    #[test]
    fn requeued_candidates_are_retried() {
        let t = tracker();
        let now = Instant::now();
        let pair = ids();
        let relation_id = Uuid::new_v4();
        t.requeue(&[pair], &[relation_id], 30, now);
        let ready = t.take_ready(30, now);
        assert_eq!(ready.pairs, vec![pair]);
        assert_eq!(ready.relations, vec![relation_id]);
    }

    #[test]
    fn nil_ids_are_never_candidates() {
        let t = tracker();
        let now = Instant::now();
        t.observe(&unset((Uuid::nil(), Uuid::new_v4()), 1), None, now);
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn doc_ids_match_the_indexer() {
        let e = Uuid::parse_str("3619f7b6-ab07-41f6-ba89-a80ee4528617").unwrap();
        let s = Uuid::parse_str("b5a31f81-82b0-4243-7ede-0f84ee02f104").unwrap();
        assert_eq!(
            doc_id(&(e, s)),
            "3619f7b6-ab07-41f6-ba89-a80ee4528617_b5a31f81-82b0-4243-7ede-0f84ee02f104"
        );
    }
}
