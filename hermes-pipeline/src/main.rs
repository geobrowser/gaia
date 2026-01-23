//! Hermes Pipeline
//!
//! Consumes space-related events from Amp and transforms them into
//! Hermes protobuf messages for publication to Kafka.
//!
//! ## Event Types Handled
//!
//! - `SPACE_REGISTERED` - new space registrations -> `space.creations` topic
//! - `SUBSPACE_VERIFIED/RELATED/TOPIC_DECLARED/REMOVED` - trust events -> `space.trust.extensions` topic
//! - `EDITOR/MEMBER_ADDED/REMOVED`, `SPACE_LEFT` - membership -> `space.membership` topic
//! - `EDITOR_FLAGGED/UNFLAGGED`, `FLAGGED/UNFLAGGED` - moderation -> `space.moderation` topic
//! - `TOPIC_DECLARED` - topic declarations -> `space.topics` topic
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
//! - `AMP_FLIGHT_URL` - Amp Flight SQL URL (default: http://localhost:1602)
//! - `AMP_DATASET` - Amp dataset (default: geo/actions)
//! - `AMP_START_BLOCK` - First block to consume (default: 82655)
//! - `AMP_END_BLOCK` - Last block to consume (optional)
//! - `AMP_ACTIONS_ADDRESS` - Actions contract address (default: SPACE_REGISTRY_ADDRESS_HEX)
//! - `AMP_RECONNECT_DELAY_SECS` - Delay before reconnecting (default: 2)
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

use hermes_instrumentation::{Instrument, debug, error, info, info_span, warn};
use prost::Message;
use std::sync::OnceLock;

use hermes_kafka::create_producer;
use hermes_relay::Actions;
use hermes_relay::stream::utils;

use hermes_pipeline::cache::{CacheSource, IpfsCache};
use hermes_pipeline::pipelines;
use hermes_pipeline::pipelines::BlockMetadata;
use hermes_pipeline::pipelines::edits::RetryConfig;
use hermes_pipeline::pipelines::trust::get_extension_type;
use hermes_pipeline::pipelines::voting::get_vote_direction;

use emit::{Emitter, topics};

mod amp_stream;

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
/// - `SUBSPACE_VERIFIED/RELATED/TOPIC_DECLARED/REMOVED` -> trust pipeline
/// - `EDITOR/MEMBER_ADDED/REMOVED`, `SPACE_LEFT` -> membership pipeline
/// - `EDITOR_FLAGGED/UNFLAGGED`, `FLAGGED/UNFLAGGED` -> moderation pipeline
/// - `TOPIC_DECLARED` -> topics pipeline
/// - `PROPOSAL_CREATED/VOTED/EXECUTED` -> governance pipeline
/// - `UPVOTED/DOWNVOTED/UNVOTED` -> voting pipeline
/// - `EDITS_PUBLISHED` -> edits pipeline (with IPFS cache lookup)
///
/// The edits pipeline is kicked off first (async) so IPFS fetching happens
/// in parallel with the sync transforms. All events are emitted to Kafka in order.
pub struct Pipeline {
    emitter: Emitter,
    cache: Arc<dyn IpfsCache>,
    retry_config: RetryConfig,
}

impl Pipeline {
    pub fn new(emitter: Emitter, cache: Arc<dyn IpfsCache>) -> Self {
        Self {
            emitter,
            cache,
            retry_config: RetryConfig::default(),
        }
    }

