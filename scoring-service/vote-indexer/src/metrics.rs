//! Prometheus metrics for vote-indexer.
//!
//! The recorder and the `/metrics` listener come from `hermes_instrumentation::metrics`,
//! installed in `main`. Every counter is registered at zero by [`register`] so its series
//! exists from startup: a counter that first appears at 1 has no earlier sample, so
//! `increase()` cannot see its first event, and for [`VOTES_DROPPED`] that first event is
//! the one the alert exists to catch.
//!
//! There is deliberately no position gauge. `curation.votes` only carries a message when
//! someone votes, so any "last processed" value would stall through quiet stretches.
//! Per-partition lag is already watched by kafka-exporter.

pub const VOTES_PROCESSED: &str = "vote_indexer_votes_processed_total";
pub const VOTES_DROPPED: &str = "vote_indexer_votes_dropped_total";
pub const MESSAGES_REJECTED: &str = "vote_indexer_messages_rejected_total";
pub const RANKING_REFRESH_FAILURES: &str = "vote_indexer_ranking_refresh_failures_total";

/// Why a single message was committed past without being indexed.
#[derive(Debug, Clone, Copy)]
pub enum RejectReason {
    /// The payload is not a `HermesVoteCast`.
    Undecodable,
    /// It decoded, but its fields do not make a vote (bad object type, direction, id).
    Invalid,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            RejectReason::Undecodable => "undecodable",
            RejectReason::Invalid => "invalid",
        }
    }
}

/// Describe every counter and register it at zero.
pub fn register() {
    metrics::describe_counter!(
        VOTES_PROCESSED,
        "Votes written to Postgres in a committed batch transaction"
    );
    metrics::describe_counter!(
        VOTES_DROPPED,
        "Votes in a batch whose transaction failed. The batch's offsets are not committed, \
         but the next successful batch commits past them, so these votes are lost until \
         the topic is replayed"
    );
    metrics::describe_counter!(
        MESSAGES_REJECTED,
        "Kafka messages committed past because they could not be turned into a vote"
    );
    metrics::describe_counter!(
        RANKING_REFRESH_FAILURES,
        "Batches whose votes committed but whose feed ranking-score refresh failed; the \
         scores catch up on the next vote or a backfill"
    );

    metrics::counter!(VOTES_PROCESSED).absolute(0);
    metrics::counter!(VOTES_DROPPED).absolute(0);
    for reason in [RejectReason::Undecodable, RejectReason::Invalid] {
        metrics::counter!(MESSAGES_REJECTED, "reason" => reason.as_str()).absolute(0);
    }
    metrics::counter!(RANKING_REFRESH_FAILURES).absolute(0);
}

pub fn votes_processed(count: u64) {
    metrics::counter!(VOTES_PROCESSED).increment(count);
}

pub fn votes_dropped(count: u64) {
    metrics::counter!(VOTES_DROPPED).increment(count);
}

pub fn message_rejected(reason: RejectReason) {
    metrics::counter!(MESSAGES_REJECTED, "reason" => reason.as_str()).increment(1);
}

pub fn ranking_refresh_failed() {
    metrics::counter!(RANKING_REFRESH_FAILURES).increment(1);
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
        for line in [
            "vote_indexer_votes_processed_total 0",
            "vote_indexer_votes_dropped_total 0",
            "vote_indexer_messages_rejected_total{reason=\"undecodable\"} 0",
            "vote_indexer_messages_rejected_total{reason=\"invalid\"} 0",
            "vote_indexer_ranking_refresh_failures_total 0",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }

    #[test]
    fn counters_accumulate() {
        let rendered = render_with(|| {
            register();
            votes_processed(100);
            votes_processed(7);
            votes_dropped(12);
            message_rejected(RejectReason::Invalid);
            message_rejected(RejectReason::Invalid);
            message_rejected(RejectReason::Undecodable);
            ranking_refresh_failed();
        });
        for line in [
            "vote_indexer_votes_processed_total 107",
            "vote_indexer_votes_dropped_total 12",
            "vote_indexer_messages_rejected_total{reason=\"invalid\"} 2",
            "vote_indexer_messages_rejected_total{reason=\"undecodable\"} 1",
            "vote_indexer_ranking_refresh_failures_total 1",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }
}
