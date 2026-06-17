//! Hermes Pipeline
//!
//! Consumes space-related events from hermes-substream via hermes-relay and
//! transforms them into Hermes protobuf messages for publication to Kafka.
//!
//! ## Event Types Handled
//!
//! - `SPACE_REGISTERED` - new space registrations -> `space.creations` topic
//! - `SUBSPACE_VERIFIED/RELATED/TOPIC_SET/UNSET` - trust events -> `space.trust.extensions` topic
//! - `EDITOR/MEMBER_ADDED/REMOVED`, `SPACE_LEFT` - membership -> `space.membership` topic
//! - `EDITOR_FLAGGED/UNFLAGGED`, `FLAGGED/UNFLAGGED` - moderation -> `space.moderation` topic
//! - `TOPIC_SET` - topic declarations -> `space.topics` topic
//! - `PROPOSAL_CREATED/VOTED/EXECUTED` - governance -> `space.governance` topic
//! - `UPVOTED/DOWNVOTED/UNVOTED` - curation voting -> `curation.votes` topic
//! - `EDITS_PUBLISHED` - edit publications -> `knowledge.edits` topic
//!
//! ## Architecture
//!
//! The pipeline processes blocks in two phases:
//! 1. **Transform**: All pipelines run concurrently. Edits pipeline (async with IPFS fetch)
//!    is kicked off first so network I/O happens in parallel with sync transforms.
//! 2. **Emit**: Send events to Kafka in order (spaces, membership, trust, moderation,
//!    topics, governance, voting, edits)
//!
//! ## Configuration
//!
//! Environment variables:
//! - `USE_MOCK` - Set to "true" or "1" to use mock data (default: false)
//! - `SUBSTREAMS_ENDPOINT` - Substreams endpoint URL (default: geotest.substreams.pinax.network:443)
//! - `SUBSTREAMS_API_TOKEN` - API token for substreams authentication
//! - `SUBSTREAMS_START_BLOCK` - First block to consume on cold start (default: 138000).
//!   Ignored when a persisted cursor exists in the `meta` table.
//! - `SUBSTREAMS_END_BLOCK` - Last block to consume (default: u64::MAX for continuous)
//! - `KAFKA_BROKER` - Kafka broker address (default: localhost:9092)
//! - `KAFKA_USERNAME` - SASL username for managed Kafka (optional)
//! - `KAFKA_PASSWORD` - SASL password for managed Kafka (optional)
//! - `KAFKA_SSL_CA_PEM` - Custom CA cert for SSL (optional)
//! - `DATABASE_URL` - PostgreSQL URL for IPFS cache (required when USE_MOCK=false)

mod emit;

use std::collections::HashMap;
use std::env;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use hermes_instrumentation::{Instrument, debug, error, info, info_span, warn};
use prost::Message;
use std::sync::OnceLock;

use hermes_kafka::create_producer;
use hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockScopedData;
use hermes_relay::stream::utils;
use hermes_relay::{Actions, HermesModule, Sink, StreamSource};

use hermes_pipeline::cache::{CacheSource, IpfsCache};
use hermes_pipeline::cursor::{self, CursorStore, MockCursorStore, PostgresCursorStore};
use hermes_pipeline::pipelines;
use hermes_pipeline::pipelines::BlockMetadata;
use hermes_pipeline::pipelines::prefetch::{self, RetryConfig};
use hermes_pipeline::pipelines::trust::get_extension_type;
use hermes_pipeline::pipelines::voting::get_vote_direction;

use emit::{Emitter, topics};

/// Error type for the pipeline that implements std::error::Error
#[derive(Debug)]
pub struct PipelineError(anyhow::Error);

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PipelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl From<anyhow::Error> for PipelineError {
    fn from(err: anyhow::Error) -> Self {
        PipelineError(err)
    }
}

impl From<prost::DecodeError> for PipelineError {
    fn from(err: prost::DecodeError) -> Self {
        PipelineError(anyhow::Error::from(err))
    }
}

/// Pipeline transformer that processes all space-related events.
///
/// Subscribes to `HermesModule::Actions` and processes:
/// - `SPACE_REGISTERED` -> spaces pipeline
/// - `SUBSPACE_VERIFIED/RELATED/TOPIC_SET/UNSET` -> trust pipeline
/// - `EDITOR/MEMBER_ADDED/REMOVED`, `SPACE_LEFT` -> membership pipeline
/// - `EDITOR_FLAGGED/UNFLAGGED`, `FLAGGED/UNFLAGGED` -> moderation pipeline
/// - `TOPIC_SET` -> topics pipeline
/// - `PROPOSAL_CREATED/VOTED/EXECUTED` -> governance pipeline
/// - `UPVOTED/DOWNVOTED/UNVOTED` -> voting pipeline
/// - `EDITS_PUBLISHED` -> edits pipeline (with IPFS cache lookup)
///
/// The edits pipeline is kicked off first (async) so IPFS fetching happens
/// in parallel with the sync transforms. All events are emitted to Kafka in order.
pub struct Pipeline {
    emitter: Emitter,
    cache: Arc<dyn IpfsCache>,
    cursor_store: Arc<dyn CursorStore>,
    /// Clock timestamp (Unix seconds) of the most recently processed block.
    /// Stashed by `process_block_impl` so `persist_cursor` can update lag
    /// gauges only after the cursor has been durably written. Zero means
    /// "no block processed yet this run" (e.g., right after startup).
    last_block_timestamp: AtomicI64,
    retry_config: RetryConfig,
}

