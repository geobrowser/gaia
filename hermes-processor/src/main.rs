//! Hermes Processor
//!
//! Consumes events from mock-substream and transforms them into Hermes protobuf
//! messages, then publishes to Kafka topics.

use prost::Message;
use rdkafka::config::ClientConfig;
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{BaseProducer, BaseRecord, Producer};
use std::env;
use std::time::Duration;

use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::knowledge::HermesEdit;
use hermes_schema::pb::space::{
    DefaultDaoSpacePayload, HermesCreateSpace, HermesSpaceTrustExtension, PersonalSpacePayload,
    RelatedExtension, SubtopicExtension, VerifiedExtension,
};
use wire::pb::grc20::{DataType as WireDataType, Entity, Op, Property, Relation, Value};

use mock_substream::{
    test_topology, BlockMetadata, EditPublished, MockEvent, SpaceCreated, SpaceType,
    TrustExtended, TrustExtension,
};

// =============================================================================
// Conversion: mock-substream -> Hermes protos
// =============================================================================

fn convert_block_metadata(meta: &BlockMetadata) -> BlockchainMetadata {
    BlockchainMetadata {
        created_at: meta.block_timestamp,
        created_by: vec![], // Not available in mock metadata
        block_number: meta.block_number,
        cursor: meta.cursor.clone(),
    }
}

fn convert_space_created(event: &SpaceCreated) -> HermesCreateSpace {
    let payload = match &event.space_type {
        SpaceType::Personal { owner } => {
            Some(hermes_schema::pb::space::hermes_create_space::Payload::PersonalSpace(
                PersonalSpacePayload {
                    owner: owner.to_vec(),
                },
            ))
        }
        SpaceType::Dao {
            initial_editors,
            initial_members,
        } => {
            Some(hermes_schema::pb::space::hermes_create_space::Payload::DefaultDaoSpace(
                DefaultDaoSpacePayload {
                    initial_editors: initial_editors.iter().map(|id| id.to_vec()).collect(),
                    initial_members: initial_members.iter().map(|id| id.to_vec()).collect(),
                },
            ))
        }
    };

    HermesCreateSpace {
        space_id: event.space_id.to_vec(),
        topic_id: event.topic_id.to_vec(),
        payload,
        meta: Some(convert_block_metadata(&event.meta)),
    }
}

fn convert_trust_extended(event: &TrustExtended) -> HermesSpaceTrustExtension {
    let extension = match &event.extension {
        TrustExtension::Verified { target_space_id } => {
            Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Verified(
                VerifiedExtension {
                    target_space_id: target_space_id.to_vec(),
                },
            ))
        }
        TrustExtension::Related { target_space_id } => {
            Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Related(
                RelatedExtension {
                    target_space_id: target_space_id.to_vec(),
                },
            ))
        }
        TrustExtension::Subtopic { target_topic_id } => {
            Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Subtopic(
                SubtopicExtension {
                    target_topic_id: target_topic_id.to_vec(),
                },
            ))
        }
    };

    HermesSpaceTrustExtension {
        source_space_id: event.source_space_id.to_vec(),
        extension,
        meta: Some(convert_block_metadata(&event.meta)),
    }
}

