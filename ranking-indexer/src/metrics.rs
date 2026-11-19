//! Prometheus metrics for the ranking-indexer consumer.
//!
//! The recorder and the `/metrics` listener come from `hermes_instrumentation::metrics`,
//! installed in `main`. The CronJob binaries that share this crate never install one, so
//! the calls here are no-ops in them. Every series is registered at zero by [`register`]
//! so it exists from startup: a counter that first appears at 1 has no earlier sample, so
//! `increase()` cannot see its first event.
//!
//! No failure counter covers the transient path that gives up. After
//! `MAX_TRANSIENT_ATTEMPTS` the process exits without committing, by design, so Kafka
//! redelivers on restart. Its counter would die with the process before a scrape could
//! read it; the container restart count is the signal for that path.
//!
//! There is deliberately no position gauge. Edits and membership events only arrive when
//! someone writes, so any "last processed" value would stall through quiet stretches.
//! Per-partition lag is already watched by kafka-exporter.

pub const MESSAGES_PROCESSED: &str = "ranking_indexer_messages_processed_total";
pub const MESSAGES_SKIPPED: &str = "ranking_indexer_messages_skipped_total";
pub const TRANSIENT_RETRIES: &str = "ranking_indexer_transient_retries_total";
pub const PG_WRITES_SKIPPED: &str = "ranking_indexer_pg_writes_skipped_total";
/// Recomputes of a queued block (`ranks.pending_recomputes`, GEO-3591) that
/// failed while an edit drained the queue. The edit carries on; the block stays
/// queued for the hourly backfill.
pub const PENDING_RECOMPUTE_FAILURES: &str = "ranking_indexer_pending_recompute_failures_total";
/// Blocks whose queue entry is dead: the backfill gave up recomputing them.
pub const PENDING_RECOMPUTE_DEAD: &str = "ranking_indexer_pending_recompute_dead_blocks";
pub const USER_INTEREST_SHADOW_ROWS: &str = "ranking_indexer_user_interest_shadow_rows_total";
pub const USER_INTEREST_SHADOW_MISMATCHES: &str =
    "ranking_indexer_user_interest_shadow_mismatches_total";

/// Compares made by the `*_RUST_COMPUTE` shadows, by `flag` and `outcome`. The shadows run in the
/// CronJob binaries, which install no recorder, so the series are no-ops there until one is
/// installed and scraped; the shadow's log line is the reading in the meantime.
pub const RUST_COMPUTE_SHADOW: &str = "ranking_indexer_rust_compute_shadow_total";

/// One `*_RUST_COMPUTE` flag: a Rust twin of a SQL job that runs after it and only compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeFlag {
    DebateWatch,
    FeedSignals,
    EntityRankingScore,
    AccountExclusions,
    AccountVoteWeight,
    AccountWeights,
    AgendaListings,
    AgendaRescore,
    Anchor,
    PrimerStats,
    ClaimOverlap,
    TopicSuggestions,
    TopicCooccurrence,
    TopicRanking,
}

impl ComputeFlag {
    pub const ALL: [ComputeFlag; 14] = [
        ComputeFlag::DebateWatch,
        ComputeFlag::FeedSignals,
        ComputeFlag::EntityRankingScore,
        ComputeFlag::AccountExclusions,
        ComputeFlag::AccountVoteWeight,
        ComputeFlag::AccountWeights,
        ComputeFlag::AgendaListings,
        ComputeFlag::AgendaRescore,
        ComputeFlag::Anchor,
        ComputeFlag::PrimerStats,
        ComputeFlag::ClaimOverlap,
        ComputeFlag::TopicSuggestions,
        ComputeFlag::TopicCooccurrence,
        ComputeFlag::TopicRanking,
    ];

