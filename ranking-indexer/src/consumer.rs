//! Kafka consumer for the `knowledge.edits` and `space.membership` topics.
//!
//! Mirrors the vote-indexer consumer: a manual-commit `StreamConsumer` with the
//! environment topic prefix applied. Edits are decoded in two steps —
//! `HermesEdit` (prost) then the raw GRC2/GRC2Z payload via the `grc-20` crate,
//! the same decoder the kg-indexer uses. Membership messages are dispatched on
//! the `event-type` Kafka header, same as the kg-indexer.

use hermes_kafka::get_topic_prefix;
use prost::Message;
use rdkafka::{
    client::ClientContext,
    config::ClientConfig,
    consumer::{BaseConsumer, Consumer, ConsumerContext, RebalanceProtocol, StreamConsumer},
    error::RDKafkaErrorCode,
    types::RDKafkaRespErr,
    Offset, TopicPartitionList,
};
use std::env;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::error::IndexerError;
use crate::membership::MembershipEvent;

/// Base topic for knowledge edits.
const EDITS_TOPIC: &str = "knowledge.edits";

/// Base topic for membership events (role granted/revoked, space left).
const MEMBERSHIP_TOPIC: &str = "space.membership";

/// Which subscribed topic a message arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicKind {
    Edits,
    Membership,
}

/// Consumer context that starts *fresh* membership partitions at the latest
/// offset instead of replaying the topic's history.
///
/// The membership view (`ranks.members` / `ranks.editors`) is seeded by
/// migration 0062 from the kg-indexer-maintained public tables, so on the
/// first deploy — when the group has committed offsets for `knowledge.edits`
/// but none for `space.membership` — `auto.offset.reset=earliest` would
/// replay the membership topic's entire retained history, recomputing and
/// *republishing* aggregates through every historical membership state along
/// the way. Starting at the high watermark is correct because the seed
/// already reflects everything up to migration time; events produced after
/// the consumer joins are consumed normally. Edits partitions are untouched.
///
/// Caveat: if the group's committed offsets expire (group inactive longer
/// than the broker's `offsets.retention.minutes`), membership resumes at
/// latest and the outage window's events are skipped — re-run the 0062 seed
/// inserts to reconcile the view before restarting.
pub struct RankingConsumerContext {
    membership_topic: String,
}

impl RankingConsumerContext {
    /// For each membership partition in the assignment with no committed
    /// offset, override the start position to the high watermark. Leaves
    /// every other partition untouched (committed offset, or
    /// `auto.offset.reset` if none). On lookup failure the assignment is left
    /// as-is, falling back to `earliest` — a full replay converges (it only
    /// flaps published aggregates), whereas wrongly skipping ahead would
    /// silently lose events.
    fn start_fresh_membership_partitions_at_latest(
        &self,
        consumer: &BaseConsumer<Self>,
        assignment: &mut TopicPartitionList,
    ) {
        let partitions: Vec<i32> = assignment
            .elements_for_topic(&self.membership_topic)
            .iter()
            .map(|e| e.partition())
            .collect();
        if partitions.is_empty() {
            return;
        }

        let mut query = TopicPartitionList::new();
        for partition in &partitions {
            query.add_partition(&self.membership_topic, *partition);
        }
        let committed = match consumer.committed_offsets(query, Duration::from_secs(10)) {
            Ok(committed) => committed,
            Err(e) => {
                error!(
                    error = %e,
                    topic = %self.membership_topic,
                    "Failed to fetch committed membership offsets — falling back to \
                     auto.offset.reset (full replay)"
                );
                return;
            }
        };

        for elem in committed.elements() {
            if elem.offset() != Offset::Invalid {
                continue;
            }
            if let Some(mut assigned) =
                assignment.find_partition(&self.membership_topic, elem.partition())
            {
                if let Err(e) = assigned.set_offset(Offset::End) {
                    error!(error = %e, partition = elem.partition(), "Failed to set start offset");
                    continue;
                }
                info!(
                    topic = %self.membership_topic,
                    partition = elem.partition(),
                    "No committed offset for membership partition — starting at latest \
                     (view is seeded by migration 0062)"
                );
            }
        }
    }
}

impl ClientContext for RankingConsumerContext {}

