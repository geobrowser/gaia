//! Notification indexer entry point.
//!
//! Three concurrent tasks:
//! 1. Governance consumer — subscribes to `space.governance`, processes all governance
//!    events (PROPOSAL_CREATED, PROPOSAL_UPDATED, PROPOSAL_VOTED,
//!    PROPOSAL_EXECUTED, PROPOSAL_SETTINGS_UPDATED) and writes to the notification outbox.
//! 2. Knowledge edits consumer — subscribes to `knowledge.edits`, decodes GRC-20
//!    payloads, detects bounty-related CreateRelation operations (interest, allocated,
//!    payout) and writes to the notification outbox.
//! 3. Rejection poller — every 60s, finds proposals that expired without execution
//!    and writes rejection notifications.

use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use hermes_instrumentation::{debug, error, info, info_span, warn};
use rdkafka::message::Message;
use rdkafka::Timestamp;

use notification_indexer::consumer::{
    get_event_type, parse_hermes_edit, parse_proposal_created, parse_proposal_executed,
    parse_proposal_settings_updated, parse_proposal_updated, parse_proposal_voted, KafkaConsumer,
    KnowledgeEditsConsumer,
};
use notification_indexer::consumer_lag::LagMonitor;
use notification_indexer::error::IndexerError;
use notification_indexer::models::{
    build_rejection_event, build_vote_threshold_event, extract_bounty_created,
    extract_bounty_relations, extract_proposal_comments, handle_bounty_allocated,
    handle_bounty_created, handle_bounty_interest, handle_bounty_payout, handle_comment,
    handle_proposal_comment, handle_proposal_created, handle_proposal_executed,
    handle_proposal_settings_updated, handle_proposal_updated, handle_proposal_voted,
    merge_recipients, BountyConfig, CommentThreadInfo, NotificationEventType, TargetedRecipients,
};
use notification_indexer::storage::Storage;

fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();

    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    info!("Starting notification-indexer");

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| IndexerError::Config(format!("Failed to build tokio runtime: {}", e)))?
        .block_on(async_main())
}

