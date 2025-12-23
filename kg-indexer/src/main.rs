use std::collections::HashMap;
use std::env;
use std::time::{Duration, Instant};

use futures::StreamExt;
use rdkafka::Message;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod consumer;
mod error;
mod handlers;
mod models;
mod storage;

use consumer::{get_event_type, parse_message, KafkaConsumer, KgMessage};
use error::IndexerError;
use storage::Storage;

/// A buffered event with its Kafka metadata for later commit.
struct BufferedEvent {
    msg: KgMessage,
    topic: String,
    partition: i32,
    offset: i64,
}

/// Buffer for events by block number.
struct BlockBuffer {
    /// Events grouped by block number.
    events: HashMap<u64, Vec<BufferedEvent>>,
    /// When each block was first seen.
    first_seen: HashMap<u64, Instant>,
    /// Timeout for waiting for is_last event.
    stale_timeout: Duration,
}

impl BlockBuffer {
    fn new(stale_timeout: Duration) -> Self {
        Self {
            events: HashMap::new(),
            first_seen: HashMap::new(),
            stale_timeout,
        }
    }

    /// Add an event to the buffer.
    fn push(&mut self, block_number: u64, event: BufferedEvent) {
        self.first_seen
            .entry(block_number)
            .or_insert_with(Instant::now);
        self.events.entry(block_number).or_default().push(event);
    }

    /// Remove and return all events for a block, sorted by sequence.
    fn take_block(&mut self, block_number: u64) -> Vec<BufferedEvent> {
        self.first_seen.remove(&block_number);
        let mut events = self.events.remove(&block_number).unwrap_or_default();
        events.sort_by_key(|e| e.msg.sequence());
        events
    }

    /// Get block numbers that have been buffered longer than the stale timeout.
    fn stale_blocks(&self) -> Vec<u64> {
        let now = Instant::now();
        self.first_seen
            .iter()
            .filter(|(_, first_seen)| now.duration_since(**first_seen) > self.stale_timeout)
            .map(|(block, _)| *block)
            .collect()
    }
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting kg-indexer");