    /// The `flag` label value.
    pub fn as_str(self) -> &'static str {
        match self {
            ComputeFlag::DebateWatch => "debate_watch",
            ComputeFlag::FeedSignals => "feed_signals",
            ComputeFlag::EntityRankingScore => "entity_ranking_score",
            ComputeFlag::AccountExclusions => "account_exclusions",
            ComputeFlag::AccountVoteWeight => "account_vote_weight",
            ComputeFlag::AccountWeights => "account_weights",
            ComputeFlag::AgendaListings => "agenda_listings",
            ComputeFlag::AgendaRescore => "agenda_rescore",
            ComputeFlag::Anchor => "anchor",
            ComputeFlag::PrimerStats => "primer_stats",
            ComputeFlag::ClaimOverlap => "claim_overlap",
            ComputeFlag::TopicSuggestions => "topic_suggestions",
            ComputeFlag::TopicCooccurrence => "topic_cooccurrence",
            ComputeFlag::TopicRanking => "topic_ranking",
        }
    }

    /// The environment variable that turns the shadow on.
    pub fn env_var(self) -> &'static str {
        match self {
            ComputeFlag::DebateWatch => "DEBATE_WATCH_RUST_COMPUTE",
            ComputeFlag::FeedSignals => "FEED_SIGNALS_RUST_COMPUTE",
            ComputeFlag::EntityRankingScore => "ENTITY_RANKING_SCORE_RUST_COMPUTE",
            ComputeFlag::AccountExclusions => "ACCOUNT_EXCLUSIONS_RUST_COMPUTE",
            ComputeFlag::AccountVoteWeight => "ACCOUNT_VOTE_WEIGHT_RUST_COMPUTE",
            ComputeFlag::AccountWeights => "ACCOUNT_WEIGHTS_RUST_COMPUTE",
            ComputeFlag::AgendaListings => "AGENDA_LISTINGS_RUST_COMPUTE",
            ComputeFlag::AgendaRescore => "AGENDA_RESCORE_RUST_COMPUTE",
            ComputeFlag::Anchor => "ANCHOR_RUST_COMPUTE",
            ComputeFlag::PrimerStats => "PRIMER_STATS_RUST_COMPUTE",
            ComputeFlag::ClaimOverlap => "CLAIM_OVERLAP_RUST_COMPUTE",
            ComputeFlag::TopicSuggestions => "TOPIC_SUGGESTIONS_RUST_COMPUTE",
            ComputeFlag::TopicCooccurrence => "TOPIC_COOCCURRENCE_RUST_COMPUTE",
            ComputeFlag::TopicRanking => "TOPIC_RANKING_RUST_COMPUTE",
        }
    }
}

/// What one shadow compare found. `Failed`: an input read or the twin errored, so nothing was
/// compared. `Unsupported`: there was nothing to compare (for example no sample row yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowOutcome {
    Match,
    Mismatch,
    Failed,
    Unsupported,
}

impl ShadowOutcome {
    pub const ALL: [ShadowOutcome; 4] = [
        ShadowOutcome::Match,
        ShadowOutcome::Mismatch,
        ShadowOutcome::Failed,
        ShadowOutcome::Unsupported,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ShadowOutcome::Match => "match",
            ShadowOutcome::Mismatch => "mismatch",
            ShadowOutcome::Failed => "failed",
            ShadowOutcome::Unsupported => "unsupported",
        }
    }

    pub fn from_mismatches(n: usize) -> Self {
        if n == 0 {
            ShadowOutcome::Match
        } else {
            ShadowOutcome::Mismatch
        }
    }

    pub fn from_agreement(agrees: bool) -> Self {
        if agrees {
            ShadowOutcome::Match
        } else {
            ShadowOutcome::Mismatch
        }
    }
}

/// Counts one shadow compare. Like every call here, a no-op in a process with no recorder.
pub fn rust_compute_shadow(flag: ComputeFlag, outcome: ShadowOutcome) {
    metrics::counter!(RUST_COMPUTE_SHADOW, "flag" => flag.as_str(), "outcome" => outcome.as_str())
        .increment(1);
}

/// Which subscribed topic a message came from, as a label value.
#[derive(Debug, Clone, Copy)]
pub enum Topic {
    Edits,
    Membership,
}

impl Topic {
    fn as_str(self) -> &'static str {
        match self {
            Topic::Edits => "edits",
            Topic::Membership => "membership",
        }
    }
}

/// A Postgres write skipped because `RANKING_INDEXER_PG_WRITES` is off.
#[derive(Debug, Clone, Copy)]
pub enum PgWriteSite {
    AddMember,
    RemoveMember,
    AddEditor,
    RemoveEditor,
    UpsertRankingBlock,
    UpsertRanking,
    SetRankingBlock,
    ReplaceRankingItems,
    DeleteRankingItems,
    UnlinkRankings,
    DeleteRankings,
    FillRelationIds,
    ClearPendingRecompute,
    NotePendingRecomputeFailure,
    ReconcileClearedBallots,
    RecoverRanking,
    ReplaceRankPositionProjection,
    ReplaceRankingScores,
    AccountExclusionsReplace,
    AccountWeightsRefresh,
    AgendaListingsRefresh,
    FeedCompositionRenderedSample,
    FeedCompositionWindowSample,
    FeedSignalsApply,
    FeedSignalsRecord,
    NotInterestedApply,
    PrimerClaimsRefresh,
    ClaimOverlapSample,
    TopicInterestSweep,
    TopicInterestRefit,
    TopicRankingReconcile,
    TopicSuggestionsRefresh,
    StanceMapRun,
}