impl Pipeline {
    pub fn new(
        emitter: Emitter,
        cache: Arc<dyn IpfsCache>,
        cursor_store: Arc<dyn CursorStore>,
    ) -> Self {
        Self {
            emitter,
            cache,
            cursor_store,
            last_block_timestamp: AtomicI64::new(0),
            retry_config: RetryConfig::default(),
        }
    }

    /// Create a pipeline with custom retry configuration.
    #[allow(dead_code)]
    pub fn with_retry_config(
        emitter: Emitter,
        cache: Arc<dyn IpfsCache>,
        cursor_store: Arc<dyn CursorStore>,
        retry_config: RetryConfig,
    ) -> Self {
        Self {
            emitter,
            cache,
            cursor_store,
            last_block_timestamp: AtomicI64::new(0),
            retry_config,
        }
    }

    /// Flush all pending messages to Kafka.
    pub fn flush(&self, timeout: std::time::Duration) {
        self.emitter.flush(timeout);
    }
}

impl Pipeline {
    async fn process_block_impl(
        &self,
        output_value: &[u8],
        relay_meta: hermes_relay::stream::utils::BlockMetadata,
        meta: BlockMetadata,
    ) -> Result<(), PipelineError> {
        // Decode the Actions message from the block output
        let actions_msg = Actions::decode(output_value)?;
        let actions = &actions_msg.actions;

        // =========================================================================
        // Phase 0: Prefetch all IPFS URIs needed for this block
        // =========================================================================
        // Batch all cache lookups at the start so transform functions can be sync.
        // This fetches URIs for both EDITS_PUBLISHED and PROPOSAL_CREATED actions.
        let prefetch_result = prefetch::prefetch_block(actions, &self.cache, &self.retry_config)
            .instrument(info_span!("prefetch", action_count = actions.len()))
            .await;

        // =========================================================================
        // Phase 1: Transform actions into events
        // =========================================================================

        // All transforms are now synchronous - they use the prefetched cache.
        let mut spaces = info_span!("transform.spaces", action_count = actions.len())
            .in_scope(|| pipelines::spaces::transform(actions, &meta))
            .map_err(|e| {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "transform.spaces",
                    block_number = meta.block_number,
                    error = %e,
                    "Transform failed"
                );
                e
            })?;