impl ConsumerContext for RankingConsumerContext {
    // Full override of the default rebalance flow (the default body can't be
    // called from an override): identical assign/unassign behavior via the
    // public API, plus the fresh-membership-partition offset override before
    // partitions are assigned.
    fn rebalance(
        &self,
        base_consumer: &BaseConsumer<Self>,
        err: RDKafkaRespErr,
        tpl: &mut TopicPartitionList,
    ) {
        match err {
            RDKafkaRespErr::RD_KAFKA_RESP_ERR__ASSIGN_PARTITIONS => {
                self.start_fresh_membership_partitions_at_latest(base_consumer, tpl);
                let result = match base_consumer.rebalance_protocol() {
                    RebalanceProtocol::Cooperative => base_consumer.incremental_assign(tpl),
                    _ => base_consumer.assign(tpl),
                };
                if let Err(e) = result {
                    error!(error = %e, "Failed to assign partitions during rebalance");
                }
            }
            RDKafkaRespErr::RD_KAFKA_RESP_ERR__REVOKE_PARTITIONS => {
                let result = match base_consumer.rebalance_protocol() {
                    RebalanceProtocol::Cooperative => base_consumer.incremental_unassign(tpl),
                    _ => base_consumer.unassign(),
                };
                if let Err(e) = result {
                    error!(error = %e, "Failed to unassign partitions during rebalance");
                }
            }
            _ => {
                let code: RDKafkaErrorCode = err.into();
                error!(error = %code, "Kafka rebalance error");
                if let Err(e) = base_consumer.unassign() {
                    warn!(error = %e, "Failed to unassign partitions after rebalance error");
                }
            }
        }
    }
}

/// Kafka consumer for knowledge edits + membership events.
pub struct KafkaConsumer {
    consumer: StreamConsumer<RankingConsumerContext>,
    edits_topic: String,
    membership_topic: String,
}

impl KafkaConsumer {
    pub fn new(brokers: &str, group_id: &str) -> Result<Self, IndexerError> {
        let mut config = ClientConfig::new();
        config
            .set("bootstrap.servers", brokers)
            .set("group.id", group_id)
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .set("session.timeout.ms", "6000");

        if let Ok(username) = env::var("KAFKA_USERNAME") {
            config.set("security.protocol", "SASL_SSL");
            config.set("sasl.mechanisms", "PLAIN");
            config.set("sasl.username", &username);

            let password = env::var("KAFKA_PASSWORD").map_err(|_| {
                IndexerError::Config(
                    "KAFKA_PASSWORD must be set when KAFKA_USERNAME is configured".into(),
                )
            })?;
            config.set("sasl.password", &password);
        }

        if let Ok(ca_pem) = env::var("KAFKA_SSL_CA_PEM") {
            config.set("ssl.ca.pem", &ca_pem);
        }

        let prefix = get_topic_prefix();
        let edits_topic = format!("{}{}", prefix, EDITS_TOPIC);
        let membership_topic = format!("{}{}", prefix, MEMBERSHIP_TOPIC);

        let context = RankingConsumerContext {
            membership_topic: membership_topic.clone(),
        };
        let consumer: StreamConsumer<RankingConsumerContext> =
            config.create_with_context(context)?;

        info!(
            brokers = %brokers,
            group_id = %group_id,
            topic_prefix = %prefix,
            edits_topic = %edits_topic,
            membership_topic = %membership_topic,
            "Created Kafka consumer"
        );

        Ok(Self {
            consumer,
            edits_topic,
            membership_topic,
        })
    }

    pub fn subscribe(&self) -> Result<(), IndexerError> {
        self.consumer
            .subscribe(&[&self.edits_topic, &self.membership_topic])?;
        info!(
            edits_topic = %self.edits_topic,
            membership_topic = %self.membership_topic,
            "Subscribed to Kafka topics"
        );
        Ok(())
    }

    /// Which of the subscribed topics a message's topic is.
    pub fn topic_kind(&self, topic: &str) -> Option<TopicKind> {
        if topic == self.edits_topic {
            Some(TopicKind::Edits)
        } else if topic == self.membership_topic {
            Some(TopicKind::Membership)
        } else {
            None
        }
    }