impl PgWriteSite {
    pub(crate) const ALL: [PgWriteSite; 33] = [
        PgWriteSite::AddMember,
        PgWriteSite::RemoveMember,
        PgWriteSite::AddEditor,
        PgWriteSite::RemoveEditor,
        PgWriteSite::UpsertRankingBlock,
        PgWriteSite::UpsertRanking,
        PgWriteSite::SetRankingBlock,
        PgWriteSite::ReplaceRankingItems,
        PgWriteSite::DeleteRankingItems,
        PgWriteSite::UnlinkRankings,
        PgWriteSite::DeleteRankings,
        PgWriteSite::FillRelationIds,
        PgWriteSite::ClearPendingRecompute,
        PgWriteSite::NotePendingRecomputeFailure,
        PgWriteSite::ReconcileClearedBallots,
        PgWriteSite::RecoverRanking,
        PgWriteSite::ReplaceRankPositionProjection,
        PgWriteSite::ReplaceRankingScores,
        PgWriteSite::AccountExclusionsReplace,
        PgWriteSite::AccountWeightsRefresh,
        PgWriteSite::AgendaListingsRefresh,
        PgWriteSite::FeedCompositionRenderedSample,
        PgWriteSite::FeedCompositionWindowSample,
        PgWriteSite::FeedSignalsApply,
        PgWriteSite::FeedSignalsRecord,
        PgWriteSite::NotInterestedApply,
        PgWriteSite::PrimerClaimsRefresh,
        PgWriteSite::ClaimOverlapSample,
        PgWriteSite::TopicInterestSweep,
        PgWriteSite::TopicInterestRefit,
        PgWriteSite::TopicRankingReconcile,
        PgWriteSite::TopicSuggestionsRefresh,
        PgWriteSite::StanceMapRun,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PgWriteSite::AddMember => "add_member",
            PgWriteSite::RemoveMember => "remove_member",
            PgWriteSite::AddEditor => "add_editor",
            PgWriteSite::RemoveEditor => "remove_editor",
            PgWriteSite::UpsertRankingBlock => "upsert_ranking_block",
            PgWriteSite::UpsertRanking => "upsert_ranking",
            PgWriteSite::SetRankingBlock => "set_ranking_block",
            PgWriteSite::ReplaceRankingItems => "replace_ranking_items",
            PgWriteSite::DeleteRankingItems => "delete_ranking_items",
            PgWriteSite::UnlinkRankings => "unlink_rankings",
            PgWriteSite::DeleteRankings => "delete_rankings",
            PgWriteSite::FillRelationIds => "fill_relation_ids",
            PgWriteSite::ClearPendingRecompute => "clear_pending_recompute",
            PgWriteSite::NotePendingRecomputeFailure => "note_pending_recompute_failure",
            PgWriteSite::ReconcileClearedBallots => "reconcile_cleared_ballots",
            PgWriteSite::RecoverRanking => "recover_ranking",
            PgWriteSite::ReplaceRankPositionProjection => "replace_rank_position_projection",
            PgWriteSite::ReplaceRankingScores => "replace_ranking_scores",
            PgWriteSite::AccountExclusionsReplace => "account_exclusions_replace",
            PgWriteSite::AccountWeightsRefresh => "account_weights_refresh",
            PgWriteSite::AgendaListingsRefresh => "agenda_listings_refresh",
            PgWriteSite::FeedCompositionRenderedSample => "feed_composition_rendered_sample",
            PgWriteSite::FeedCompositionWindowSample => "feed_composition_window_sample",
            PgWriteSite::FeedSignalsApply => "feed_signals_apply",
            PgWriteSite::FeedSignalsRecord => "feed_signals_record",
            PgWriteSite::NotInterestedApply => "not_interested_apply",
            PgWriteSite::PrimerClaimsRefresh => "primer_claims_refresh",
            PgWriteSite::ClaimOverlapSample => "claim_overlap_sample",
            PgWriteSite::TopicInterestSweep => "topic_interest_sweep",
            PgWriteSite::TopicInterestRefit => "topic_interest_refit",
            PgWriteSite::TopicRankingReconcile => "topic_ranking_reconcile",
            PgWriteSite::TopicSuggestionsRefresh => "topic_suggestions_refresh",
            PgWriteSite::StanceMapRun => "stance_map_run",
        }
    }
}