        let mut membership = info_span!("transform.membership", action_count = actions.len())
            .in_scope(|| pipelines::membership::transform(actions, &meta))
            .map_err(|e| {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "transform.membership",
                    block_number = meta.block_number,
                    error = %e,
                    "Transform failed"
                );
                e
            })?;

        let mut trust = info_span!("transform.trust", action_count = actions.len())
            .in_scope(|| pipelines::trust::transform(actions, &meta))
            .map_err(|e| {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "transform.trust",
                    block_number = meta.block_number,
                    error = %e,
                    "Transform failed"
                );
                e
            })?;

        let mut moderation = info_span!("transform.moderation", action_count = actions.len())
            .in_scope(|| pipelines::moderation::transform(actions, &meta))
            .map_err(|e| {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "transform.moderation",
                    block_number = meta.block_number,
                    error = %e,
                    "Transform failed"
                );
                e
            })?;

        let mut topics = info_span!("transform.topics", action_count = actions.len())
            .in_scope(|| pipelines::topics::transform(actions, &meta))
            .map_err(|e| {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "transform.topics",
                    block_number = meta.block_number,
                    error = %e,
                    "Transform failed"
                );
                e
            })?;

        let mut governance = info_span!("transform.governance", action_count = actions.len())
            .in_scope(|| pipelines::governance::transform(actions, &meta, &prefetch_result.cache))
            .map_err(|e| {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "transform.governance",
                    block_number = meta.block_number,
                    error = %e,
                    "Transform failed"
                );
                e
            })?;

        let mut voting = info_span!("transform.voting", action_count = actions.len())
            .in_scope(|| pipelines::voting::transform(actions, &meta))
            .map_err(|e| {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "transform.voting",
                    block_number = meta.block_number,
                    error = %e,
                    "Transform failed"
                );
                e
            })?;

        // Edits transform is now sync - uses prefetched cache
        let mut edits = info_span!("transform.edits", action_count = actions.len())
            .in_scope(|| pipelines::edits::transform(actions, &meta, &prefetch_result.cache))
            .map_err(|e| {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "transform.edits",
                    block_number = meta.block_number,
                    error = %e,
                    "Transform failed"
                );
                e
            })?;

        // =========================================================================
        // Phase 1.5: Mark the last event in the block
        // =========================================================================
        // Find max sequence across all events and mark that event with is_last = true.
        // This allows consumers to know when they've received all events for a block.
        {
            use pipelines::{mark_sequence_as_last, max_sequence};

            let max_seq = [
                max_sequence(&spaces.events),
                max_sequence(&membership.roles_granted),
                max_sequence(&membership.roles_revoked),
                max_sequence(&membership.spaces_left),
                max_sequence(&trust.events),
                max_sequence(&moderation.editors_flagged),
                max_sequence(&moderation.editors_unflagged),
                max_sequence(&moderation.content_flagged),
                max_sequence(&moderation.content_unflagged),
                max_sequence(&topics.topics_declared),
                max_sequence(&topics.topics_removed),
                max_sequence(&governance.proposals_created),
                max_sequence(&governance.proposals_updated),
                max_sequence(&governance.proposals_voted),
                max_sequence(&governance.proposals_executed),
                max_sequence(&governance.proposals_settings_updated),
                max_sequence(&governance.voting_settings_updated),
                max_sequence(&voting.votes),
                max_sequence(&edits.events),
            ]
            .into_iter()
            .max()
            .unwrap_or(0);

            // Try to mark the event with max_seq as last (only one will match)
            let _ = mark_sequence_as_last(&mut spaces.events, max_seq)
                || mark_sequence_as_last(&mut membership.roles_granted, max_seq)
                || mark_sequence_as_last(&mut membership.roles_revoked, max_seq)
                || mark_sequence_as_last(&mut membership.spaces_left, max_seq)
                || mark_sequence_as_last(&mut trust.events, max_seq)
                || mark_sequence_as_last(&mut moderation.editors_flagged, max_seq)
                || mark_sequence_as_last(&mut moderation.editors_unflagged, max_seq)
                || mark_sequence_as_last(&mut moderation.content_flagged, max_seq)
                || mark_sequence_as_last(&mut moderation.content_unflagged, max_seq)
                || mark_sequence_as_last(&mut topics.topics_declared, max_seq)
                || mark_sequence_as_last(&mut topics.topics_removed, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_created, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_updated, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_voted, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_executed, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_settings_updated, max_seq)
                || mark_sequence_as_last(&mut governance.voting_settings_updated, max_seq)
                || mark_sequence_as_last(&mut voting.votes, max_seq)
                || mark_sequence_as_last(&mut edits.events, max_seq);
        }

        // =========================================================================
        // Phase 2: Emit events to Kafka in order
        // =========================================================================
        // Ordering matters here:
        // 1. Spaces must be emitted first since all other events reference spaces
        // 2. Membership events next (who can do what in spaces)
        // 3. Trust events define the space topology
        // 4. Moderation events (flagging)
        // 5. Topic declarations
        // 6. Governance events (proposals reference spaces)
        // 7. Voting events (social layer)
        // 8. Edits come last as they may reference entities across trusted spaces

        let space_count = spaces.events.len() as u64;
        let membership_count = membership.total() as u64;
        let trust_count = trust.total();
        let moderation_count = moderation.total() as u64;
        let topics_count = topics.total() as u64;
        let governance_count = governance.total() as u64;
        let voting_count = voting.total() as u64;
        let edit_count = edits.events.len() as u64;
        let total = space_count
            + membership_count
            + trust_count
            + moderation_count
            + topics_count
            + governance_count
            + voting_count
            + edit_count;

        let mut counts_by_topic: HashMap<String, u64> = HashMap::new();
        counts_by_topic.insert(topics::SPACE_CREATIONS.to_string(), space_count);
        counts_by_topic.insert(topics::MEMBERSHIP.to_string(), membership_count);
        counts_by_topic.insert(topics::TRUST_EXTENSIONS.to_string(), trust_count as u64);
        counts_by_topic.insert(topics::MODERATION.to_string(), moderation_count);
        counts_by_topic.insert(topics::TOPICS.to_string(), topics_count);
        counts_by_topic.insert(topics::GOVERNANCE.to_string(), governance_count);
        counts_by_topic.insert(topics::VOTING.to_string(), voting_count);
        counts_by_topic.insert(topics::EDITS.to_string(), edit_count);

        let mut counts_by_event_type: HashMap<String, u64> = HashMap::new();
        counts_by_event_type.insert("SPACE_REGISTERED".to_string(), space_count);
        counts_by_event_type.insert("EDITS_PUBLISHED".to_string(), edit_count);
        counts_by_event_type.insert(
            "ROLE_GRANTED".to_string(),
            membership.roles_granted.len() as u64,
        );
        counts_by_event_type.insert(
            "ROLE_REVOKED".to_string(),
            membership.roles_revoked.len() as u64,
        );
        counts_by_event_type.insert(
            "SPACE_LEFT".to_string(),
            membership.spaces_left.len() as u64,
        );
        counts_by_event_type.insert("TRUST_EXTENSION".to_string(), trust_count as u64);
        counts_by_event_type.insert("SUBSPACE_VERIFIED".to_string(), trust.verified as u64);
        counts_by_event_type.insert("SUBSPACE_RELATED".to_string(), trust.related as u64);
        counts_by_event_type.insert(
            "SUBSPACE_TOPIC_SET".to_string(),
            trust.topic_declared as u64,
        );
        counts_by_event_type.insert("SUBSPACE_UNVERIFIED".to_string(), trust.unverified as u64);
        counts_by_event_type.insert("SUBSPACE_UNRELATED".to_string(), trust.unrelated as u64);
        counts_by_event_type.insert(
            "SUBSPACE_TOPIC_UNSET".to_string(),
            trust.topic_removed as u64,
        );
        counts_by_event_type.insert(
            "EDITOR_FLAGGED".to_string(),
            moderation.editors_flagged.len() as u64,
        );
        counts_by_event_type.insert(
            "EDITOR_UNFLAGGED".to_string(),
            moderation.editors_unflagged.len() as u64,
        );
        counts_by_event_type.insert(
            "CONTENT_FLAGGED".to_string(),
            moderation.content_flagged.len() as u64,
        );
        counts_by_event_type.insert(
            "CONTENT_UNFLAGGED".to_string(),
            moderation.content_unflagged.len() as u64,
        );
        // Summary key matches the wire-format Kafka `event-type` header
        // (see emit.rs:491) — kept as TOPIC_DECLARED for kg-indexer's
        // EXPECTED_EVENT_TYPES completeness check (kg-indexer/src/main.rs:685).
        counts_by_event_type.insert(
            "TOPIC_DECLARED".to_string(),
            topics.topics_declared.len() as u64,
        );
        counts_by_event_type.insert(
            "TOPIC_REMOVED".to_string(),
            topics.topics_removed.len() as u64,
        );
        counts_by_event_type.insert(
            "PROPOSAL_CREATED".to_string(),
            governance.proposals_created.len() as u64,
        );
        counts_by_event_type.insert(
            "PROPOSAL_UPDATED".to_string(),
            governance.proposals_updated.len() as u64,
        );
        counts_by_event_type.insert(
            "PROPOSAL_VOTED".to_string(),
            governance.proposals_voted.len() as u64,
        );
        counts_by_event_type.insert(
            "PROPOSAL_EXECUTED".to_string(),
            governance.proposals_executed.len() as u64,
        );
        counts_by_event_type.insert(
            "PROPOSAL_SETTINGS_UPDATED".to_string(),
            governance.proposals_settings_updated.len() as u64,
        );
        counts_by_event_type.insert(
            "VOTING_SETTINGS_UPDATED".to_string(),
            governance.voting_settings_updated.len() as u64,
        );
        counts_by_event_type.insert("VOTE_CAST".to_string(), voting.votes.len() as u64);

        info!(
            event = "hermes_pipeline.batch_summary",
            block_number = meta.block_number,
            total_events = total,
            counts_by_topic = ?counts_by_topic,
            counts_by_event_type = ?counts_by_event_type,
            "Batch summary"
        );

        // Only emit and create spans if there's actual data
        if total > 0 {
            let emit_start = std::time::Instant::now();
            info!(
                event = "hermes_pipeline.emit_start",
                block_number = meta.block_number,
                total_events = total,
                counts_by_topic = ?counts_by_topic,
                counts_by_event_type = ?counts_by_event_type,
                "Emit start"
            );

            // 1. Emit spaces
            if !spaces.events.is_empty() {
                for event in &spaces.events {
                    self.emitter.emit(event).await?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        "Space registered"
                    );
                }
            }

            // 2. Emit membership events
            if membership.total() > 0 {
                for event in &membership.roles_granted {
                    self.emitter.emit(event).await?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        member_space_id = %hex::encode(&event.member_space_id),
                        "Role granted"
                    );
                }
                for event in &membership.roles_revoked {
                    self.emitter.emit(event).await?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        member_space_id = %hex::encode(&event.member_space_id),
                        "Role revoked"
                    );
                }
                for event in &membership.spaces_left {
                    self.emitter.emit(event).await?;
                    debug!(
                        member_id = %hex::encode(&event.member_id),
                        space_id = %hex::encode(&event.space_id),
                        "Space left"
                    );
                }
            }

            // 3. Emit trust events
            if trust.total() > 0 {
                for event in &trust.events {
                    self.emitter.emit(event).await?;
                    debug!(
                        source = %hex::encode(&event.source_space_id),
                        extension_type = get_extension_type(event),
                        "Trust event emitted"
                    );
                }
            }

            // 4. Emit moderation events
            if moderation.total() > 0 {
                for event in &moderation.editors_flagged {
                    self.emitter.emit(event).await?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        editor = %hex::encode(&event.editor_account),
                        "Editor flagged"
                    );
                }
                for event in &moderation.editors_unflagged {
                    self.emitter.emit(event).await?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        editor = %hex::encode(&event.editor_account),
                        "Editor unflagged"
                    );
                }
                for event in &moderation.content_flagged {
                    self.emitter.emit(event).await?;
                    debug!(
                        flagger_id = %hex::encode(&event.flagger_id),
                        target_space_id = %hex::encode(&event.target_space_id),
                        "Content flagged"
                    );
                }
                for event in &moderation.content_unflagged {
                    self.emitter.emit(event).await?;
                    debug!(
                        unflagger_id = %hex::encode(&event.unflagger_id),
                        target_space_id = %hex::encode(&event.target_space_id),
                        "Content unflagged"
                    );
                }
            }

            // 5. Emit topic declarations and removals.
            // Both vecs are individually sorted by sequence (transform iterates
            // actions in order). Merge-sort by sequence here so that declare/
            // remove pairs in the same block are emitted in chain order — this
            // matters because per-partition Kafka order determines the order
            // consumers apply the writes, and a remove emitted after a later
            // declare for the same space would leave the indexer in NULL state
            // when the chain ended in the declared state.
            if topics.total() > 0 {
                let mut declared_iter = topics.topics_declared.iter().peekable();
                let mut removed_iter = topics.topics_removed.iter().peekable();
                loop {
                    let next_declared_seq = declared_iter
                        .peek()
                        .and_then(|e| e.meta.as_ref())
                        .map(|m| m.sequence);
                    let next_removed_seq = removed_iter
                        .peek()
                        .and_then(|e| e.meta.as_ref())
                        .map(|m| m.sequence);
                    match (next_declared_seq, next_removed_seq) {
                        (Some(d), Some(r)) if d <= r => {
                            let event = declared_iter.next().unwrap();
                            self.emitter.emit(event).await?;
                            debug!(
                                space_id = %hex::encode(&event.space_id),
                                topic_id = %hex::encode(&event.topic_id),
                                "Topic declared"
                            );
                        }
                        (Some(_), Some(_)) => {
                            let event = removed_iter.next().unwrap();
                            self.emitter.emit(event).await?;
                            debug!(
                                space_id = %hex::encode(&event.space_id),
                                topic_id = %hex::encode(&event.topic_id),
                                "Topic removed"
                            );
                        }
                        (Some(_), None) => {
                            let event = declared_iter.next().unwrap();
                            self.emitter.emit(event).await?;
                            debug!(
                                space_id = %hex::encode(&event.space_id),
                                topic_id = %hex::encode(&event.topic_id),
                                "Topic declared"
                            );
                        }
                        (None, Some(_)) => {
                            let event = removed_iter.next().unwrap();
                            self.emitter.emit(event).await?;
                            debug!(
                                space_id = %hex::encode(&event.space_id),
                                topic_id = %hex::encode(&event.topic_id),
                                "Topic removed"
                            );
                        }
                        (None, None) => break,
                    }
                }
            }

            // 6. Emit governance events
            if governance.total() > 0 {
                for event in &governance.proposals_created {
                    self.emitter.emit(event).await?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal created"
                    );
                }
                for event in &governance.proposals_updated {
                    self.emitter.emit(event).await?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal updated"
                    );
                }
                for event in &governance.proposals_voted {
                    self.emitter.emit(event).await?;
                    debug!(
                        voter_id = %hex::encode(&event.voter_id),
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal voted"
                    );
                }
                for event in &governance.proposals_executed {
                    self.emitter.emit(event).await?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal executed"
                    );
                }
                for event in &governance.proposals_settings_updated {
                    self.emitter.emit(event).await?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal settings updated"
                    );
                }
                for event in &governance.voting_settings_updated {
                    self.emitter.emit(event).await?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        "Voting settings updated"
                    );
                }
            }

            // 7. Emit voting events
            if voting.total() > 0 {
                for event in &voting.votes {
                    self.emitter.emit(event).await?;
                    debug!(
                        voter_id = %hex::encode(&event.voter_id),
                        object_id = %hex::encode(&event.object_id),
                        direction = get_vote_direction(event),
                        "Vote cast"
                    );
                }
            }

            // 8. Emit edits
            if !edits.events.is_empty() {
                for event in &edits.events {
                    self.emitter.emit(event).await?;
                    let space_id_display = if event.space_id.len() == 16 {
                        uuid::Uuid::from_bytes(
                            event.space_id.as_slice().try_into().unwrap_or([0; 16]),
                        )
                        .to_string()
                    } else {
                        hex::encode(&event.space_id)
                    };
                    debug!(
                        name = %event.name,
                        space_id = %space_id_display,
                        payload_bytes = event.payload.len(),
                        "Edit published"
                    );
                }
            }

            let emit_duration_ms = emit_start.elapsed().as_millis();
            if sentry_enabled() {
                info!(
                    event = "hermes_pipeline.emit_end",
                    block_number = meta.block_number,
                    total_events = total,
                    counts_by_topic = ?counts_by_topic,
                    counts_by_event_type = ?counts_by_event_type,
                    "Emit end"
                );
            } else {
                info!(
                    event = "hermes_pipeline.emit_end",
                    block_number = meta.block_number,
                    total_events = total,
                    duration_ms = emit_duration_ms,
                    counts_by_topic = ?counts_by_topic,
                    counts_by_event_type = ?counts_by_event_type,
                    "Emit end"
                );
            }
        }

        // Log cache issues (from prefetch and edits transform)
        let total_cache_misses = prefetch_result.cache_misses + edits.cache_misses;
        let total_errored_entries = prefetch_result.errored_entries + edits.errored_entries;
        let total_fetch_failures = prefetch_result.fetch_failures;
        let total_oversized_edits = edits.oversized_events;

        if total_cache_misses > 0 {
            error!(
                block_number = meta.block_number,
                count = total_cache_misses,
                "Edits dropped: IPFS payload not found in cache (indexer pipeline issue)"
            );
        }
        if total_errored_entries > 0 {
            error!(
                block_number = meta.block_number,
                count = total_errored_entries,
                "Edits dropped: IPFS payload errored (invalid user content)"
            );
        }
        if total_fetch_failures > 0 {
            warn!(count = total_fetch_failures, "Cache fetch failures");
        }
        if total_oversized_edits > 0 {
            warn!(
                block_number = meta.block_number,
                count = total_oversized_edits,
                "Oversized edits exceeded Kafka max message size and were skipped"
            );
        }

        // Emit block summary for consumers
        let created_at = meta.timestamp.parse().unwrap_or(0);
        let summary = hermes_schema::pb::block_summary::HermesBlockSummary {
            block_number: meta.block_number,
            cursor: meta.cursor.clone(),
            created_at,
            total_events: total,
            counts_by_topic,
            counts_by_event_type,
        };
        self.emitter.emit(&summary).await?;

        // Block fully ack'd to Kafka. Stash the block's clock time so
        // `persist_cursor` can update the lag gauges only after the cursor
        // is durably persisted — keeping the gauges consistent with what a
        // restart would actually resume from. Mirrors the pattern in
        // hermes-ipfs-cache/src/lib.rs:340-346.
        self.last_block_timestamp
            .store(meta.timestamp.parse().unwrap_or(0), Ordering::Relaxed);

        // Log block summary
        if total > 0 || total_cache_misses > 0 || total_errored_entries > 0 {
            info!(
                spaces = space_count,
                membership = membership_count,
                trust_verified = trust.verified,
                trust_related = trust.related,
                trust_topic = trust.topic_declared,
                trust_unverified = trust.unverified,
                trust_unrelated = trust.unrelated,
                trust_topic_removed = trust.topic_removed,
                moderation = moderation_count,
                topics = topics_count,
                governance = governance_count,
                voting_up = voting.upvotes,
                voting_down = voting.downvotes,
                voting_unvote = voting.unvotes,
                edits = edit_count,
                oversized_edits = total_oversized_edits,
                cache_misses = total_cache_misses,
                errored_entries = total_errored_entries,
                fetch_failures = total_fetch_failures,
                drift = %utils::format_drift(&relay_meta),
                "Block processed"
            );
        }

        Ok(())
    }
}

