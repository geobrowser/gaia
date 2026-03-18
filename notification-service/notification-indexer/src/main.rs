//! Notification indexer entry point.
//!
//! Two concurrent tasks:
//! 1. Kafka consumer — subscribes to `space.governance`, processes all governance
//!    events (PROPOSAL_CREATED, PROPOSAL_UPDATED, PROPOSAL_VOTED,
//!    PROPOSAL_EXECUTED, PROPOSAL_SETTINGS_UPDATED) and writes to the notification outbox.
//! 2. Rejection poller — every 60s, finds proposals that expired without execution
//!    and writes rejection notifications.

use std::env;

use futures::StreamExt;
use hermes_instrumentation::{error, info, warn};
use rdkafka::message::Message;

use notification_indexer::consumer::{
    get_event_type, parse_proposal_created, parse_proposal_executed, parse_proposal_settings_updated,
    parse_proposal_updated, parse_proposal_voted, KafkaConsumer,
};
use notification_indexer::error::IndexerError;
use notification_indexer::models::{
    build_rejection_event, handle_proposal_created, handle_proposal_executed,
    handle_proposal_settings_updated, handle_proposal_updated, handle_proposal_voted,
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

    // Initialize storage
    let storage = Storage::connect(&database_url).await?;
    info!("Connected to database");

    // Initialize Kafka consumer
    let consumer = KafkaConsumer::new(&kafka_broker, &kafka_group_id)?;
    consumer.subscribe()?;

    // Set up shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
    let mut shutdown_rx2 = shutdown_tx.subscribe();

    tokio::spawn(async move {
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
                    match poller_storage.find_expired_proposals().await {
                        Ok(expired) => {
                            if !expired.is_empty() {
                                info!(count = expired.len(), "Found expired proposals");
                            }
                            for proposal in expired {
                                let event = build_rejection_event(
                                    proposal.id,
                                    proposal.space_id,
                                    proposal.proposed_by,
                                    proposal.end_time,
                                );
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
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to query expired proposals");
                        }
                    }
                }
            }
        }
    });

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
                info!(
                    processed = processed_count,
                    errors = error_count,
                    "Heartbeat"
                );
            }

            message = stream.next() => {
                match message {
                    Some(Ok(msg)) => {
                        let topic = msg.topic().to_string();
                        let partition = msg.partition();
                        let offset = msg.offset();

                        let event_type = get_event_type(msg.headers());

                        if let Some(payload) = msg.payload() {
                            let result = match event_type.as_deref() {
                                Some("PROPOSAL_CREATED") => {
                                    parse_proposal_created(payload)
                                        .map_err(IndexerError::from)
                                        .and_then(|proto| {
                                            handle_proposal_created(&proto)
                                                .map_err(IndexerError::from)
                                        })
                                }
                                Some("PROPOSAL_UPDATED") => {
                                    parse_proposal_updated(payload)
                                        .map_err(IndexerError::from)
                                        .and_then(|proto| {
                                            handle_proposal_updated(&proto)
                                                .map_err(IndexerError::from)
                                        })
                                }
                                Some("PROPOSAL_VOTED") => {
                                    parse_proposal_voted(payload)
                                        .map_err(IndexerError::from)
                                        .and_then(|proto| {
                                            handle_proposal_voted(&proto)
                                                .map_err(IndexerError::from)
                                        })
                                }
                                Some("PROPOSAL_EXECUTED") => {
                                    parse_proposal_executed(payload)
                                        .map_err(IndexerError::from)
                                        .and_then(|proto| {
                                            handle_proposal_executed(&proto)
                                                .map_err(IndexerError::from)
                                        })
                                }
                                Some("PROPOSAL_SETTINGS_UPDATED") => {
                                    parse_proposal_settings_updated(payload)
                                        .map_err(IndexerError::from)
                                        .and_then(|proto| {
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
                                Ok(event) => {
                                    // Resolve editors for the space
                                    let space_id = match uuid::Uuid::parse_str(&event.payload.space_id) {
                                        Ok(sid) => sid,
                                        Err(e) => {
                                            error!(error = %e, "Invalid space_id in event");
                                            error_count += 1;
                                            if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                                error!(error = %e, "Failed to commit offset");
                                            }
                                            continue;
                                        }
                                    };

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
                                    } else {
                                        match storage.insert_notifications_for_editors(&event, &editors).await {
                                            Ok(count) => {
                                                if count > 0 {
                                                    info!(
                                                        event_type = %event.payload.event_type,
                                                        proposal_id = %event.payload.proposal_id,
                                                        editors = editors.len(),
                                                        inserted = count,
                                                        "Inserted per-editor notifications"
                                                    );
                                                }
                                                processed_count += 1;
                                            }
                                            Err(e) => {
                                                error!(
                                                    error = %e,
                                                    event_type = %event.payload.event_type,
                                                    "Failed to insert notifications"
                                                );
                                                error_count += 1;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        error = %e,
                                        partition = partition,
                                        offset = offset,
                                        "Failed to handle governance message"
                                    );
                                    error_count += 1;
                                }
                            }
                        }

                        // Commit offset regardless of processing outcome to avoid getting stuck
                        if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                            error!(error = %e, "Failed to commit offset");
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

    // Wait for poller to finish gracefully (it should exit via shutdown_rx2)
    match tokio::time::timeout(tokio::time::Duration::from_secs(5), poller_handle).await {
        Ok(Ok(())) => info!("Rejection poller stopped"),
        Ok(Err(e)) => warn!(error = %e, "Rejection poller task failed"),
        Err(_) => {
            warn!("Rejection poller did not stop within 5s, aborting");
        }
    }

    info!(
        processed = processed_count,
        errors = error_count,
        "Shutdown complete"
    );

    Ok(())
}