/// Describe every counter and register it at zero.
pub fn register() {
    metrics::describe_counter!(
        MESSAGES_PROCESSED,
        "Messages applied to the ranks schema and committed, including membership event \
         types the indexer deliberately ignores (SPACE_LEFT)"
    );
    metrics::describe_counter!(
        MESSAGES_SKIPPED,
        "Messages committed past without being applied because they can never succeed \
         (malformed payload, unknown role or event type)"
    );
    metrics::describe_counter!(
        TRANSIENT_RETRIES,
        "Retries after a transient database or Kafka error; exhausting them exits the process"
    );
    metrics::describe_counter!(
        USER_INTEREST_SHADOW_ROWS,
        "Stored user topic signal rows the user-interest Rust shadow compared"
    );
    metrics::describe_counter!(
        USER_INTEREST_SHADOW_MISMATCHES,
        "Rows where the user-interest Rust twin disagreed with the SQL (shadow only, never written)"
    );
    metrics::describe_counter!(
        PG_WRITES_SKIPPED,
        "Storage write calls skipped because RANKING_INDEXER_PG_WRITES is off, by site"
    );
    for site in PgWriteSite::ALL {
        metrics::counter!(PG_WRITES_SKIPPED, "site" => site.as_str()).absolute(0);
    }
    metrics::counter!(PENDING_RECOMPUTE_FAILURES).absolute(0);
    metrics::gauge!(PENDING_RECOMPUTE_DEAD).set(0.0);
    metrics::counter!(USER_INTEREST_SHADOW_ROWS).absolute(0);
    metrics::counter!(USER_INTEREST_SHADOW_MISMATCHES).absolute(0);
    metrics::describe_counter!(
        RUST_COMPUTE_SHADOW,
        "Compares a *_RUST_COMPUTE shadow made against the SQL, by flag and outcome \
         (match, mismatch, failed, unsupported). Shadow only: the SQL stays the answer"
    );
    for flag in ComputeFlag::ALL {
        for outcome in ShadowOutcome::ALL {
            metrics::counter!(RUST_COMPUTE_SHADOW, "flag" => flag.as_str(), "outcome" => outcome.as_str())
                .absolute(0);
        }
    }

    for topic in [Topic::Edits, Topic::Membership] {
        metrics::counter!(MESSAGES_PROCESSED, "topic" => topic.as_str()).absolute(0);
        metrics::counter!(MESSAGES_SKIPPED, "topic" => topic.as_str()).absolute(0);
        metrics::counter!(TRANSIENT_RETRIES, "topic" => topic.as_str()).absolute(0);
    }
}

pub fn pg_write_skipped(site: PgWriteSite) {
    metrics::counter!(PG_WRITES_SKIPPED, "site" => site.as_str()).increment(1);
}

pub fn pending_recompute_dead(blocks: i64) {
    metrics::gauge!(PENDING_RECOMPUTE_DEAD).set(blocks as f64);
}

pub fn pending_recompute_failed() {
    metrics::counter!(PENDING_RECOMPUTE_FAILURES).increment(1);
}

pub fn message_processed(topic: Topic) {
    metrics::counter!(MESSAGES_PROCESSED, "topic" => topic.as_str()).increment(1);
}

pub fn message_skipped(topic: Topic) {
    metrics::counter!(MESSAGES_SKIPPED, "topic" => topic.as_str()).increment(1);
}

pub fn transient_retry(topic: Topic) {
    metrics::counter!(TRANSIENT_RETRIES, "topic" => topic.as_str()).increment(1);
}