impl Sink for Pipeline {
    type Error = PipelineError;

    async fn process_block_scoped_data(&self, data: &BlockScopedData) -> Result<(), Self::Error> {
        let output = utils::output(data);
        let relay_meta = utils::block_metadata(data);
        let meta: BlockMetadata = relay_meta.clone().into();

        let span = info_span!("process_block", block_number = meta.block_number);

        self.process_block_impl(output.value.as_slice(), relay_meta, meta)
            .instrument(span)
            .await
    }

    fn process_block_undo_signal(
        &self,
        undo_signal: &hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockUndoSignal,
    ) -> std::result::Result<(), Self::Error> {
        // For now, just log the undo signal.
        // In a production system, we would delete any data recorded after this block.
        //
        // The trait's `run_live` will still rewind the cursor (via
        // `persist_cursor(undo_signal.last_valid_cursor, ...)`) so substreams
        // replays the reorged blocks with their new canonical contents —
        // kg-indexer's event_id dedup makes the replay safe for net-new
        // state. Stale events from the orphaned chain remain in Kafka — a
        // known correctness gap acknowledged in the cursor-persistence
        // design.
        let last_valid_block = undo_signal
            .last_valid_block
            .as_ref()
            .map_or(0, |b| b.number);
        warn!(
            event = "hermes_pipeline.undo_signal",
            indexer_id = cursor::INDEXER_ID,
            last_valid_block,
            "Block undo signal received — cursor will rewind on persist"
        );
        Ok(())
    }