fn convert_op(op: &mock_substream::Op) -> Op {
    match op {
        mock_substream::Op::UpdateEntity(update) => Op {
            payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                id: update.id.to_vec(),
                values: update
                    .values
                    .iter()
                    .map(|v| Value {
                        property: v.property.to_vec(),
                        value: v.value.clone(),
                        options: None,
                    })
                    .collect(),
            })),
        },
        mock_substream::Op::CreateRelation(rel) => Op {
            payload: Some(wire::pb::grc20::op::Payload::CreateRelation(Relation {
                id: rel.id.to_vec(),
                r#type: rel.relation_type.to_vec(),
                from_entity: rel.from_entity.to_vec(),
                from_space: rel.from_space.map(|s| s.to_vec()),
                from_version: None,
                to_entity: rel.to_entity.to_vec(),
                to_space: rel.to_space.map(|s| s.to_vec()),
                to_version: None,
                entity: rel.entity.to_vec(),
                position: rel.position.clone(),
                verified: rel.verified,
            })),
        },
        mock_substream::Op::CreateProperty(prop) => Op {
            payload: Some(wire::pb::grc20::op::Payload::CreateProperty(Property {
                id: prop.id.to_vec(),
                data_type: match prop.data_type {
                    mock_substream::DataType::String => WireDataType::String as i32,
                    mock_substream::DataType::Number => WireDataType::Number as i32,
                    mock_substream::DataType::Boolean => WireDataType::Boolean as i32,
                    mock_substream::DataType::Time => WireDataType::Time as i32,
                    mock_substream::DataType::Point => WireDataType::Point as i32,
                    mock_substream::DataType::Relation => WireDataType::Relation as i32,
                },
            })),
        },
        mock_substream::Op::UpdateRelation(update) => Op {
            payload: Some(wire::pb::grc20::op::Payload::UpdateRelation(
                wire::pb::grc20::RelationUpdate {
                    id: update.id.to_vec(),
                    from_space: update.from_space.map(|s| s.to_vec()),
                    from_version: None,
                    to_space: update.to_space.map(|s| s.to_vec()),
                    to_version: None,
                    position: update.position.clone(),
                    verified: update.verified,
                },
            )),
        },
        mock_substream::Op::DeleteRelation(id) => Op {
            payload: Some(wire::pb::grc20::op::Payload::DeleteRelation(id.to_vec())),
        },
        mock_substream::Op::UnsetEntityValues(unset) => Op {
            payload: Some(wire::pb::grc20::op::Payload::UnsetEntityValues(
                wire::pb::grc20::UnsetEntityValues {
                    id: unset.id.to_vec(),
                    properties: unset.properties.iter().map(|p| p.to_vec()).collect(),
                },
            )),
        },
        mock_substream::Op::UnsetRelationFields(unset) => Op {
            payload: Some(wire::pb::grc20::op::Payload::UnsetRelationFields(
                wire::pb::grc20::UnsetRelationFields {
                    id: unset.id.to_vec(),
                    from_space: unset.from_space,
                    from_version: None,
                    to_space: unset.to_space,
                    to_version: None,
                    position: unset.position,
                    verified: unset.verified,
                },
            )),
        },
    }
}

fn convert_edit_published(event: &EditPublished) -> HermesEdit {
    HermesEdit {
        id: event.edit_id.to_vec(),
        name: event.name.clone(),
        ops: event.ops.iter().map(convert_op).collect(),
        authors: event.authors.iter().map(|a| a.to_vec()).collect(),
        language: None,
        space_id: hex::encode(event.space_id),
        is_canonical: true, // Canonicality is determined by Atlas, default to true
        meta: Some(convert_block_metadata(&event.meta)),
    }
}

// =============================================================================
// Kafka producers
// =============================================================================

fn create_producer(broker: &str) -> Result<BaseProducer, Box<dyn std::error::Error>> {
    let mut config = ClientConfig::new();

    config
        .set("bootstrap.servers", broker)
        .set("client.id", "hermes-processor")
        .set("compression.type", "zstd")
        .set("message.timeout.ms", "5000")
        .set("queue.buffering.max.messages", "100000")
        .set("queue.buffering.max.kbytes", "1048576")
        .set("batch.num.messages", "10000");

    // If SASL credentials are provided, enable SASL/SSL (for managed Kafka)
    // Otherwise, use plaintext (for local development)
    if let (Ok(username), Ok(password)) = (
        env::var("KAFKA_USERNAME"),
        env::var("KAFKA_PASSWORD"),
    ) {
        config
            .set("security.protocol", "SASL_SSL")
            .set("sasl.mechanisms", "PLAIN")
            .set("sasl.username", &username)
            .set("sasl.password", &password);

        // Use custom CA certificate if provided (PEM format string)
        if let Ok(ca_pem) = env::var("KAFKA_SSL_CA_PEM") {
            config.set("ssl.ca.pem", &ca_pem);
        }
    }

    Ok(config.create()?)
}