/// One user-interest shadow run: `rows` stored rows compared, `mismatches` of them differing.
pub fn user_interest_shadow(rows: usize, mismatches: usize) {
    metrics::counter!(USER_INTEREST_SHADOW_ROWS).increment(rows as u64);
    metrics::counter!(USER_INTEREST_SHADOW_MISMATCHES).increment(mismatches as u64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;

    fn render_with(f: impl FnOnce()) -> String {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, f);
        handle.render()
    }

    #[test]
    fn register_exposes_every_counter_at_zero() {
        let rendered = render_with(register);
        for name in [MESSAGES_PROCESSED, MESSAGES_SKIPPED, TRANSIENT_RETRIES] {
            for topic in ["edits", "membership"] {
                let line = format!("{name}{{topic=\"{topic}\"}} 0");
                assert!(rendered.contains(&line), "missing `{line}` in:\n{rendered}");
            }
        }
    }

    #[test]
    fn the_shadow_counters_start_at_zero_and_add_each_run() {
        let rendered = render_with(|| {
            register();
            user_interest_shadow(10, 0);
            user_interest_shadow(5, 2);
        });
        assert!(rendered.contains("ranking_indexer_user_interest_shadow_rows_total 15"));
        assert!(rendered.contains("ranking_indexer_user_interest_shadow_mismatches_total 2"));
        let zero = render_with(register);
        assert!(zero.contains("ranking_indexer_user_interest_shadow_mismatches_total 0"));
    }

    #[test]
    fn counters_split_by_topic() {
        let rendered = render_with(|| {
            register();
            message_processed(Topic::Edits);
            message_processed(Topic::Edits);
            message_processed(Topic::Membership);
            message_skipped(Topic::Membership);
            transient_retry(Topic::Edits);
        });
        for line in [
            "ranking_indexer_messages_processed_total{topic=\"edits\"} 2",
            "ranking_indexer_messages_processed_total{topic=\"membership\"} 1",
            "ranking_indexer_messages_skipped_total{topic=\"membership\"} 1",
            "ranking_indexer_messages_skipped_total{topic=\"edits\"} 0",
            "ranking_indexer_transient_retries_total{topic=\"edits\"} 1",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }

    #[test]
    fn every_rust_compute_flag_has_four_outcome_series_at_zero() {
        let rendered = render_with(register);
        assert_eq!(ComputeFlag::ALL.len(), 14);
        for flag in ComputeFlag::ALL {
            for outcome in ["match", "mismatch", "failed", "unsupported"] {
                let line = format!(
                    "ranking_indexer_rust_compute_shadow_total{{flag=\"{}\",outcome=\"{outcome}\"}} 0",
                    flag.as_str()
                );
                assert!(rendered.contains(&line), "missing `{line}` in:\n{rendered}");
            }
        }
    }

    #[test]
    fn a_shadow_outcome_moves_only_its_own_series() {
        let rendered = render_with(|| {
            register();
            rust_compute_shadow(ComputeFlag::TopicSuggestions, ShadowOutcome::Match);
            rust_compute_shadow(ComputeFlag::TopicSuggestions, ShadowOutcome::Match);
            rust_compute_shadow(ComputeFlag::TopicSuggestions, ShadowOutcome::Mismatch);
            rust_compute_shadow(ComputeFlag::ClaimOverlap, ShadowOutcome::Unsupported);
            rust_compute_shadow(ComputeFlag::Anchor, ShadowOutcome::Failed);
        });
        for line in [
            "ranking_indexer_rust_compute_shadow_total{flag=\"topic_suggestions\",outcome=\"match\"} 2",
            "ranking_indexer_rust_compute_shadow_total{flag=\"topic_suggestions\",outcome=\"mismatch\"} 1",
            "ranking_indexer_rust_compute_shadow_total{flag=\"claim_overlap\",outcome=\"unsupported\"} 1",
            "ranking_indexer_rust_compute_shadow_total{flag=\"anchor\",outcome=\"failed\"} 1",
            "ranking_indexer_rust_compute_shadow_total{flag=\"anchor\",outcome=\"match\"} 0",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }

    #[test]
    fn mismatch_counts_classify_as_match_or_mismatch() {
        assert_eq!(ShadowOutcome::from_mismatches(0), ShadowOutcome::Match);
        assert_eq!(ShadowOutcome::from_mismatches(3), ShadowOutcome::Mismatch);
        assert_eq!(ShadowOutcome::from_agreement(true), ShadowOutcome::Match);
        assert_eq!(
            ShadowOutcome::from_agreement(false),
            ShadowOutcome::Mismatch
        );
    }

    /// Every `*_RUST_COMPUTE` env var read anywhere under `src/` must be a [`ComputeFlag`], so a
    /// new twin cannot ship without a counter.
    #[test]
    fn every_rust_compute_env_var_in_the_source_has_a_flag() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let known: Vec<&str> = ComputeFlag::ALL.iter().map(|f| f.env_var()).collect();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            for line in text.lines().filter(|l| l.contains("env::var(\"")) {
                let Some(start) = line.find("var(\"") else {
                    continue;
                };
                let rest = &line[start + 5..];
                let name = &rest[..rest.find('"').unwrap()];
                if name.ends_with("_RUST_COMPUTE") {
                    assert!(known.contains(&name), "{name} in {f:?} has no ComputeFlag");
                }
            }
        }
    }
}