    async fn persist_cursor(&self, cursor: String, block: u64) -> Result<(), Self::Error> {
        // Log BEFORE the write so the cursor is recoverable from Axiom/Sentry
        // even if the DB write fails (the failure surfaces separately as
        // `event = "hermes_pipeline.persist_cursor_failed"` with the same
        // fields). Mirrors hermes-ipfs-cache/README.md:154-167.
        info!(
            event = "hermes_pipeline.batch_end",
            indexer_id = cursor::INDEXER_ID,
            block_number = block,
            cursor = %cursor,
            "Cursor persist"
        );
        match self.cursor_store.persist(&cursor, block).await {
            Ok(()) => {
                // Cursor durably persisted — update lag gauges. We always
                // update the block gauge (from the trait argument), but
                // only update the timestamp gauge if a normal block has
                // been processed this run; otherwise (e.g. an undo signal
                // arrives before any block) the timestamp would be zero
                // and corrupt the lag dashboard.
                hermes_instrumentation::metrics::set_latest_processed_block(block);
                let ts = self.last_block_timestamp.load(Ordering::Relaxed);
                if ts > 0 {
                    hermes_instrumentation::metrics::set_latest_processed_block_timestamp(ts);
                }
            }
            Err(e) => {
                // Persist failure is non-fatal: keep processing blocks and
                // retry on the next persist_cursor call (called per-block
                // by the Sink trait). The cursor in the batch_end log above
                // is enough to manually recover the row if it stays missing
                // — see hermes-ipfs-cache/README.md:154-167 for the
                // procedure; ours mirrors it under event="batch_end".
                // Lag gauges intentionally stay at the last *durably*
                // persisted block so they reflect resume-from state, not
                // in-memory state. Matches the hermes-ipfs-cache failure
                // policy (lib.rs:330-346) — halting here would multiply
                // the disruption on transient DB blips.
                error!(
                    event = "hermes_pipeline.persist_cursor_failed",
                    indexer_id = cursor::INDEXER_ID,
                    block_number = block,
                    cursor = %cursor,
                    error = %e,
                    "Failed to persist cursor — continuing; will retry on next block"
                );
            }
        }
        Ok(())
    }