/// Build telemetry configuration from environment variables.
fn build_telemetry_config() -> hermes_instrumentation::Config {
    use hermes_instrumentation::{Backend, Config};

    let backend = match env::var("SENTRY_DSN") {
        Ok(dsn) => {
            let traces_sample_rate = env::var("SENTRY_TRACES_SAMPLE_RATE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0);
            let send_default_pii = env::var("SENTRY_SEND_DEFAULT_PII")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);
            let environment = env::var("SENTRY_ENVIRONMENT").ok();
            let release = env::var("SENTRY_RELEASE").ok();
            let debug = env::var("SENTRY_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);

            println!(
                "Telemetry: Sentry (env: {}, release: {}, debug: {})",
                environment.as_deref().unwrap_or("none"),
                release.as_deref().unwrap_or("none"),
                if debug { "yes" } else { "no" }
            );

            Backend::Sentry {
                dsn,
                traces_sample_rate,
                send_default_pii,
                environment,
                release,
                debug,
                axiom: hermes_instrumentation::AxiomConfig::from_env(),
            }
        }
        _ => {
            println!("Telemetry: Console (set SENTRY_DSN to enable Sentry)");
            Backend::Console
        }
    };

    Config::new("notification-indexer", backend)
}

async fn async_main() -> Result<(), IndexerError> {
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let kafka_broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let kafka_group_id_governance = env::var("KAFKA_GROUP_ID_GOVERNANCE")
        .unwrap_or_else(|_| "notification-indexer-governance".to_string());
    let kafka_group_id_ke = env::var("KAFKA_GROUP_ID_KNOWLEDGE_EDITS")
        .unwrap_or_else(|_| "notification-indexer-knowledge-edits".to_string());

    let rejection_poll_interval_secs: u64 = env::var("REJECTION_POLL_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    let health_port: u16 = env::var("HEALTH_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    let block_delay: u64 = env::var("BLOCK_DELAY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);

    let block_delay_timeout_secs: u64 = env::var("BLOCK_DELAY_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);

    // Minimum event age: skip Kafka messages older than this many seconds.
    // Default: 86400 (1 day). Set to 0 to process all historical events.
    let min_age_secs: u64 = env::var("NOTIFICATION_MIN_AGE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(259200); // 3 days

    if min_age_secs > 0 {
        info!(
            min_age_secs = min_age_secs,
            "Skipping events older than {}s", min_age_secs
        );
    }

    // Entity-vote-threshold notifications: notify an entity's creator when its
    // upvotes reach this value. Set to 0 to disable the vote poller entirely.
    let vote_threshold: i64 = env::var("VOTE_NOTIFICATION_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let vote_poll_interval_secs: u64 = env::var("VOTE_POLL_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    // Overlap window (seconds) the vote poller backs its scan up by each tick.
    // votes_count.updated_at is a wall-clock stamp, but rows can still commit
    // slightly behind the cursor under concurrent writers; re-scanning a short
    // trailing window catches them (already_notified + idempotency dedupe the
    // re-scan). Set to 0 to disable.
    let vote_poll_overlap_secs: i64 = env::var("VOTE_POLL_OVERLAP_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    // Cold-start lookback (seconds): on the very first poll (no persisted cursor),
    // only consider entities whose votes changed within this trailing window
    // instead of replaying all history. Prevents a deploy-time backfill storm that
    // would notify every already-over-threshold entity. Default 1 day.
    let vote_cold_start_lookback_secs: i64 = env::var("VOTE_COLD_START_LOOKBACK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(86_400);

    // Initialize storage
    let storage = Storage::connect(&database_url).await?;
    info!("Connected to database");

    // Start health check server
    let _health_handle =
        notification_indexer::health::start_health_server(storage.pool().clone(), health_port);

    // Initialize Kafka consumer
    let consumer = KafkaConsumer::new(&kafka_broker, &kafka_group_id_governance)?;
    consumer.subscribe()?;

    // Start background lag monitor (dedicated thread, never blocks async runtime)
    let prefix = hermes_kafka::get_topic_prefix();
    let governance_topic = format!("{}space.governance", prefix);
    let lag_poll_secs: u64 = env::var("HEARTBEAT_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let lag_monitor = match LagMonitor::start(
        &kafka_broker,
        &kafka_group_id_governance,
        governance_topic,
        lag_poll_secs,
    ) {
        Ok(m) => {
            info!("Lag monitor started");
            Some(m)
        }
        Err(e) => {
            warn!(error = %e, "Failed to start lag monitor — heartbeat will report lag=-1");
            None
        }
    };

    let bounty_config = BountyConfig::new();
    info!(
        interest = %bounty_config.interest_type_id,
        allocated = %bounty_config.allocated_type_id,
        payout = %bounty_config.payout_type_id,
        "Bounty relation type config"
    );

    // Set up shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
    let mut shutdown_rx2 = shutdown_tx.subscribe();
    let mut shutdown_rx3 = shutdown_tx.subscribe();
    let mut shutdown_rx4 = shutdown_tx.subscribe();

    let _signal_handle = tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        shutdown_tx.send(()).ok();
    });

    // Spawn rejection poller task
    let poller_storage = Storage::new(storage.pool().clone());
    let poller_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
            rejection_poll_interval_secs,
        ));

        loop {
            tokio::select! {
                _ = shutdown_rx2.recv() => {
                    info!("Rejection poller shutting down");
                    break;
                }
                _ = interval.tick() => {
                    const BATCH_LIMIT: i64 = 1000;

                    // Process in batches to avoid unbounded memory usage
                    loop {
                        match poller_storage.find_expired_proposals(BATCH_LIMIT).await {
                            Ok(expired) => {
                                let batch_len = expired.len();
                                if !expired.is_empty() {
                                    info!(count = batch_len, "Found expired proposals");
                                }
                                for proposal in &expired {
                                    let mut event = build_rejection_event(
                                        proposal.id,
                                        proposal.space_id,
                                        proposal.proposed_by,
                                        proposal.end_time,
                                    );
                                    // Enrich rejection with names (proposal is guaranteed to exist)
                                    enrich_payload(&poller_storage, &mut event, proposal.space_id).await;

                                    let editors = match poller_storage.find_editors_for_space(proposal.space_id).await {
                                        Ok(eds) => eds,
                                        Err(e) => {
                                            error!(error = %e, space_id = %proposal.space_id, "Failed to look up editors for rejection, will retry next poll");
                                            continue;
                                        }
                                    };
                                    // Notify the proposer that their proposal was rejected, even if
                                    // they are not an editor. `proposed_by` is the proposer's
                                    // personal-space UUID, usable directly as a recipient.
                                    let recipients = merge_recipients(editors, vec![proposal.proposed_by]);
                                    if recipients.is_empty() {
                                        continue;
                                    }
                                    match poller_storage.insert_notifications_for_users(&event, &recipients).await {
                                        Ok(count) if count > 0 => {
                                            info!(
                                                proposal_id = %proposal.id,
                                                recipients = recipients.len(),
                                                inserted = count,
                                                "Inserted rejection notifications"
                                            );
                                        }
                                        Ok(_) => {
                                            // All duplicates, already notified
                                        }
                                        Err(e) => {
                                            error!(
                                                error = %e,
                                                proposal_id = %proposal.id,
                                                "Failed to insert rejection notifications"
                                            );
                                        }
                                    }
                                }
                                // If we got fewer than the limit, there are no more to process
                                if (batch_len as i64) < BATCH_LIMIT {
                                    break;
                                }
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to query expired proposals");
                                break;
                            }
                        }
                    }
                }
            }
        }
    });

    // Spawn entity-vote-threshold poller task.
    //
    // Polls votes_count for entities whose upvotes reached VOTE_NOTIFICATION_THRESHOLD
    // and notifies each entity's creator. Uses a persisted keyset cursor on
    // (updated_at, id) so each poll only scans rows whose counts changed since the
    // last poll. Disabled when the threshold is <= 0.
    let vote_poller_handle: Option<tokio::task::JoinHandle<()>> = if vote_threshold > 0 {
        let vote_storage = Storage::new(storage.pool().clone());
        info!(
            threshold = vote_threshold,
            interval_secs = vote_poll_interval_secs,
            "Entity-vote-threshold poller enabled"
        );
        Some(tokio::spawn(async move {
            const CURSOR_NAME: &str = "entity_votes_threshold";
            const BATCH_LIMIT: i64 = 1000;
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(vote_poll_interval_secs));

            loop {
                tokio::select! {
                    _ = shutdown_rx4.recv() => {
                        info!("Vote poller shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        // Resume from the persisted cursor. On the very first run
                        // (no cursor) start a bounded lookback behind now instead of
                        // epoch, so a fresh deploy doesn't replay every historical
                        // over-threshold entity (backfill storm).
                        let (persisted_ts, persisted_id) = match vote_storage.get_poll_cursor(CURSOR_NAME).await {
                            Ok(Some(c)) => c,
                            Ok(None) => {
                                let now_secs = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .map(|d| d.as_secs() as i64)
                                    .unwrap_or(0);
                                let start = sqlx::types::chrono::DateTime::<sqlx::types::chrono::Utc>::from_timestamp(
                                    now_secs - vote_cold_start_lookback_secs,
                                    0,
                                )
                                .unwrap_or_default();
                                info!(
                                    lookback_secs = vote_cold_start_lookback_secs,
                                    "Vote poller cold start — scanning from the lookback window"
                                );
                                (start, 0i64)
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to read vote poll cursor; skipping tick");
                                continue;
                            }
                        };

                        // Back the scan start up by the overlap window so rows that
                        // committed slightly behind the cursor are re-examined. The
                        // persisted high-water (hw_*) never moves backward, so the
                        // overlap only re-scans a bounded trailing window each tick;
                        // already_notified + idempotency make the re-scan a no-op.
                        let mut cur_ts = sqlx::types::chrono::DateTime::<sqlx::types::chrono::Utc>::from_timestamp(
                            persisted_ts.timestamp() - vote_poll_overlap_secs,
                            0,
                        )
                        .unwrap_or(persisted_ts);
                        let mut cur_id = 0i64;
                        let mut hw_ts = persisted_ts;
                        let mut hw_id = persisted_id;

                        loop {
                            let rows = match vote_storage
                                .find_entity_vote_counts_since(cur_ts, cur_id, vote_threshold, BATCH_LIMIT)
                                .await
                            {
                                Ok(r) => r,
                                Err(e) => {
                                    error!(error = %e, "Failed to query entity vote counts");
                                    break;
                                }
                            };
                            let batch_len = rows.len();
                            if batch_len == 0 {
                                break;
                            }

                            let mut had_error = false;
                            for row in &rows {
                                // Skip entities already notified at this threshold
                                // (the anti-check) — but still advance the cursor.
                                if row.upvotes >= vote_threshold && !row.already_notified {
                                    match vote_storage.find_entity_home_space(row.entity_id).await {
                                        Ok(Some(creator)) => {
                                            let mut event = build_vote_threshold_event(
                                                row.entity_id,
                                                row.space_id,
                                                row.upvotes,
                                                row.downvotes,
                                                vote_threshold,
                                            );
                                            enrich_payload(&vote_storage, &mut event, row.space_id).await;
                                            match vote_storage.insert_notification_for_user(&event, creator).await {
                                                Ok(c) if c > 0 => {
                                                    info!(
                                                        entity_id = %row.entity_id,
                                                        upvotes = row.upvotes,
                                                        threshold = vote_threshold,
                                                        "Inserted entity vote-threshold notification"
                                                    );
                                                }
                                                Ok(_) => { /* already notified at this threshold */ }
                                                Err(e) => {
                                                    error!(error = %e, entity_id = %row.entity_id, "Failed to insert vote-threshold notification");
                                                    had_error = true;
                                                    break;
                                                }
                                            }
                                        }
                                        Ok(None) => {
                                            debug!(entity_id = %row.entity_id, "No creator/home space; skipping vote-threshold notification");
                                        }
                                        Err(e) => {
                                            error!(error = %e, entity_id = %row.entity_id, "DB error resolving entity creator");
                                            had_error = true;
                                            break;
                                        }
                                    }
                                }
                                // Page forward over every scanned row...
                                cur_ts = row.updated_at;
                                cur_id = row.cursor_id;
                                // ...and track the high-water (never below the previous cursor).
                                if (row.updated_at, row.cursor_id) > (hw_ts, hw_id) {
                                    hw_ts = row.updated_at;
                                    hw_id = row.cursor_id;
                                }
                            }

                            if had_error {
                                // Don't persist — retry this batch on the next tick.
                                break;
                            }
                            // Persist the high-water after the batch succeeds, so a
                            // mid-batch DB error retries (idempotency makes re-inserts
                            // a no-op).
                            if let Err(e) = vote_storage.set_poll_cursor(CURSOR_NAME, hw_ts, hw_id).await {
                                error!(error = %e, "Failed to persist vote poll cursor");
                                break;
                            }

                            if (batch_len as i64) < BATCH_LIMIT {
                                break;
                            }
                        }
                    }
                }
            }
        }))
    } else {
        info!("Entity-vote-threshold poller disabled (VOTE_NOTIFICATION_THRESHOLD <= 0)");
        None
    };

    // Spawn knowledge edits consumer task
    let ke_handle: tokio::task::JoinHandle<()> = {
        let ke_kafka_broker = kafka_broker.clone();
        let ke_kafka_group_id = kafka_group_id_ke.clone();
        let ke_bd = block_delay;
        let ke_bd_timeout = block_delay_timeout_secs;
        let ke_min_age = min_age_secs;
        let ke_stor = Storage::new(storage.pool().clone());
        let ke_cfg = bounty_config;
        let ke_heartbeat_secs: u64 = env::var("HEARTBEAT_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);

        tokio::spawn(async move {
            // Create consumer inside the task so it's fully owned
            let kec = match KnowledgeEditsConsumer::new(&ke_kafka_broker, &ke_kafka_group_id) {
                Ok(c) => c,
                Err(e) => {
                    error!(error = %e, "Failed to create knowledge edits consumer");
                    return;
                }
            };
            if let Err(e) = kec.subscribe() {
                error!(error = %e, "Failed to subscribe to knowledge.edits");
                return;
            }

            let mut ke_stream = kec.stream();
            let mut ke_processed: u64 = 0;
            let mut ke_errors: u64 = 0;
            let mut ke_skipped: u64 = 0;

            let mut ke_heartbeat =
                tokio::time::interval(tokio::time::Duration::from_secs(ke_heartbeat_secs));
            ke_heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            info!("Knowledge edits consumer loop started");

            loop {
                tokio::select! {
                    _ = shutdown_rx3.recv() => {
                        info!("Knowledge edits consumer shutting down");
                        break;
                    }
                    _ = ke_heartbeat.tick() => {
                        info!(
                            processed = ke_processed,
                            errors = ke_errors,
                            skipped = ke_skipped,
                            "Knowledge edits heartbeat"
                        );
                    }
                    message = ke_stream.next() => {
                        match message {
                            Some(Ok(msg)) => {
                                let topic = msg.topic().to_string();
                                let partition = msg.partition();
                                let offset = msg.offset();

                                // Skip old messages
                                if ke_min_age > 0 {
                                    let now_ms = SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .map(|d| d.as_millis() as i64)
                                        .unwrap_or(0);
                                    let too_old = match msg.timestamp() {
                                        Timestamp::CreateTime(ts) | Timestamp::LogAppendTime(ts) => {
                                            (now_ms - ts) > (ke_min_age as i64 * 1000)
                                        }
                                        Timestamp::NotAvailable => false,
                                    };
                                    if too_old {
                                        debug!(
                                            partition = partition,
                                            offset = offset,
                                            "Skipping old knowledge edit (older than {}s)",
                                            ke_min_age
                                        );
                                        if let Err(e) = kec.commit_message(&topic, partition, offset) {
                                            error!(error = %e, "Failed to commit knowledge edits offset");
                                        }
                                        continue;
                                    }
                                }

                                let Some(payload) = msg.payload() else {
                                    if let Err(e) = kec.commit_message(&topic, partition, offset) {
                                        error!(error = %e, "Failed to commit knowledge edits offset");
                                    }
                                    continue;
                                };

                                // Parse HermesEdit protobuf
                                let hermes_edit = match parse_hermes_edit(payload) {
                                    Ok(edit) => edit,
                                    Err(e) => {
                                        // Parse error — commit to avoid poison pill
                                        warn!(
                                            error = %e,
                                            partition = partition,
                                            offset = offset,
                                            "Failed to parse HermesEdit, committing to skip"
                                        );
                                        ke_errors += 1;
                                        if let Err(e) = kec.commit_message(&topic, partition, offset) {
                                            error!(error = %e, "Failed to commit knowledge edits offset");
                                        }
                                        continue;
                                    }
                                };

                                // Extract bounty relations from GRC-20 payload
                                let relations = match extract_bounty_relations(&hermes_edit, &ke_cfg) {
                                    Ok(rels) => rels,
                                    Err(e) => {
                                        // Decode error — commit to avoid poison pill
                                        warn!(
                                            error = %e,
                                            partition = partition,
                                            offset = offset,
                                            "Failed to extract bounty relations, committing to skip"
                                        );
                                        ke_errors += 1;
                                        if let Err(e) = kec.commit_message(&topic, partition, offset) {
                                            error!(error = %e, "Failed to commit knowledge edits offset");
                                        }
                                        continue;
                                    }
                                };

                                // Also detect newly-created bounties (Phase 3a) and comments on
                                // proposals (Phase 2a) in the same edit.
                                let bounties_created = match extract_bounty_created(&hermes_edit) {
                                    Ok(b) => b,
                                    Err(e) => {
                                        warn!(error = %e, partition = partition, offset = offset, "Failed to extract bounty-created, committing to skip");
                                        ke_errors += 1;
                                        if let Err(e) = kec.commit_message(&topic, partition, offset) {
                                            error!(error = %e, "Failed to commit knowledge edits offset");
                                        }
                                        continue;
                                    }
                                };
                                let comments = match extract_proposal_comments(&hermes_edit) {
                                    Ok(c) => c,
                                    Err(e) => {
                                        warn!(error = %e, partition = partition, offset = offset, "Failed to extract proposal comments, committing to skip");
                                        ke_errors += 1;
                                        if let Err(e) = kec.commit_message(&topic, partition, offset) {
                                            error!(error = %e, "Failed to commit knowledge edits offset");
                                        }
                                        continue;
                                    }
                                };

                                if relations.is_empty()
                                    && bounties_created.is_empty()
                                    && comments.is_empty()
                                {
                                    // Nothing relevant in this edit — commit and continue
                                    if let Err(e) = kec.commit_message(&topic, partition, offset) {
                                        error!(error = %e, "Failed to commit knowledge edits offset");
                                    }
                                    continue;
                                }

                                // Process all bounty relations in this edit.
                                // Returns Err on DB errors (don't commit — retry on restart).
                                // Returns Ok on success or non-retryable skips.
                                let process_result: Result<(), ()> = async {
                                    for (mut info, event_type) in relations {
                                        // Block delay: wait for kg-indexer catchup
                                        if ke_bd > 0 {
                                            wait_for_kg_catchup(
                                                &ke_stor,
                                                info.block_number,
                                                ke_bd,
                                                ke_bd_timeout,
                                            )
                                            .await;
                                        }

                                        match event_type {
                                            NotificationEventType::BountyInterest => {
                                                // Resolve bounty_space_id from DB
                                                match ke_stor.lookup_bounty_space(info.bounty_entity_id).await {
                                                    Ok(Some(space_id)) => {
                                                        info.bounty_space_id = space_id;
                                                    }
                                                    Ok(None) => {
                                                        warn!(
                                                            bounty_entity_id = %info.bounty_entity_id,
                                                            "Could not resolve bounty space, skipping interest notification"
                                                        );
                                                        ke_skipped += 1;
                                                        continue;
                                                    }
                                                    Err(e) => {
                                                        error!(error = %e, "DB error looking up bounty space, will retry");
                                                        ke_errors += 1;
                                                        return Err(());
                                                    }
                                                }

                                                let mut event = handle_bounty_interest(&info);
                                                enrich_payload(&ke_stor, &mut event, info.bounty_space_id).await;

                                                let editors = ke_stor.find_editors_for_space(info.bounty_space_id).await.map_err(|e| {
                                                    error!(error = %e, "DB error looking up editors for bounty interest, will retry");
                                                    ke_errors += 1;
                                                })?;

                                                if editors.is_empty() {
                                                    ke_processed += 1;
                                                    continue;
                                                }

                                                ke_stor.insert_notifications_for_users(&event, &editors).await.map(|count| {
                                                    if count > 0 {
                                                        info!(
                                                            event_type = "bounty_interest",
                                                            editors = editors.len(),
                                                            inserted = count,
                                                            "Inserted bounty interest notifications"
                                                        );
                                                    }
                                                    ke_processed += 1;
                                                }).map_err(|e| {
                                                    error!(error = %e, "DB error inserting bounty interest notifications, will retry");
                                                    ke_errors += 1;
                                                })?;
                                            }
                                            NotificationEventType::BountyAllocated
                                            | NotificationEventType::BountyPayout => {
                                                // Resolve curator_space_id if nil
                                                if info.curator_space_id.is_nil() {
                                                    match ke_stor.lookup_entity_space(info.curator_entity_id).await {
                                                        Ok(Some(space_id)) => {
                                                            info.curator_space_id = space_id;
                                                        }
                                                        Ok(None) => {
                                                            warn!(
                                                                bounty_entity_id = %info.bounty_entity_id,
                                                                curator_entity_id = %info.curator_entity_id,
                                                                event_type = %event_type.as_str(),
                                                                "Could not resolve curator space, skipping notification"
                                                            );
                                                            ke_skipped += 1;
                                                            continue;
                                                        }
                                                        Err(e) => {
                                                            error!(error = %e, "DB error looking up curator space, will retry");
                                                            ke_errors += 1;
                                                            return Err(());
                                                        }
                                                    }
                                                }

                                                let mut event = if event_type == NotificationEventType::BountyAllocated {
                                                    handle_bounty_allocated(&info)
                                                } else {
                                                    handle_bounty_payout(&info)
                                                };
                                                enrich_payload(&ke_stor, &mut event, info.bounty_space_id).await;

                                                ke_stor.insert_notification_for_user(&event, info.curator_space_id).await.map(|count| {
                                                    if count > 0 {
                                                        info!(
                                                            event_type = %event_type.as_str(),
                                                            curator_space_id = %info.curator_space_id,
                                                            inserted = count,
                                                            "Inserted bounty notification for curator"
                                                        );
                                                    }
                                                    ke_processed += 1;
                                                }).map_err(|e| {
                                                    error!(
                                                        error = %e,
                                                        event_type = %event_type.as_str(),
                                                        "DB error inserting bounty notification, will retry"
                                                    );
                                                    ke_errors += 1;
                                                })?;
                                            }
                                            _ => {
                                                continue;
                                            }
                                        }
                                    }

                                    // Phase 3a: newly-created bounties → notify the space's editors.
                                    for info in bounties_created {
                                        if ke_bd > 0 {
                                            wait_for_kg_catchup(&ke_stor, info.block_number, ke_bd, ke_bd_timeout).await;
                                        }
                                        let mut event = handle_bounty_created(&info);
                                        enrich_payload(&ke_stor, &mut event, info.space_id).await;
                                        let editors = ke_stor.find_editors_for_space(info.space_id).await.map_err(|e| {
                                            error!(error = %e, "DB error looking up editors for bounty_created, will retry");
                                            ke_errors += 1;
                                        })?;
                                        if editors.is_empty() {
                                            ke_processed += 1;
                                            continue;
                                        }
                                        ke_stor.insert_notifications_for_users(&event, &editors).await.map(|count| {
                                            if count > 0 {
                                                info!(
                                                    event_type = "bounty_created",
                                                    space_id = %info.space_id,
                                                    editors = editors.len(),
                                                    inserted = count,
                                                    "Inserted bounty_created notifications"
                                                );
                                            }
                                            ke_processed += 1;
                                        }).map_err(|e| {
                                            error!(error = %e, "DB error inserting bounty_created notifications, will retry");
                                            ke_errors += 1;
                                        })?;
                                    }

                                    // Comments. If the comment replies *directly* to a proposal,
                                    // notify the proposer gated on member/editor (Phase 2a). Otherwise
                                    // it's a general comment / reply in a thread: notify all thread
                                    // participants plus the thread root's creator (Phase 2b).
                                    // App servers handle per-user muting/snoozing.
                                    for mut info in comments {
                                        if ke_bd > 0 {
                                            wait_for_kg_catchup(&ke_stor, info.block_number, ke_bd, ke_bd_timeout).await;
                                        }
                                        let (proposer, proposal_space) = match ke_stor.find_proposal_proposer_and_space(info.proposal_id).await {
                                            Ok(Some(pair)) => pair,
                                            Ok(None) => {
                                                // Phase 2b: general comment thread.
                                                let root = ke_stor.resolve_thread_root(info.proposal_id).await.map_err(|e| {
                                                    error!(error = %e, "DB error resolving comment thread root, will retry");
                                                    ke_errors += 1;
                                                })?;
                                                let mut recipients = ke_stor.find_thread_participants(root).await.map_err(|e| {
                                                    error!(error = %e, "DB error resolving thread participants, will retry");
                                                    ke_errors += 1;
                                                })?;
                                                // Root creator: exact for proposals (proposer),
                                                // best-effort home space otherwise.
                                                let root_space = match ke_stor.find_proposal_proposer_and_space(root).await.map_err(|e| {
                                                    error!(error = %e, "DB error resolving thread root proposal, will retry");
                                                    ke_errors += 1;
                                                })? {
                                                    Some((root_proposer, space)) => {
                                                        recipients.push(root_proposer);
                                                        space
                                                    }
                                                    None => match ke_stor.find_entity_home_space(root).await.map_err(|e| {
                                                        error!(error = %e, "DB error resolving thread root home space, will retry");
                                                        ke_errors += 1;
                                                    })? {
                                                        Some(home) => {
                                                            recipients.push(home);
                                                            home
                                                        }
                                                        None => info.commenter_space_id,
                                                    },
                                                };
                                                // Don't notify the comment's own author.
                                                let recipients: Vec<uuid::Uuid> = merge_recipients(recipients, vec![])
                                                    .into_iter()
                                                    .filter(|r| *r != info.commenter_space_id)
                                                    .collect();
                                                if recipients.is_empty() {
                                                    ke_processed += 1;
                                                    continue;
                                                }
                                                let cinfo = CommentThreadInfo {
                                                    comment_entity_id: info.comment_entity_id,
                                                    parent_id: info.proposal_id,
                                                    root_id: root,
                                                    commenter_space_id: info.commenter_space_id,
                                                    root_space_id: root_space,
                                                    block_number: info.block_number,
                                                    sequence: info.sequence,
                                                    timestamp: info.timestamp,
                                                };
                                                let mut event = handle_comment(&cinfo);
                                                enrich_payload(&ke_stor, &mut event, root_space).await;
                                                ke_stor.insert_notifications_for_users(&event, &recipients).await.map(|count| {
                                                    if count > 0 {
                                                        info!(
                                                            event_type = "comment",
                                                            root_id = %root,
                                                            recipients = recipients.len(),
                                                            inserted = count,
                                                            "Inserted comment thread notifications"
                                                        );
                                                    }
                                                    ke_processed += 1;
                                                }).map_err(|e| {
                                                    error!(error = %e, "DB error inserting comment notifications, will retry");
                                                    ke_errors += 1;
                                                })?;
                                                continue;
                                            }
                                            Err(e) => {
                                                error!(error = %e, "DB error resolving proposal for comment, will retry");
                                                ke_errors += 1;
                                                return Err(());
                                            }
                                        };
                                        let allowed = ke_stor.is_member_or_editor(proposal_space, info.commenter_space_id).await.map_err(|e| {
                                            error!(error = %e, "DB error checking commenter membership, will retry");
                                            ke_errors += 1;
                                        })?;
                                        if !allowed {
                                            debug!(
                                                proposal_id = %info.proposal_id,
                                                commenter = %info.commenter_space_id,
                                                "Comment author is not a member/editor of the proposal space, skipping"
                                            );
                                            ke_skipped += 1;
                                            continue;
                                        }
                                        info.proposal_space_id = proposal_space;
                                        let mut event = handle_proposal_comment(&info);
                                        enrich_payload(&ke_stor, &mut event, proposal_space).await;
                                        ke_stor.insert_notification_for_user(&event, proposer).await.map(|count| {
                                            if count > 0 {
                                                info!(
                                                    event_type = "proposal_comment",
                                                    proposal_id = %info.proposal_id,
                                                    proposer = %proposer,
                                                    inserted = count,
                                                    "Inserted proposal_comment notification"
                                                );
                                            }
                                            ke_processed += 1;
                                        }).map_err(|e| {
                                            error!(error = %e, "DB error inserting proposal_comment notification, will retry");
                                            ke_errors += 1;
                                        })?;
                                    }

                                    Ok(())
                                }.await;

                                // Only commit offset if all relations processed successfully.
                                // On DB error (Err): don't commit — retry on restart.
                                if process_result.is_ok() {
                                    if let Err(e) = kec.commit_message(&topic, partition, offset) {
                                        error!(error = %e, "Failed to commit knowledge edits offset");
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                error!(error = %e, "Knowledge edits Kafka error");
                            }
                            None => {
                                info!("Knowledge edits stream ended");
                                break;
                            }
                        }
                    }
                }
            }

            kec.flush_commits();
            info!(
                processed = ke_processed,
                errors = ke_errors,
                "Knowledge edits consumer stopped"
            );
        })
    };

    // Main Kafka consumer loop
    let mut stream = consumer.stream();
    let mut processed_count: u64 = 0;
    let mut error_count: u64 = 0;

    // Heartbeat: log stats every 60 seconds so operators can see the service is alive
    let heartbeat_interval_secs: u64 = env::var("HEARTBEAT_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let mut heartbeat_timer =
        tokio::time::interval(tokio::time::Duration::from_secs(heartbeat_interval_secs));
    heartbeat_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    info!("Starting message processing loop");

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Kafka consumer shutting down");
                break;
            }

            _ = heartbeat_timer.tick() => {
                // Approximate outbox size from planner statistics (reltuples) —
                // an O(1) catalog lookup, rather than a count(*) seq-scan over
                // the unbounded, ever-growing outbox on every heartbeat. The
                // estimate is refreshed by autovacuum/ANALYZE; -1 before the
                // first analyze, which is fine for a log gauge.
                let outbox_total_approx = sqlx::query_scalar::<_, i64>(
                    "SELECT reltuples::bigint FROM pg_class WHERE oid = 'notification_outbox'::regclass"
                )
                .fetch_one(storage.pool())
                .await
                .unwrap_or(-1);

                let pool_size = storage.pool().size();
                let pool_idle = storage.pool().num_idle();
                let consumer_lag = lag_monitor.as_ref().map_or(-1, |m| m.get());

                info!(
                    processed = processed_count,
                    errors = error_count,
                    outbox_total_approx = outbox_total_approx,
                    consumer_lag = consumer_lag,
                    pool_size = pool_size,
                    pool_idle = pool_idle,
                    "Heartbeat"
                );
            }

            message = stream.next() => {
                match message {
                    Some(Ok(msg)) => {
                        let topic = msg.topic().to_string();
                        let partition = msg.partition();
                        let offset = msg.offset();

                        // Skip old messages on startup replay
                        if min_age_secs > 0 {
                            let now_ms = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map(|d| d.as_millis() as i64)
                                .unwrap_or(0);
                            let too_old = match msg.timestamp() {
                                Timestamp::CreateTime(ts) | Timestamp::LogAppendTime(ts) => {
                                    (now_ms - ts) > (min_age_secs as i64 * 1000)
                                }
                                Timestamp::NotAvailable => false,
                            };
                            if too_old {
                                debug!(
                                    partition = partition,
                                    offset = offset,
                                    "Skipping old message (older than {}s)",
                                    min_age_secs
                                );
                                if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                    error!(error = %e, "Failed to commit offset");
                                }
                                continue;
                            }
                        }

                        let event_type = get_event_type(msg.headers());
                        let _span = info_span!(
                            "notification_indexer.process_event",
                            event_type = event_type.as_deref().unwrap_or("unknown"),
                            partition = partition,
                            offset = offset,
                        )
                        .entered();
                        let mut should_commit = false;

                        if let Some(payload) = msg.payload() {
                            let result = match event_type.as_deref() {
                                Some("PROPOSAL_CREATED") => {
                                    parse_proposal_created(payload).and_then(|proto| {
                                        handle_proposal_created(&proto)
                                            .map_err(IndexerError::from)
                                    })
                                }
                                Some("PROPOSAL_UPDATED") => {
                                    parse_proposal_updated(payload).and_then(|proto| {
                                        handle_proposal_updated(&proto)
                                            .map_err(IndexerError::from)
                                    })
                                }
                                Some("PROPOSAL_VOTED") => {
                                    parse_proposal_voted(payload).and_then(|proto| {
                                        handle_proposal_voted(&proto)
                                            .map_err(IndexerError::from)
                                    })
                                }
                                Some("PROPOSAL_EXECUTED") => {
                                    parse_proposal_executed(payload).and_then(|proto| {
                                        handle_proposal_executed(&proto)
                                            .map_err(IndexerError::from)
                                    })
                                }
                                Some("PROPOSAL_SETTINGS_UPDATED") => {
                                    parse_proposal_settings_updated(payload).and_then(|proto| {
                                        handle_proposal_settings_updated(&proto)
                                            .map_err(IndexerError::from)
                                    })
                                }
                                _ => {
                                    // Ignore unknown event types
                                    if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                        error!(error = %e, "Failed to commit offset");
                                    }
                                    continue;
                                }
                            };

                            match result {
                                Ok(mut event) => {
                                    // Check block timestamp for age (more reliable than Kafka timestamp
                                    // which can be recent even for old blocks after topic reprocessing)
                                    if let Some(ts) = event.payload.timestamp {
                                        if is_block_too_old(ts, min_age_secs) {
                                            debug!(
                                                event_type = %event.payload.event_type,
                                                block_timestamp = ts,
                                                "Skipping old governance event (block older than {}s)",
                                                min_age_secs
                                            );
                                            if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                                error!(error = %e, "Failed to commit offset");
                                            }
                                            continue;
                                        }
                                    }

                                    // Block delay: wait for kg-indexer to catch up so
                                    // names/metadata are populated before we send
                                    if block_delay > 0 {
                                        if let Some(msg_block) = event.payload.block_number {
                                            wait_for_kg_catchup(
                                                &storage,
                                                msg_block,
                                                block_delay,
                                                block_delay_timeout_secs,
                                            )
                                            .await;
                                        }
                                    }

                                    // Resolve editors for the space
                                    let space_id = match uuid::Uuid::parse_str(&event.payload.space_id) {
                                        Ok(sid) => sid,
                                        Err(e) => {
                                            // Malformed space_id — cannot reprocess, commit to avoid poison pill
                                            error!(error = %e, "Invalid space_id in event");
                                            error_count += 1;
                                            if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                                error!(error = %e, "Failed to commit offset");
                                            }
                                            continue;
                                        }
                                    };

                                    // Enrich payload with human-readable names (best-effort)
                                    enrich_payload(&storage, &mut event, space_id).await;

                                    let editors = match storage.find_editors_for_space(space_id).await {
                                        Ok(eds) => eds,
                                        Err(e) => {
                                            // DB error — don't commit offset so we retry on restart
                                            error!(
                                                error = %e,
                                                space_id = %space_id,
                                                event_type = %event.payload.event_type,
                                                "Failed to look up editors, will retry"
                                            );
                                            error_count += 1;
                                            continue;
                                        }
                                    };

                                    // Targeted recipients beyond the space's editors (see
                                    // NotificationEventType::targeted_recipients). Filtering to a
                                    // specific audience is done app-side, so we deliver the relevant
                                    // superset. Best-effort: a lookup miss only drops the targeted
                                    // extra — editors always get the base event, and the proposer
                                    // is usually also an editor.
                                    let extra: Vec<uuid::Uuid> = match event.governance_proposal_id() {
                                        Some(pid) => match event.event_type.targeted_recipients() {
                                            TargetedRecipients::Proposer => {
                                                match storage.find_proposer_for_proposal(pid).await {
                                                    Ok(Some(proposer)) => vec![proposer],
                                                    Ok(None) => vec![],
                                                    Err(e) => {
                                                        warn!(error = %e, proposal_id = %pid, "Failed to resolve proposer; notifying editors only");
                                                        vec![]
                                                    }
                                                }
                                            }
                                            TargetedRecipients::Voters => {
                                                match storage.find_voters_for_proposal(pid).await {
                                                    Ok(voters) => voters,
                                                    Err(e) => {
                                                        warn!(error = %e, proposal_id = %pid, "Failed to resolve voters; notifying editors only");
                                                        vec![]
                                                    }
                                                }
                                            }
                                            TargetedRecipients::None => vec![],
                                        },
                                        None => vec![],
                                    };
                                    let recipients = merge_recipients(editors, extra);

                                    if recipients.is_empty() {
                                        // Genuinely no recipients — this is normal, not an error
                                        processed_count += 1;
                                        should_commit = true;
                                    } else {
                                        match storage.insert_notifications_for_users(&event, &recipients).await {
                                            Ok(count) => {
                                                if count > 0 {
                                                    info!(
                                                        event_type = %event.payload.event_type,
                                                        category = %event.payload.category,
                                                        recipients = recipients.len(),
                                                        inserted = count,
                                                        "Inserted per-user notifications"
                                                    );
                                                }
                                                processed_count += 1;
                                                should_commit = true;
                                            }
                                            Err(e) => {
                                                // DB error — don't commit offset so we retry on restart
                                                error!(
                                                    error = %e,
                                                    event_type = %event.payload.event_type,
                                                    "Failed to insert notifications, will retry"
                                                );
                                                error_count += 1;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    // Handler/parse error — cannot reprocess, commit to avoid poison pill
                                    warn!(
                                        error = %e,
                                        partition = partition,
                                        offset = offset,
                                        "Failed to handle governance message"
                                    );
                                    error_count += 1;
                                    should_commit = true;
                                }
                            }
                        } else {
                            // No payload — nothing to process, safe to commit
                            should_commit = true;
                        }

                        // Only commit offset when processing succeeded or the message
                        // is permanently unprocessable. DB errors leave the offset
                        // uncommitted so the message is retried on restart.
                        if should_commit {
                            if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                error!(error = %e, "Failed to commit offset");
                            }
                        }
                    }
                    Some(Err(e)) => {
                        error!(error = %e, "Kafka error");
                    }
                    None => {
                        info!("Stream ended");
                        break;
                    }
                }
            }
        }
    }

    // Flush any pending async offset commits before shutting down
    consumer.flush_commits();

    // Wait for poller to finish gracefully (it should exit via shutdown_rx2)
    match tokio::time::timeout(tokio::time::Duration::from_secs(5), poller_handle).await {
        Ok(Ok(())) => info!("Rejection poller stopped"),
        Ok(Err(e)) => warn!(error = %e, "Rejection poller task failed"),
        Err(_) => {
            warn!("Rejection poller did not stop within 5s, aborting");
        }
    }

    // Wait for knowledge edits consumer to finish
    match tokio::time::timeout(tokio::time::Duration::from_secs(5), ke_handle).await {
        Ok(Ok(())) => info!("Knowledge edits consumer stopped"),
        Ok(Err(e)) => warn!(error = %e, "Knowledge edits consumer task failed"),
        Err(_) => {
            warn!("Knowledge edits consumer did not stop within 5s, aborting");
        }
    }

    // Wait for the vote poller to finish (if enabled)
    if let Some(handle) = vote_poller_handle {
        match tokio::time::timeout(tokio::time::Duration::from_secs(5), handle).await {
            Ok(Ok(())) => info!("Vote poller stopped"),
            Ok(Err(e)) => warn!(error = %e, "Vote poller task failed"),
            Err(_) => warn!("Vote poller did not stop within 5s, aborting"),
        }
    }

    info!(
        processed = processed_count,
        errors = error_count,
        "Shutdown complete"
    );

    Ok(())
}

/// Check if a block timestamp is too old based on `min_age_secs`.
///
/// Uses the on-chain `BlockchainMetadata.created_at` (epoch seconds) rather than
/// the Kafka message timestamp, which can be recent even for old blocks if the
/// Kafka topic was recently (re)populated.
fn is_block_too_old(block_created_at: u64, min_age_secs: u64) -> bool {
    if min_age_secs == 0 {
        return false;
    }
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    block_created_at > 0 && now_secs.saturating_sub(block_created_at) > min_age_secs
}

/// Wait for the kg-indexer to process blocks up to (at least) the message's block.
///
/// Polls `lookup_latest_block()` until `latest_kg_block >= msg_block + delay`,
/// or until `timeout_secs` elapses. Handles Anvil/dev environments where blocks
/// may not advance without transactions.
async fn wait_for_kg_catchup(storage: &Storage, msg_block: u64, delay: u64, timeout_secs: u64) {
    let target = msg_block.saturating_add(delay);
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);

    loop {
        if let Some(latest) = storage.lookup_latest_block().await {
            if latest >= target {
                return;
            }
            debug!(
                msg_block = msg_block,
                target = target,
                latest_kg_block = latest,
                "Waiting for kg-indexer to catch up"
            );
        } else {
            // No blocks in DB yet — don't wait
            return;
        }

        if tokio::time::Instant::now() >= deadline {
            debug!(
                msg_block = msg_block,
                timeout_secs = timeout_secs,
                "Block delay timeout, processing anyway (best-effort)"
            );
            return;
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

/// Enrich a notification event payload with human-readable names from the DB.
///
/// All lookups are best-effort — failures are silently ignored and the
/// corresponding field remains None (omitted from the webhook payload).
async fn enrich_payload(
    storage: &Storage,
    event: &mut notification_indexer::models::NotificationEvent,
    space_id: uuid::Uuid,
) {
    use notification_indexer::models::{NotificationData, NotificationEventType};

    // Space name (common to all event types). Prefer the space's page-entity
    // display name (e.g. "Wonderland"); fall back to the bare space entity's
    // name (the auto-generated "Space <uuid>" placeholder) only if there is no
    // page entity / name.
    event.payload.space_name = match storage.lookup_space_name(space_id).await {
        Some(name) => Some(name),
        None => storage.lookup_entity_name(space_id, space_id).await,
    };

    match &mut event.payload.data {
        NotificationData::Governance(ref mut gov) => {
            // Proposal name
            if let Ok(pid) = uuid::Uuid::parse_str(&gov.proposal_id) {
                gov.proposal_name = storage.lookup_proposal_name(pid).await;
            }

            // Proposer display name. `proposer_id` is the proposer's personal-space
            // UUID (proposals.proposed_by), so its display name lives on that
            // space's page entity — resolve it the same way as the space name, not
            // via lookup_entity_name on the bare id (which only finds the
            // "Space <uuid>" placeholder, scoped to the wrong space → null).
            if let Some(ref proposer_id) = gov.proposer_id {
                if let Ok(pid) = uuid::Uuid::parse_str(proposer_id) {
                    gov.proposer_name = storage.lookup_space_name(pid).await;
                }
            }

            // Voter display name
            if let Some(ref voter_id) = gov.voter_id {
                if let Ok(vid) = uuid::Uuid::parse_str(voter_id) {
                    gov.voter_name = storage.lookup_entity_name(vid, space_id).await;
                }
            }

            // Member/editor target display names — name *who* an editor/member
            // request is about. `target_address` on add_member/add_editor actions
            // is the target's personal-space UUID (hermes decodes addMember(bytes16)
            // into it), so it resolves to a name like any other space.
            if let Some(actions) = gov.actions.as_mut() {
                for action in actions.iter_mut() {
                    if matches!(action.action_type.as_str(), "add_member" | "add_editor") {
                        if let Some(target) = action.target_address.as_deref() {
                            if let Ok(target_space_id) = uuid::Uuid::parse_str(target) {
                                action.target_name =
                                    storage.lookup_space_name(target_space_id).await;
                            }
                        }
                    }
                }
            }

            // Vote tallies (proposal_voted events only)
            if event.event_type == NotificationEventType::ProposalVoted {
                if let Ok(pid) = uuid::Uuid::parse_str(&gov.proposal_id) {
                    if let Some((yes, no, abstain)) = storage.lookup_vote_tallies(pid).await {
                        gov.yes_count = Some(yes);
                        gov.no_count = Some(no);
                        gov.abstain_count = Some(abstain);
                    }
                }
            }
        }
        NotificationData::Bounty(ref mut bounty) => {
            // Bounty entity name
            if let Ok(bid) = uuid::Uuid::parse_str(&bounty.bounty_entity_id) {
                if let Ok(bsid) = uuid::Uuid::parse_str(&bounty.bounty_space_id) {
                    bounty.bounty_name = storage.lookup_entity_name(bid, bsid).await;
                }
            }

            // Curator display name
            if let Ok(cid) = uuid::Uuid::parse_str(&bounty.curator_space_id) {
                bounty.curator_name = storage.lookup_entity_name(cid, cid).await;
            }
        }
        NotificationData::BountyCreated(ref mut bc) => {
            // Bounty entity name (looked up in the bounty's space).
            if let (Ok(bid), Ok(bsid)) = (
                uuid::Uuid::parse_str(&bc.bounty_entity_id),
                uuid::Uuid::parse_str(&bc.bounty_space_id),
            ) {
                bc.bounty_name = storage.lookup_entity_name(bid, bsid).await;
            }
        }
        NotificationData::Comment(ref mut comment) => {
            // Proposal name (the proposal being commented on).
            if let Ok(pid) = uuid::Uuid::parse_str(&comment.proposal_id) {
                comment.proposal_name = storage.lookup_proposal_name(pid).await;
            }
        }
        NotificationData::GeneralComment(ref mut c) => {
            // Name of the thread root (looked up in its home space, == `space_id`).
            if let Ok(rid) = uuid::Uuid::parse_str(&c.root_id) {
                c.root_name = storage.lookup_entity_name(rid, space_id).await;
            }
        }
        NotificationData::VoteThreshold(ref mut vt) => {
            // Name of the entity that hit the threshold (looked up in its vote space).
            if let Ok(eid) = uuid::Uuid::parse_str(&vt.entity_id) {
                vt.entity_name = storage.lookup_entity_name(eid, space_id).await;
            }
        }
    }
}