fn send_space(
    producer: &BaseProducer,
    space: &HermesCreateSpace,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = Vec::new();
    space.encode(&mut payload)?;

    let space_type = match &space.payload {
        Some(hermes_schema::pb::space::hermes_create_space::Payload::PersonalSpace(_)) => {
            "PERSONAL"
        }
        Some(hermes_schema::pb::space::hermes_create_space::Payload::DefaultDaoSpace(_)) => {
            "DEFAULT_DAO"
        }
        None => "UNKNOWN",
    };

    let record = BaseRecord::to("space.creations")
        .key(&space.space_id)
        .payload(&payload)
        .headers(OwnedHeaders::new().insert(Header {
            key: "space-type",
            value: Some(space_type),
        }));

    producer.send(record).map_err(|(e, _)| e)?;
    Ok(())
}

fn send_trust_extension(
    producer: &BaseProducer,
    trust_extension: &HermesSpaceTrustExtension,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = Vec::new();
    trust_extension.encode(&mut payload)?;

    let extension_type = match &trust_extension.extension {
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Verified(_)) => {
            "VERIFIED"
        }
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Related(_)) => {
            "RELATED"
        }
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Subtopic(_)) => {
            "SUBTOPIC"
        }
        None => "UNKNOWN",
    };

    let record = BaseRecord::to("space.trust.extensions")
        .key(&trust_extension.source_space_id)
        .payload(&payload)
        .headers(OwnedHeaders::new().insert(Header {
            key: "extension-type",
            value: Some(extension_type),
        }));

    producer.send(record).map_err(|(e, _)| e)?;
    Ok(())
}

fn send_edit(
    producer: &BaseProducer,
    edit: &HermesEdit,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = Vec::new();
    edit.encode(&mut payload)?;

    let record = BaseRecord::to("knowledge.edits")
        .key(&edit.space_id)
        .payload(&payload)
        .headers(OwnedHeaders::new().insert(Header {
            key: "edit-name",
            value: Some(&edit.name),
        }));

    producer.send(record).map_err(|(e, _)| e)?;
    Ok(())
}

// =============================================================================
// Main
// =============================================================================

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());

    println!("Hermes Processor starting...");
    println!("Connecting to Kafka broker: {}", broker);

    let producer: BaseProducer = create_producer(&broker)?;

    println!("Connected to Kafka broker");

    // Generate deterministic topology from mock-substream
    println!("\n=== Processing mock-substream topology ===\n");
    let blocks = test_topology::generate();

    let mut space_count = 0;
    let mut trust_count = 0;
    let mut edit_count = 0;
    let mut error_count = 0;

    for block in &blocks {
        for event in &block.events {
            let result = match event {
                MockEvent::SpaceCreated(space) => {
                    let hermes_space = convert_space_created(space);
                    let space_id_hex = hex::encode(&space.space_id);
                    match send_space(&producer, &hermes_space) {
                        Ok(_) => {
                            space_count += 1;
                            println!("Space created: {}", space_id_hex);
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
                MockEvent::TrustExtended(trust) => {
                    let hermes_trust = convert_trust_extended(trust);
                    let source_hex = hex::encode(&trust.source_space_id);
                    let ext_type = match &trust.extension {
                        TrustExtension::Verified { .. } => "verified",
                        TrustExtension::Related { .. } => "related",
                        TrustExtension::Subtopic { .. } => "subtopic",
                    };
                    match send_trust_extension(&producer, &hermes_trust) {
                        Ok(_) => {
                            trust_count += 1;
                            println!("Trust extended: {} -> {} ({})", source_hex, ext_type, ext_type);
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
                MockEvent::EditPublished(edit) => {
                    let hermes_edit = convert_edit_published(edit);
                    let space_id_hex = hex::encode(&edit.space_id);
                    match send_edit(&producer, &hermes_edit) {
                        Ok(_) => {
                            edit_count += 1;
                            println!(
                                "Edit published: {} in space {} ({} ops)",
                                edit.name,
                                space_id_hex,
                                edit.ops.len()
                            );
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
            };

            if let Err(e) = result {
                eprintln!("Error processing event: {}", e);
                error_count += 1;
            }
        }
    }

    // Flush all pending messages
    println!("\nFlushing messages to Kafka...");
    producer.flush(Duration::from_secs(30))?;

    println!("\n=== Processing complete ===");
    println!("Spaces created: {}", space_count);
    println!("Trust extensions: {}", trust_count);
    println!("Edits published: {}", edit_count);
    println!("Errors: {}", error_count);
    println!("\nHermes Processor finished.");

    Ok(())
}