    async fn load_persisted_cursor(&self) -> Result<Option<String>, Self::Error> {
        let cursor = self
            .cursor_store
            .load()
            .await
            .map_err(anyhow::Error::from)?;
        match &cursor {
            Some(c) => info!(
                event = "hermes_pipeline.resume",
                indexer_id = cursor::INDEXER_ID,
                cursor = %c,
                "Resuming from persisted cursor"
            ),
            None => info!(
                event = "hermes_pipeline.cold_start",
                indexer_id = cursor::INDEXER_ID,
                "No persisted cursor — starting from SUBSTREAMS_START_BLOCK"
            ),
        }
        Ok(cursor)
    }
}

/// Build telemetry configuration from environment variables.
///
/// Environment variables:
/// - `SENTRY_DSN` - Sentry DSN/ingest URL
/// - `SENTRY_TRACES_SAMPLE_RATE` - Sampling rate (0.0 - 1.0)
/// - `SENTRY_SEND_DEFAULT_PII` - Set to "true" to include PII
/// - `SENTRY_ENVIRONMENT` - Environment tag (e.g., "prod", "staging")
/// - `SENTRY_RELEASE` - Release name (e.g., "service@1.2.3")
/// - `SENTRY_DEBUG` - Set to "true" to also emit spans to stdout
///
/// If `SENTRY_DSN` is not set, falls back to Console backend.
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

    Config::new("hermes-pipeline", backend)
}