    pub fn stream(&self) -> rdkafka::consumer::MessageStream<'_, RankingConsumerContext> {
        self.consumer.stream()
    }

    pub fn commit_message(
        &self,
        topic: &str,
        partition: i32,
        offset: i64,
    ) -> Result<(), IndexerError> {
        let mut tpl = TopicPartitionList::new();
        tpl.add_partition_offset(topic, partition, rdkafka::Offset::Offset(offset + 1))?;
        self.consumer
            .commit(&tpl, rdkafka::consumer::CommitMode::Async)?;
        debug!(topic = %topic, partition = partition, offset = offset, "Committed offset");
        Ok(())
    }
}

/// Decode the `HermesEdit` envelope from a Kafka payload.
pub fn parse_edit(
    payload: &[u8],
) -> Result<hermes_schema::pb::knowledge::HermesEdit, IndexerError> {
    Ok(hermes_schema::pb::knowledge::HermesEdit::decode(payload)?)
}

/// Decode the raw GRC2/GRC2Z payload bytes into a `grc_20::Edit` (handles both
/// compressed and uncompressed forms, same as the kg-indexer).
pub fn decode_grc20(payload: &[u8]) -> Result<grc_20::Edit<'_>, IndexerError> {
    grc_20::decode_edit(payload).map_err(|e| IndexerError::decode(format!("grc20: {e}")))
}

/// The `event-type` Kafka header, which selects the protobuf message type on
/// the membership topic (same convention as the kg-indexer).
pub fn get_event_type(headers: Option<&rdkafka::message::BorrowedHeaders>) -> Option<String> {
    use rdkafka::message::Headers;
    headers.and_then(|h| {
        for header in h.iter() {
            if header.key == "event-type" {
                if let Some(value) = header.value {
                    return String::from_utf8(value.to_vec()).ok();
                }
            }
        }
        None
    })
}

/// Decode a `space.membership` message, dispatching on the `event-type` header.
///
/// `Ok(None)` means a known event type this indexer intentionally ignores —
/// distinguished from unknown types (an error) so an expected event never
/// logs at warn level.
pub fn parse_membership_event(
    payload: &[u8],
    event_type: Option<&str>,
) -> Result<Option<MembershipEvent>, IndexerError> {
    match event_type {
        Some("ROLE_GRANTED") => Ok(Some(MembershipEvent::RoleGranted(
            hermes_schema::pb::membership::HermesRoleGranted::decode(payload)
                .map_err(|e| IndexerError::decode(format!("HermesRoleGranted: {e}")))?,
        ))),
        Some("ROLE_REVOKED") => Ok(Some(MembershipEvent::RoleRevoked(
            hermes_schema::pb::membership::HermesRoleRevoked::decode(payload)
                .map_err(|e| IndexerError::decode(format!("HermesRoleRevoked: {e}")))?,
        ))),
        // Emitted by the pipeline but intentionally unhandled, for parity with
        // the kg-indexer (which also ignores it).
        Some("SPACE_LEFT") => Ok(None),
        other => Err(IndexerError::decode(format!(
            "unknown membership event type: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_schema::pb::membership::{HermesRoleGranted, MembershipRole};
    use uuid::Uuid;

    #[test]
    fn membership_dispatches_on_event_type_header() {
        let granted = HermesRoleGranted {
            space_id: Uuid::from_u128(1).as_bytes().to_vec(),
            member_space_id: Uuid::from_u128(2).as_bytes().to_vec(),
            role: MembershipRole::Member as i32,
            meta: None,
        };
        let payload = granted.encode_to_vec();

        assert!(matches!(
            parse_membership_event(&payload, Some("ROLE_GRANTED")),
            Ok(Some(MembershipEvent::RoleGranted(_)))
        ));
        assert!(matches!(
            parse_membership_event(&payload, Some("ROLE_REVOKED")),
            Ok(Some(MembershipEvent::RoleRevoked(_)))
        ));
        // known-but-unhandled event type -> ignored, not an error
        assert!(matches!(
            parse_membership_event(&payload, Some("SPACE_LEFT")),
            Ok(None)
        ));
        // missing or unknown header -> decode error, not a crash
        assert!(parse_membership_event(&payload, None).is_err());
        assert!(parse_membership_event(&payload, Some("BOGUS")).is_err());
    }
}