    // Load configuration from environment
    let database_url =
        env::var("DATABASE_URL").map_err(|_| IndexerError::config("DATABASE_URL not set"))?;
    let kafka_broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let kafka_group_id = env::var("KAFKA_GROUP_ID").unwrap_or_else(|_| "kg-indexer".to_string());
    let stale_timeout_secs: u64 = env::var("BLOCK_STALE_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    // Initialize storage
    let storage = Storage::new(&database_url).await?;
    info!("Connected to database");

    // Initialize Kafka consumer
    let consumer = KafkaConsumer::new(&kafka_broker, &kafka_group_id)?;
    consumer.subscribe()?;

    // Set up shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        shutdown_tx.send(()).ok();
    });

    // Main processing loop
    //
    // Events are buffered by block number and processed together when `is_last=true`
    // arrives, ensuring correct ordering within a block. However, we can't rely solely
    // on `is_last` because:
    //   1. It may arrive on a topic we don't subscribe to (e.g., curation.votes)
    //   2. The producer may crash before sending it
    //   3. Network issues may cause it to be lost
    //
    // To handle these cases, we use `tokio::select!` with a periodic tick that checks
    // for stale blocks (buffered longer than `stale_timeout`). The tick runs independently
    // of the Kafka stream, so even if no messages arrive, stale blocks get processed.
    let mut stream = consumer.stream();
    let stale_timeout = Duration::from_secs(stale_timeout_secs);
    let mut buffer = BlockBuffer::new(stale_timeout);
    let mut processed_count: u64 = 0;
    let mut error_count: u64 = 0;
    let mut blocks_processed: u64 = 0;
    let mut stale_check_interval = tokio::time::interval(Duration::from_secs(1));

    info!(
        stale_timeout_secs = stale_timeout_secs,
        "Starting message processing loop"
    );

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutting down...");
                break;
            }

            _ = stale_check_interval.tick() => {
                // Periodically check for stale blocks and force-process them
                for block_number in buffer.stale_blocks() {
                    let events = buffer.take_block(block_number);
                    let event_count = events.len();
                    warn!(
                        block_number = block_number,
                        event_count = event_count,
                        stale_timeout_secs = stale_timeout_secs,
                        "Force-processing stale block"
                    );
                    match process_block(events, &storage, &consumer).await {
                        Ok(count) => {
                            processed_count += count as u64;
                            blocks_processed += 1;
                            info!(
                                block_number = block_number,
                                processed = count,
                                "Force-processed stale block"
                            );
                        }
                        Err(e) => {
                            error_count += 1;
                            error!(
                                block_number = block_number,
                                error = %e,
                                "Failed to process stale block"
                            );
                        }
                    }
                }
            }

            message = stream.next() => {
                match message {
                    Some(Ok(msg)) => {
                        let topic = msg.topic().to_string();
                        let partition = msg.partition();
                        let offset = msg.offset();

                        if let Some(payload) = msg.payload() {
                            let event_type = get_event_type(msg.headers());
                            match parse_message(&topic, payload, event_type.as_deref()) {
                                Ok(kg_msg) => {
                                    // Get block number from metadata
                                    let block_number = match kg_msg.block_number() {
                                        Some(bn) => bn,
                                        None => {
                                            // Fall back to immediate processing if no metadata
                                            warn!(
                                                topic = %topic,
                                                "Message has no block metadata, processing immediately"
                                            );
                                            match process_message(kg_msg, &storage).await {
                                                Ok(_) => {
                                                    processed_count += 1;
                                                }
                                                Err(e) => {
                                                    error!(error = %e, "Failed to process message");
                                                    error_count += 1;
                                                }
                                            }
                                            if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                                error!(error = %e, "Failed to commit offset");
                                            }
                                            continue;
                                        }
                                    };

                                    let is_last = kg_msg.is_last();

                                    // Buffer the message
                                    buffer.push(block_number, BufferedEvent {
                                        msg: kg_msg,
                                        topic,
                                        partition,
                                        offset,
                                    });

                                    // If this is the last event in the block, process all buffered events
                                    if is_last {
                                        let events = buffer.take_block(block_number);
                                        let event_count = events.len();

                                        debug!(
                                            block_number = block_number,
                                            event_count = event_count,
                                            "Processing block"
                                        );

                                        match process_block(events, &storage, &consumer).await {
                                            Ok(ops) => {
                                                processed_count += event_count as u64;
                                                blocks_processed += 1;
                                                debug!(
                                                    block_number = block_number,
                                                    ops = ops,
                                                    "Block processed"
                                                );
                                            }
                                            Err(e) => {
                                                error!(
                                                    block_number = block_number,
                                                    error = %e,
                                                    "Failed to process block"
                                                );
                                                error_count += event_count as u64;
                                            }
                                        }

                                        if blocks_processed.is_multiple_of(10) && blocks_processed > 0 {
                                            info!(
                                                blocks = blocks_processed,
                                                messages = processed_count,
                                                errors = error_count,
                                                "Progress update"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        topic = %topic,
                                        partition = partition,
                                        offset = offset,
                                        error = %e,
                                        "Failed to parse message"
                                    );
                                    error_count += 1;

                                    // Still commit to avoid getting stuck
                                    if let Err(e) = consumer.commit_message(&topic, partition, offset) {
                                        error!(error = %e, "Failed to commit offset");
                                    }
                                }
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

    info!(
        processed = processed_count,
        errors = error_count,
        "Shutdown complete"
    );

    Ok(())
}

/// Process a single Kafka message within its own transaction.
/// Returns the number of database operations performed.
async fn process_message(msg: KgMessage, storage: &Storage) -> Result<usize, IndexerError> {
    use handlers::membership::MembershipChange;
    use models::relations::RelationOp;
    use models::values::ValueChangeType;

    let mut tx = storage.pool.begin().await?;
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await?;

    let ops = match msg {
        KgMessage::Edit(edit) => {
            let result = handlers::edits::handle_edit(&edit)?;

            // Partition values into sets and deletes
            let (set_values, delete_values): (Vec<_>, Vec<_>) = result
                .values
                .into_iter()
                .partition(|v| matches!(v.change_type, ValueChangeType::Set));

            let delete_value_ids: Vec<_> = delete_values
                .into_iter()
                .map(|v| (v.id, v.space_id))
                .collect();

            // Partition relations by operation type
            let mut set_relations = Vec::new();
            let mut update_relations = Vec::new();
            let mut unset_relations = Vec::new();
            let mut delete_relations = Vec::new();

            for op in result.relations {
                match op {
                    RelationOp::Create(r) => set_relations.push(r),
                    RelationOp::Update(r) => update_relations.push(r),
                    RelationOp::Unset(r) => unset_relations.push(r),
                    RelationOp::Delete(r) => delete_relations.push((r.id, r.space_id)),
                }
            }

            let ops = result.entities.len()
                + result.properties.len()
                + set_values.len()
                + delete_value_ids.len()
                + set_relations.len()
                + update_relations.len()
                + unset_relations.len()
                + delete_relations.len();

            // Bulk insert all operations
            storage.insert_entities(&result.entities, &mut tx).await?;
            storage
                .insert_properties(&result.properties, &mut tx)
                .await?;
            storage.insert_values(&set_values, &mut tx).await?;
            storage.delete_values(&delete_value_ids, &mut tx).await?;
            storage.insert_relations(&set_relations, &mut tx).await?;
            storage.update_relations(&update_relations, &mut tx).await?;
            storage
                .unset_relation_fields(&unset_relations, &mut tx)
                .await?;
            storage.delete_relations(&delete_relations, &mut tx).await?;

            ops
        }
        KgMessage::CreateSpace(space) => {
            let space_item = handlers::spaces::handle_create_space(&space)?;
            storage.insert_spaces(&[space_item], &mut tx).await?;
            1
        }
        KgMessage::RoleGranted(event) => {
            match handlers::membership::handle_role_granted(&event)? {
                MembershipChange::AddEditor(e) => {
                    storage.insert_editors(&[e], &mut tx).await?;
                }
                MembershipChange::AddMember(m) => {
                    storage.insert_members(&[m], &mut tx).await?;
                }
                _ => {} // Shouldn't happen for granted
            }
            1
        }
        KgMessage::RoleRevoked(event) => {
            match handlers::membership::handle_role_revoked(&event)? {
                MembershipChange::RemoveEditor(e) => {
                    storage.remove_editors(&[e], &mut tx).await?;
                }
                MembershipChange::RemoveMember(m) => {
                    storage.remove_members(&[m], &mut tx).await?;
                }
                _ => {} // Shouldn't happen for revoked
            }
            1
        }
        KgMessage::TrustExtension(event) => {
            if let Some(subspace) = handlers::subspaces::handle_trust_extension(&event)? {
                storage.insert_subspaces(&[subspace], &mut tx).await?;
                1
            } else {
                0
            }
        }
        KgMessage::ProposalCreated(event) => {
            let result = handlers::governance::handle_proposal_created(&event)?;
            debug!(
                proposal_id = %result.proposal.id,
                actions = result.actions.len(),
                "Processing ProposalCreated"
            );
            storage
                .insert_proposals(&[result.proposal], &mut tx)
                .await?;
            if !result.actions.is_empty() {
                storage
                    .insert_proposal_actions(&result.actions, &mut tx)
                    .await?;
            }
            1 + result.actions.len()
        }
        KgMessage::ProposalVoted(event) => {
            let vote = handlers::governance::handle_proposal_voted(&event)?;
            debug!(
                proposal_id = %vote.proposal_id,
                voter_id = %vote.voter_id,
                "Processing ProposalVoted"
            );
            storage.insert_proposal_votes(&[vote], &mut tx).await?;
            1
        }
        KgMessage::ProposalExecuted(event) => {
            let execution = handlers::governance::handle_proposal_executed(&event)?;
            debug!(
                proposal_id = %execution.proposal_id,
                "Processing ProposalExecuted"
            );
            storage
                .update_proposal_executed(execution.proposal_id, execution.executed_at, &mut tx)
                .await?;
            1
        }
    };

    tx.commit().await?;

    Ok(ops)
}

/// Process all events in a block within a single transaction.
/// Events should already be sorted by sequence.
/// Returns the total number of database operations performed.
async fn process_block(
    events: Vec<BufferedEvent>,
    storage: &Storage,
    consumer: &KafkaConsumer,
) -> Result<usize, IndexerError> {
    use handlers::membership::MembershipChange;
    use models::relations::RelationOp;
    use models::values::ValueChangeType;

    if events.is_empty() {
        return Ok(0);
    }

    let mut tx = storage.pool.begin().await?;
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await?;
    let mut total_ops = 0;

    // Process each message in sequence order
    for event in &events {
        let ops = match &event.msg {
            KgMessage::Edit(edit) => {
                let result = handlers::edits::handle_edit(edit)?;

                // Partition values into sets and deletes
                let (set_values, delete_values): (Vec<_>, Vec<_>) = result
                    .values
                    .into_iter()
                    .partition(|v| matches!(v.change_type, ValueChangeType::Set));

                let delete_value_ids: Vec<_> = delete_values
                    .into_iter()
                    .map(|v| (v.id, v.space_id))
                    .collect();

                // Partition relations by operation type
                let mut set_relations = Vec::new();
                let mut update_relations = Vec::new();
                let mut unset_relations = Vec::new();
                let mut delete_relations = Vec::new();

                for op in result.relations {
                    match op {
                        RelationOp::Create(r) => set_relations.push(r),
                        RelationOp::Update(r) => update_relations.push(r),
                        RelationOp::Unset(r) => unset_relations.push(r),
                        RelationOp::Delete(r) => delete_relations.push((r.id, r.space_id)),
                    }
                }

                let ops = result.entities.len()
                    + result.properties.len()
                    + set_values.len()
                    + delete_value_ids.len()
                    + set_relations.len()
                    + update_relations.len()
                    + unset_relations.len()
                    + delete_relations.len();

                // Bulk insert all operations
                storage.insert_entities(&result.entities, &mut tx).await?;
                storage
                    .insert_properties(&result.properties, &mut tx)
                    .await?;
                storage.insert_values(&set_values, &mut tx).await?;
                storage.delete_values(&delete_value_ids, &mut tx).await?;
                storage.insert_relations(&set_relations, &mut tx).await?;
                storage.update_relations(&update_relations, &mut tx).await?;
                storage
                    .unset_relation_fields(&unset_relations, &mut tx)
                    .await?;
                storage.delete_relations(&delete_relations, &mut tx).await?;

                ops
            }
            KgMessage::CreateSpace(space) => {
                let space_item = handlers::spaces::handle_create_space(space)?;
                storage.insert_spaces(&[space_item], &mut tx).await?;
                1
            }
            KgMessage::RoleGranted(event) => {
                match handlers::membership::handle_role_granted(event)? {
                    MembershipChange::AddEditor(e) => {
                        storage.insert_editors(&[e], &mut tx).await?;
                    }
                    MembershipChange::AddMember(m) => {
                        storage.insert_members(&[m], &mut tx).await?;
                    }
                    _ => {}
                }
                1
            }
            KgMessage::RoleRevoked(event) => {
                match handlers::membership::handle_role_revoked(event)? {
                    MembershipChange::RemoveEditor(e) => {
                        storage.remove_editors(&[e], &mut tx).await?;
                    }
                    MembershipChange::RemoveMember(m) => {
                        storage.remove_members(&[m], &mut tx).await?;
                    }
                    _ => {}
                }
                1
            }
            KgMessage::TrustExtension(event) => {
                if let Some(subspace) = handlers::subspaces::handle_trust_extension(event)? {
                    storage.insert_subspaces(&[subspace], &mut tx).await?;
                    1
                } else {
                    0
                }
            }
            KgMessage::ProposalCreated(event) => {
                let result = handlers::governance::handle_proposal_created(event)?;
                debug!(
                    proposal_id = %result.proposal.id,
                    actions = result.actions.len(),
                    "Processing ProposalCreated"
                );
                storage
                    .insert_proposals(&[result.proposal], &mut tx)
                    .await?;
                if !result.actions.is_empty() {
                    storage
                        .insert_proposal_actions(&result.actions, &mut tx)
                        .await?;
                }
                1 + result.actions.len()
            }
            KgMessage::ProposalVoted(event) => {
                let vote = handlers::governance::handle_proposal_voted(event)?;
                debug!(
                    proposal_id = %vote.proposal_id,
                    voter_id = %vote.voter_id,
                    "Processing ProposalVoted"
                );
                storage.insert_proposal_votes(&[vote], &mut tx).await?;
                1
            }
            KgMessage::ProposalExecuted(event) => {
                let execution = handlers::governance::handle_proposal_executed(event)?;
                debug!(
                    proposal_id = %execution.proposal_id,
                    "Processing ProposalExecuted"
                );
                storage
                    .update_proposal_executed(execution.proposal_id, execution.executed_at, &mut tx)
                    .await?;
                1
            }
        };

        total_ops += ops;
    }

    // Commit the transaction
    tx.commit().await?;

    // Commit Kafka offsets for all processed messages
    for event in events {
        if let Err(e) = consumer.commit_message(&event.topic, event.partition, event.offset) {
            error!(
                topic = %event.topic,
                partition = event.partition,
                offset = event.offset,
                error = %e,
                "Failed to commit offset"
            );
        }
    }

    Ok(total_ops)
}