    /// Create a pipeline with custom retry configuration.
    #[allow(dead_code)]
    pub fn with_retry_config(
        emitter: Emitter,
        cache: Arc<dyn IpfsCache>,
        retry_config: RetryConfig,
    ) -> Self {
        Self {
            emitter,
            cache,
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
        // Phase 1: Transform actions into events
        // =========================================================================

        // Kick off edits transform FIRST - it has async IPFS fetching that can
        // happen in parallel with the sync transforms below
        let edits_future =
            pipelines::edits::transform(actions, &meta, &self.cache, &self.retry_config)
                .instrument(info_span!("transform.edits", action_count = actions.len()));

        // Sync transforms - fast, no I/O, run while edits fetches from IPFS
        let mut spaces = info_span!("transform.spaces", action_count = actions.len())
            .in_scope(|| pipelines::spaces::transform(actions, &meta))
            .map_err(|e| {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "transform.spaces",
                    block_number = meta.block_number,
                    cursor = %meta.cursor,
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
                    cursor = %meta.cursor,
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
                    cursor = %meta.cursor,
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
                    cursor = %meta.cursor,
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
                    cursor = %meta.cursor,
                    error = %e,
                    "Transform failed"
                );
                e
            })?;

        let mut governance = info_span!("transform.governance", action_count = actions.len())
            .in_scope(|| pipelines::governance::transform(actions, &meta))
            .map_err(|e| {
                error!(
                    event = "hermes_pipeline.event_error",
                    stage = "transform.governance",
                    block_number = meta.block_number,
                    cursor = %meta.cursor,
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
                    cursor = %meta.cursor,
                    error = %e,
                    "Transform failed"
                );
                e
            })?;

        // Now await the edits - IPFS fetch should have been happening in parallel
        let mut edits = edits_future.await.map_err(|e| {
            error!(
                event = "hermes_pipeline.event_error",
                stage = "transform.edits",
                block_number = meta.block_number,
                cursor = %meta.cursor,
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
                max_sequence(&governance.proposals_created),
                max_sequence(&governance.proposals_updated),
                max_sequence(&governance.proposals_voted),
                max_sequence(&governance.proposals_executed),
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
                || mark_sequence_as_last(&mut governance.proposals_created, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_updated, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_voted, max_seq)
                || mark_sequence_as_last(&mut governance.proposals_executed, max_seq)
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
            "SUBSPACE_TOPIC_DECLARED".to_string(),
            trust.topic_declared as u64,
        );
        counts_by_event_type.insert("SUBSPACE_REMOVED".to_string(), trust.removed as u64);
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
        counts_by_event_type.insert(
            "TOPIC_DECLARED".to_string(),
            topics.topics_declared.len() as u64,
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
        counts_by_event_type.insert("VOTE_CAST".to_string(), voting.votes.len() as u64);

        info!(
            event = "hermes_pipeline.batch_summary",
            block_number = meta.block_number,
            cursor = %meta.cursor,
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
                cursor = %meta.cursor,
                total_events = total,
                counts_by_topic = ?counts_by_topic,
                counts_by_event_type = ?counts_by_event_type,
                "Emit start"
            );

            let _emit_span = info_span!("emit", total_events = total).entered();

            // 1. Emit spaces
            if !spaces.events.is_empty() {
                let _span = info_span!("emit.spaces", count = spaces.events.len()).entered();
                for event in &spaces.events {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        "Space registered"
                    );
                }
            }

            // 2. Emit membership events
            if membership.total() > 0 {
                let _span = info_span!("emit.membership", count = membership.total()).entered();
                for event in &membership.roles_granted {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        member_space_id = %hex::encode(&event.member_space_id),
                        "Role granted"
                    );
                }
                for event in &membership.roles_revoked {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        member_space_id = %hex::encode(&event.member_space_id),
                        "Role revoked"
                    );
                }
                for event in &membership.spaces_left {
                    self.emitter.emit(event)?;
                    debug!(
                        member_id = %hex::encode(&event.member_id),
                        space_id = %hex::encode(&event.space_id),
                        "Space left"
                    );
                }
            }

            // 3. Emit trust events
            if trust.total() > 0 {
                let _span = info_span!("emit.trust", count = trust.total()).entered();
                for trust_event in &trust.events {
                    self.emitter.emit(&trust_event.event)?;
                    debug!(
                        source = %hex::encode(&trust_event.event.source_space_id),
                        extension_type = get_extension_type(&trust_event.event),
                        is_removal = trust_event.is_removal,
                        "Trust event emitted"
                    );
                }
            }

            // 4. Emit moderation events
            if moderation.total() > 0 {
                let _span = info_span!("emit.moderation", count = moderation.total()).entered();
                for event in &moderation.editors_flagged {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        editor = %hex::encode(&event.editor_account),
                        "Editor flagged"
                    );
                }
                for event in &moderation.editors_unflagged {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        editor = %hex::encode(&event.editor_account),
                        "Editor unflagged"
                    );
                }
                for event in &moderation.content_flagged {
                    self.emitter.emit(event)?;
                    debug!(
                        flagger_id = %hex::encode(&event.flagger_id),
                        target_space_id = %hex::encode(&event.target_space_id),
                        "Content flagged"
                    );
                }
                for event in &moderation.content_unflagged {
                    self.emitter.emit(event)?;
                    debug!(
                        unflagger_id = %hex::encode(&event.unflagger_id),
                        target_space_id = %hex::encode(&event.target_space_id),
                        "Content unflagged"
                    );
                }
            }

            // 5. Emit topic declarations
            if topics.total() > 0 {
                let _span = info_span!("emit.topics", count = topics.total()).entered();
                for event in &topics.topics_declared {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        topic_id = %hex::encode(&event.topic_id),
                        "Topic declared"
                    );
                }
            }

            // 6. Emit governance events
            if governance.total() > 0 {
                let _span = info_span!("emit.governance", count = governance.total()).entered();
                for event in &governance.proposals_created {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal created"
                    );
                }
                for event in &governance.proposals_updated {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal updated"
                    );
                }
                for event in &governance.proposals_voted {
                    self.emitter.emit(event)?;
                    debug!(
                        voter_id = %hex::encode(&event.voter_id),
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal voted"
                    );
                }
                for event in &governance.proposals_executed {
                    self.emitter.emit(event)?;
                    debug!(
                        space_id = %hex::encode(&event.space_id),
                        proposal_id = %hex::encode(&event.proposal_id),
                        "Proposal executed"
                    );
                }
            }

            // 7. Emit voting events
            if voting.total() > 0 {
                let _span = info_span!("emit.voting", count = voting.total()).entered();
                for event in &voting.votes {
                    self.emitter.emit(event)?;
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
                let _span = info_span!("emit.edits", count = edits.events.len()).entered();
                for event in &edits.events {
                    self.emitter.emit(event)?;
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
                    cursor = %meta.cursor,
                    total_events = total,
                    counts_by_topic = ?counts_by_topic,
                    counts_by_event_type = ?counts_by_event_type,
                    "Emit end"
                );
            } else {
                info!(
                    event = "hermes_pipeline.emit_end",
                    block_number = meta.block_number,
                    cursor = %meta.cursor,
                    total_events = total,
                    duration_ms = emit_duration_ms,
                    counts_by_topic = ?counts_by_topic,
                    counts_by_event_type = ?counts_by_event_type,
                    "Emit end"
                );
            }
        }

        // Log cache issues
        if edits.cache_misses > 0 {
            warn!(
                count = edits.cache_misses,
                "Edit cache misses (retries exhausted)"
            );
        }
        if edits.errored_entries > 0 {
            warn!(
                count = edits.errored_entries,
                "Edit entries errored in cache"
            );
        }
        if edits.fetch_failures > 0 {
            warn!(count = edits.fetch_failures, "Edit fetch failures");
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
        self.emitter.emit(&summary)?;

        // Log block summary
        if total > 0 || edits.cache_misses > 0 || edits.errored_entries > 0 {
            info!(
                spaces = space_count,
                membership = membership_count,
                trust_verified = trust.verified,
                trust_related = trust.related,
                trust_topic = trust.topic_declared,
                trust_removed = trust.removed,
                moderation = moderation_count,
                topics = topics_count,
                governance = governance_count,
                voting_up = voting.upvotes,
                voting_down = voting.downvotes,
                voting_unvote = voting.unvotes,
                edits = edit_count,
                cache_misses = edits.cache_misses,
                errored_entries = edits.errored_entries,
                fetch_failures = edits.fetch_failures,
                drift = %utils::format_drift(&relay_meta),
                "Block processed"
            );
        }

        Ok(())
    }

    pub(crate) async fn process_actions_block(
        &self,
        actions: Actions,
        block_number: u64,
        timestamp_secs: u64,
        cursor: String,
    ) -> Result<(), PipelineError> {
        let relay_meta = utils::BlockMetadata {
            cursor,
            block_number,
            timestamp: timestamp_secs.to_string(),
        };
        let meta: BlockMetadata = relay_meta.clone().into();
        let encoded = actions.encode_to_vec();
        self.process_block_impl(encoded.as_slice(), relay_meta, meta)
            .await
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

    // Create the IPFS cache: mock for testing, PostgreSQL for production
    let cache = if use_mock {
        info!("Using mock IPFS cache");
        CacheSource::mock().into_cache().await?
    } else {
        let database_url =
            env::var("DATABASE_URL").expect("DATABASE_URL must be set when USE_MOCK is not true");
        info!("Connecting to IPFS cache database");
        CacheSource::live(&database_url).into_cache().await?
    };
    info!("IPFS cache initialized");

    // Create the pipeline
    let pipeline = Pipeline::new(emitter, cache);

    info!(
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

    if use_mock {
        info!("Using mock actions");
        let actions = hermes_relay::source::mock_events::test_topology::generate();
        let actions = Actions { actions };
        pipeline
            .process_actions_block(actions, 0, 0, "mock:0".to_string())
            .await?;
    } else {
        info!("Using Amp Flight stream source");
        amp_stream::run_amp_stream(&pipeline).await?;
    }

    // Flush all pending messages to Kafka before exiting
    info!("Flushing Kafka producer");
    pipeline.flush(std::time::Duration::from_secs(30));

    info!("Pipeline finished");

    Ok(())
}
