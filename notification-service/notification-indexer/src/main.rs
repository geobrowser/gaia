//! Notification indexer entry point.
//!
//! Two concurrent tasks:
//! 1. Kafka consumer — subscribes to `space.governance`, processes all governance
//!    events (PROPOSAL_CREATED, PROPOSAL_UPDATED, PROPOSAL_VOTED,
//!    PROPOSAL_EXECUTED, PROPOSAL_SETTINGS_UPDATED) and writes to the notification outbox.
//! 2. Rejection poller — every 60s, finds proposals that expired without execution
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
    build_rejection_event, extract_bounty_relations, handle_bounty_allocated,
    handle_bounty_interest, handle_bounty_payout, handle_proposal_created,
    handle_proposal_executed, handle_proposal_settings_updated, handle_proposal_updated,
    handle_proposal_voted, BountyConfig, NotificationEventType,
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
    let kafka_group_id =
        env::var("KAFKA_GROUP_ID").unwrap_or_else(|_| "notification-indexer".to_string());

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

    // Initialize storage
    let storage = Storage::connect(&database_url).await?;
    info!("Connected to database");

    // Start health check server
    let _health_handle =
        notification_indexer::health::start_health_server(storage.pool().clone(), health_port);

    // Initialize Kafka consumer
    let consumer = KafkaConsumer::new(&kafka_broker, &kafka_group_id)?;
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
        &kafka_group_id,
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

    // Knowledge edits consumer (optional, default disabled)
    let knowledge_edits_enabled = env::var("KNOWLEDGE_EDITS_ENABLED")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);

    if !knowledge_edits_enabled {
        info!("Knowledge edits consumer disabled (set KNOWLEDGE_EDITS_ENABLED=true to enable)");
    }

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
                                    if editors.is_empty() {
                                        continue;
                                    }
                                    match poller_storage.insert_notifications_for_editors(&event, &editors).await {
                                        Ok(count) if count > 0 => {
                                            info!(
                                                proposal_id = %proposal.id,
                                                editors = editors.len(),
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

    // Spawn knowledge edits consumer task (if enabled)
    let ke_handle: Option<tokio::task::JoinHandle<()>> = if knowledge_edits_enabled {
        let ke_kafka_broker = kafka_broker.clone();
        let ke_kafka_group_id = kafka_group_id.clone();
        let ke_bd = block_delay;
        let ke_bd_timeout = block_delay_timeout_secs;
        let ke_min_age = min_age_secs;
        let ke_stor = Storage::new(storage.pool().clone());
        let ke_cfg = bounty_config;

        Some(tokio::spawn(async move {
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

            info!("Knowledge edits consumer loop started");

            loop {
                tokio::select! {
                    _ = shutdown_rx3.recv() => {
                        info!("Knowledge edits consumer shutting down");
                        break;
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

                                if relations.is_empty() {
                                    // No bounty relations in this edit — commit and continue
                                    if let Err(e) = kec.commit_message(&topic, partition, offset) {
                                        error!(error = %e, "Failed to commit knowledge edits offset");
                                    }
                                    continue;
                                }

                                let mut all_ok = true;
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
                                                    continue;
                                                }
                                                Err(e) => {
                                                    error!(error = %e, "DB error looking up bounty space, will retry");
                                                    ke_errors += 1;
                                                    all_ok = false;
                                                    break;
                                                }
                                            }

                                            let mut event = handle_bounty_interest(&info);
                                            enrich_payload(&ke_stor, &mut event, info.bounty_space_id).await;

                                            let editors = match ke_stor.find_editors_for_space(info.bounty_space_id).await {
                                                Ok(eds) => eds,
                                                Err(e) => {
                                                    error!(error = %e, "DB error looking up editors for bounty interest, will retry");
                                                    ke_errors += 1;
                                                    all_ok = false;
                                                    break;
                                                }
                                            };

                                            if editors.is_empty() {
                                                ke_processed += 1;
                                                continue;
                                            }

                                            match ke_stor.insert_notifications_for_editors(&event, &editors).await {
                                                Ok(count) => {
                                                    if count > 0 {
                                                        info!(
                                                            event_type = "bounty_interest",
                                                            editors = editors.len(),
                                                            inserted = count,
                                                            "Inserted bounty interest notifications"
                                                        );
                                                    }
                                                    ke_processed += 1;
                                                }
                                                Err(e) => {
                                                    error!(error = %e, "DB error inserting bounty interest notifications, will retry");
                                                    ke_errors += 1;
                                                    all_ok = false;
                                                    break;
                                                }
                                            }
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
                                                            event_type = %event_type.as_str(),
                                                            "Could not resolve curator space, skipping notification"
                                                        );
                                                        continue;
                                                    }
                                                    Err(e) => {
                                                        error!(error = %e, "DB error looking up curator space, will retry");
                                                        ke_errors += 1;
                                                        all_ok = false;
                                                        break;
                                                    }
                                                }
                                            }

                                            let mut event = if event_type == NotificationEventType::BountyAllocated {
                                                handle_bounty_allocated(&info)
                                            } else {
                                                handle_bounty_payout(&info)
                                            };
                                            enrich_payload(&ke_stor, &mut event, info.bounty_space_id).await;

                                            match ke_stor.insert_notification_for_user(&event, info.curator_space_id).await {
                                                Ok(count) => {
                                                    if count > 0 {
                                                        info!(
                                                            event_type = %event_type.as_str(),
                                                            curator_space_id = %info.curator_space_id,
                                                            inserted = count,
                                                            "Inserted bounty notification for curator"
                                                        );
                                                    }
                                                    ke_processed += 1;
                                                }
                                                Err(e) => {
                                                    error!(
                                                        error = %e,
                                                        event_type = %event_type.as_str(),
                                                        "DB error inserting bounty notification, will retry"
                                                    );
                                                    ke_errors += 1;
                                                    all_ok = false;
                                                    break;
                                                }
                                            }
                                        }
                                        _ => {
                                            continue;
                                        }
                                    }
                                }

                                // Only commit offset if all relations in the edit were processed successfully.
                                // On DB error: don't commit — retry on restart.
                                if all_ok {
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
        }))
    } else {
        None
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
                let outbox_total = sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM notification_outbox"
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
                    outbox_total = outbox_total,
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

                                    if editors.is_empty() {
                                        // Genuinely no editors — this is normal, not an error
                                        processed_count += 1;
                                        should_commit = true;
                                    } else {
                                        match storage.insert_notifications_for_editors(&event, &editors).await {
                                            Ok(count) => {
                                                if count > 0 {
                                                    info!(
                                                        event_type = %event.payload.event_type,
                                                        category = %event.payload.category,
                                                        editors = editors.len(),
                                                        inserted = count,
                                                        "Inserted per-editor notifications"
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

    // Wait for knowledge edits consumer to finish (if enabled)
    if let Some(handle) = ke_handle {
        match tokio::time::timeout(tokio::time::Duration::from_secs(5), handle).await {
            Ok(Ok(())) => info!("Knowledge edits consumer stopped"),
            Ok(Err(e)) => warn!(error = %e, "Knowledge edits consumer task failed"),
            Err(_) => {
                warn!("Knowledge edits consumer did not stop within 5s, aborting");
            }
        }
    }

    info!(
        processed = processed_count,
        errors = error_count,
        "Shutdown complete"
    );

    Ok(())
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

    // Space name (common to all event types)
    event.payload.space_name = storage.lookup_entity_name(space_id, space_id).await;

    match &mut event.payload.data {
        NotificationData::Governance(ref mut gov) => {
            // Proposal name
            if let Ok(pid) = uuid::Uuid::parse_str(&gov.proposal_id) {
                gov.proposal_name = storage.lookup_proposal_name(pid).await;
            }

            // Proposer display name
            if let Some(ref proposer_id) = gov.proposer_id {
                if let Ok(pid) = uuid::Uuid::parse_str(proposer_id) {
                    gov.proposer_name = storage.lookup_entity_name(pid, space_id).await;
                }
            }

            // Voter display name
            if let Some(ref voter_id) = gov.voter_id {
                if let Ok(vid) = uuid::Uuid::parse_str(voter_id) {
                    gov.voter_name = storage.lookup_entity_name(vid, space_id).await;
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
    }
}