fn sentry_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env::var("SENTRY_DSN").ok().is_some())
}

fn main() -> anyhow::Result<()> {
    // Load .env file if present (ignored in production)
    dotenv::dotenv().ok();

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring crypto provider");

    // Initialize telemetry BEFORE tokio runtime starts.
    // Keep the guard alive until the end of main to ensure spans are flushed.
    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    // Create and run the tokio runtime manually (instead of #[tokio::main])
    // - new_multi_thread(): Uses a thread pool for parallel task execution,
    //   which is appropriate for I/O-bound services like this pipeline
    // - enable_all(): Enables both I/O and time drivers, required for
    //   network operations (Kafka, IPFS) and timeouts/delays
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    info!("Hermes Pipeline starting");

    // Install Prometheus /metrics listener. Default port 9464 lives in the
    // metrics module; override with METRICS_PORT for local runs.
    let metrics_port: Option<u16> = env::var("METRICS_PORT").ok().and_then(|s| s.parse().ok());
    hermes_instrumentation::metrics::install("hermes-pipeline", metrics_port)?;

    // Determine if we're using mock data
    let use_mock = env::var("USE_MOCK")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);

    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    info!(kafka_broker = %broker, use_mock = use_mock, "Configuration loaded");

    // Create Kafka producer and wrap in Emitter
    debug!("Connecting to Kafka broker");
    let producer = create_producer(&broker, "hermes-pipeline")?;
    let emitter = Emitter::new(producer);
    info!("Connected to Kafka broker");

    // Create the IPFS cache and cursor store. Both are mock for testing,
    // PostgreSQL for production — and they share the same DATABASE_URL.
    let (cache, cursor_store): (Arc<dyn IpfsCache>, Arc<dyn CursorStore>) = if use_mock {
        info!("Using mock IPFS cache and cursor store");
        (
            CacheSource::mock().into_cache().await?,
            Arc::new(MockCursorStore::new()),
        )
    } else {
        let database_url =
            env::var("DATABASE_URL").expect("DATABASE_URL must be set when USE_MOCK is not true");
        info!("Connecting to IPFS cache database");
        let cache = CacheSource::live(&database_url).into_cache().await?;
        let cursor_store = Arc::new(PostgresCursorStore::new(&database_url).await?);
        (cache, cursor_store)
    };
    info!("IPFS cache and cursor store initialized");

    // Create the pipeline
    let pipeline = Pipeline::new(emitter, cache, cursor_store);

    // Determine stream source: mock or live substreams
    let source = if use_mock {
        StreamSource::mock()
    } else {
        let endpoint = env::var("SUBSTREAMS_ENDPOINT")
            .unwrap_or_else(|_| "geotest.substreams.pinax.network:443".to_string());
        let start_block: i64 = env::var("SUBSTREAMS_START_BLOCK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(138000);
        let end_block: u64 = env::var("SUBSTREAMS_END_BLOCK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(u64::MAX);

        info!(
            endpoint = %endpoint,
            start_block = start_block,
            end_block = end_block,
            "Using live substreams source"
        );

        StreamSource::live(endpoint, HermesModule::Actions, start_block, end_block)
    };

    info!(
        module = %HermesModule::Actions,
        topics.spaces = topics::SPACE_CREATIONS,
        topics.membership = topics::MEMBERSHIP,
        topics.trust = topics::TRUST_EXTENSIONS,
        topics.moderation = topics::MODERATION,
        topics.topics = topics::TOPICS,
        topics.governance = topics::GOVERNANCE,
        topics.voting = topics::VOTING,
        topics.edits = topics::EDITS,
        retry_initial_ms = pipeline.retry_config.initial_delay_ms,
        retry_factor = pipeline.retry_config.factor,
        retry_max_secs = pipeline.retry_config.max_delay.as_secs(),
        retry_max_count = pipeline.retry_config.max_retries,
        "Starting pipeline"
    );

    // Run the pipeline
    pipeline.run(source).await?;

    // Flush all pending messages to Kafka before exiting
    info!("Flushing Kafka producer");
    pipeline.flush(std::time::Duration::from_secs(30));

    info!("Pipeline finished");

    Ok(())
}

#[cfg(test)]
mod sink_tests {
    //! Sink-level integration tests exercising the cursor hooks on a real
    //! `Pipeline` instance wired to a `MockCursorStore`. Validates the
    //! `Sink` trait override path end-to-end without needing Kafka or a
    //! database — `process_block_scoped_data` is bypassed here; we call
    //! the cursor hooks directly to verify they delegate to the store
    //! correctly and handle errors per the documented contract.
    //!
    //! `PostgresCursorStore` is covered separately in `cursor::tests`
    //! (run via `cargo test -p hermes-pipeline -- --ignored postgres_cursor`).
    use super::*;
    use async_trait::async_trait;
    use hermes_pipeline::cursor::CursorStoreError;

    async fn make_pipeline(cursor_store: Arc<dyn CursorStore>) -> Pipeline {
        // rdkafka's FutureProducer is lazy — construction doesn't connect,
        // so a bogus broker is fine; we never call send() in these tests.
        let producer = create_producer("localhost:1", "test-sink").expect("create producer");
        // Use Emitter::new_with_prefix to bypass the ENVIRONMENT lookup
        // inside Emitter::new — that path calls hermes_kafka::get_topic_prefix
        // which caches the prefix in a process-wide OnceLock, leaking state
        // between tests in an order-dependent way. Tests don't emit anything,
        // so the prefix value doesn't matter — empty is fine.
        let emitter = Emitter::new_with_prefix(producer, "");
        let cache = CacheSource::mock().into_cache().await.expect("mock cache");
        Pipeline::new(emitter, cache, cursor_store)
    }

    #[tokio::test]
    async fn cold_start_load_returns_none() {
        let store = Arc::new(MockCursorStore::new());
        let pipeline = make_pipeline(store).await;

        let loaded = pipeline
            .load_persisted_cursor()
            .await
            .expect("load_persisted_cursor");
        assert!(loaded.is_none(), "empty store must report cold start");
    }

    #[tokio::test]
    async fn persist_cursor_writes_to_store() {
        let store = Arc::new(MockCursorStore::new());
        let pipeline = make_pipeline(store.clone()).await;

        pipeline
            .persist_cursor("cursor_abc".to_string(), 12345)
            .await
            .expect("persist_cursor");

        let loaded = store.load().await.expect("store.load");
        assert_eq!(loaded, Some("cursor_abc".to_string()));
    }

    #[tokio::test]
    async fn load_after_persist_round_trips_via_sink_hook() {
        let store = Arc::new(MockCursorStore::new());
        let pipeline = make_pipeline(store).await;

        pipeline
            .persist_cursor("cursor_xyz".to_string(), 999)
            .await
            .expect("persist_cursor");

        // Same Pipeline reading the cursor back through the trait surface.
        let loaded = pipeline
            .load_persisted_cursor()
            .await
            .expect("load_persisted_cursor");
        assert_eq!(loaded, Some("cursor_xyz".to_string()));
    }

    #[tokio::test]
    async fn persist_cursor_overwrites_previous_value() {
        let store = Arc::new(MockCursorStore::new());
        let pipeline = make_pipeline(store.clone()).await;

        pipeline
            .persist_cursor("cursor_first".to_string(), 100)
            .await
            .expect("first persist");
        pipeline
            .persist_cursor("cursor_second".to_string(), 200)
            .await
            .expect("second persist");

        assert_eq!(
            store.load().await.expect("store.load"),
            Some("cursor_second".to_string())
        );
    }

    /// A `CursorStore` that always fails to persist — used to verify that
    /// the Sink-level override does not propagate the error and halt the
    /// stream loop. Matches the failure-policy contract documented on
    /// `Sink::persist_cursor` and mirrors `hermes-ipfs-cache`.
    struct FailingCursorStore;

    #[async_trait]
    impl CursorStore for FailingCursorStore {
        async fn load(&self) -> Result<Option<String>, CursorStoreError> {
            Ok(None)
        }

        async fn persist(&self, _cursor: &str, _block: u64) -> Result<(), CursorStoreError> {
            Err(CursorStoreError::Database(sqlx::Error::PoolClosed))
        }
    }

    #[tokio::test]
    async fn persist_failure_does_not_halt_pipeline() {
        let store: Arc<dyn CursorStore> = Arc::new(FailingCursorStore);
        let pipeline = make_pipeline(store).await;

        // Even though the store fails internally, the Sink-level hook must
        // return Ok so the trait's `run_live` loop keeps processing
        // blocks. The next block's persist_cursor call will retry the
        // write. See the comment on `impl Sink for Pipeline::persist_cursor`.
        let result = pipeline.persist_cursor("any_cursor".to_string(), 1).await;
        assert!(
            result.is_ok(),
            "persist_cursor must not halt the stream loop on store failure; got {result:?}"
        );
    }
}
