use chrono::Utc;
use prost::Message;
use rdkafka::config::ClientConfig;
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{BaseProducer, BaseRecord, Producer};
use std::env;
use std::time::Duration;
use std::thread;
use uuid::Uuid;
use rand::Rng;

use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::knowledge::HermesEdit;
use hermes_schema::pb::space::{
    HermesCreateSpace, PersonalSpacePayload, DefaultDaoSpacePayload,
    HermesSpaceTrustExtension, VerifiedExtension, RelatedExtension, SubtopicExtension
};
use wire::pb::grc20::{Op, Entity, Value, Property, DataType, Relation};

fn random_uuid_bytes() -> Vec<u8> {
    Uuid::new_v4().as_bytes().to_vec()
}

fn random_address() -> Vec<u8> {
    let mut rng = rand::thread_rng();
    (0..32).map(|_| rng.gen()).collect()
}

fn create_sample_space() -> HermesCreateSpace {
    let mut rng = rand::thread_rng();
    let is_personal = rng.gen_bool(0.5);

    HermesCreateSpace {
        space_id: random_uuid_bytes(),
        topic_id: random_uuid_bytes(),
        payload: if is_personal {
            Some(hermes_schema::pb::space::hermes_create_space::Payload::PersonalSpace(
                PersonalSpacePayload {
                    owner: random_address(),
                }
            ))
        } else {
            let editor_count = rng.gen_range(1..=5);
            let member_count = rng.gen_range(3..=10);
            Some(hermes_schema::pb::space::hermes_create_space::Payload::DefaultDaoSpace(
                DefaultDaoSpacePayload {
                    initial_editors: (0..editor_count).map(|_| random_uuid_bytes()).collect(),
                    initial_members: (0..member_count).map(|_| random_uuid_bytes()).collect(),
                }
            ))
        },
        meta: Some(BlockchainMetadata {
            created_at: Utc::now().timestamp().try_into().expect("timestamp should be positive"),
            created_by: random_address(),
            block_number: rng.gen_range(1000000..9999999),
            cursor: format!("cursor_{}", Uuid::new_v4()),
        }),
    }
}

fn create_verified_trust_extension(
    source_space_id: Vec<u8>,
    target_space_id: Vec<u8>,
) -> HermesSpaceTrustExtension {
    let mut rng = rand::thread_rng();
    HermesSpaceTrustExtension {
        source_space_id,
        extension: Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Verified(
            VerifiedExtension { target_space_id }
        )),
        meta: Some(BlockchainMetadata {
            created_at: Utc::now().timestamp().try_into().expect("timestamp should be positive"),
            created_by: random_address(),
            block_number: rng.gen_range(1000000..9999999),
            cursor: format!("cursor_{}", Uuid::new_v4()),
        }),
    }
}

fn create_related_trust_extension(
    source_space_id: Vec<u8>,
    target_space_id: Vec<u8>,
) -> HermesSpaceTrustExtension {
    let mut rng = rand::thread_rng();
    HermesSpaceTrustExtension {
        source_space_id,
        extension: Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Related(
            RelatedExtension { target_space_id }
        )),
        meta: Some(BlockchainMetadata {
            created_at: Utc::now().timestamp().try_into().expect("timestamp should be positive"),
            created_by: random_address(),
            block_number: rng.gen_range(1000000..9999999),
            cursor: format!("cursor_{}", Uuid::new_v4()),
        }),
    }
}

fn create_subtopic_trust_extension(
    source_space_id: Vec<u8>,
    target_topic_id: Vec<u8>,
) -> HermesSpaceTrustExtension {
    let mut rng = rand::thread_rng();
    HermesSpaceTrustExtension {
        source_space_id,
        extension: Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Subtopic(
            SubtopicExtension { target_topic_id }
        )),
        meta: Some(BlockchainMetadata {
            created_at: Utc::now().timestamp().try_into().expect("timestamp should be positive"),
            created_by: random_address(),
            block_number: rng.gen_range(1000000..9999999),
            cursor: format!("cursor_{}", Uuid::new_v4()),
        }),
    }
}

fn create_random_entity_op() -> Op {
    Op {
        payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
            id: random_uuid_bytes(),
            values: vec![
                Value {
                    property: random_uuid_bytes(),
                    value: format!("Random value {}", rand::thread_rng().gen::<u32>()),
                    options: None,
                }
            ],
        })),
    }
}

fn create_random_property_op() -> Op {
    Op {
        payload: Some(wire::pb::grc20::op::Payload::CreateProperty(Property {
            id: random_uuid_bytes(),
            data_type: DataType::String as i32,
        })),
    }
}

fn create_random_relation_op() -> Op {
    Op {
        payload: Some(wire::pb::grc20::op::Payload::CreateRelation(Relation {
            id: random_uuid_bytes(),
            r#type: random_uuid_bytes(),
            from_entity: random_uuid_bytes(),
            from_space: Some(random_uuid_bytes()),
            from_version: None,
            to_entity: random_uuid_bytes(),
            to_space: Some(random_uuid_bytes()),
            to_version: None,
            entity: random_uuid_bytes(),
            position: None,
            verified: Some(true),
        })),
    }
}

fn create_sample_edit(space_id: String, name: String) -> HermesEdit {
    let mut rng = rand::thread_rng();
    let op_count = rng.gen_range(1..5);
    let mut ops = Vec::new();

    for _ in 0..op_count {
        let op_type = rng.gen_range(0..3);
        ops.push(match op_type {
            0 => create_random_entity_op(),
            1 => create_random_property_op(),
            _ => create_random_relation_op(),
        });
    }

    HermesEdit {
        id: random_uuid_bytes(),
        name,
        ops,
        authors: vec![random_address()],
        language: Some(random_uuid_bytes()),
        space_id,
        is_canonical: rng.gen_bool(0.8),
        meta: Some(BlockchainMetadata {
            created_at: Utc::now().timestamp().try_into().expect("timestamp should be positive"),
            created_by: random_address(),
            block_number: rng.gen_range(1000000..9999999),
            cursor: format!("cursor_{}", Uuid::new_v4()),
        }),
    }
}

fn send_edit(
    producer: &BaseProducer,
    topic: &str,
    edit: &HermesEdit,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = Vec::new();
    edit.encode(&mut payload)?;

    let record = BaseRecord::to(topic)
        .key(&edit.space_id)
        .payload(&payload)
        .headers(OwnedHeaders::new().insert(Header {
            key: "edit-name",
            value: Some(&edit.name),
        }));

    match producer.send(record) {
        Ok(_) => {
            producer.flush(Duration::from_secs(5))?;
            println!(
                "Edit sent successfully: {} in space {}",
                edit.name, edit.space_id
            );
            Ok(())
        }
        Err((e, _)) => {
            Err(Box::new(e))
        }
    }
}

fn send_space(
    producer: &BaseProducer,
    topic: &str,
    space: &HermesCreateSpace,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = Vec::new();
    space.encode(&mut payload)?;

    let space_type = match &space.payload {
        Some(hermes_schema::pb::space::hermes_create_space::Payload::PersonalSpace(_)) => "PERSONAL",
        Some(hermes_schema::pb::space::hermes_create_space::Payload::DefaultDaoSpace(_)) => "DEFAULT_DAO",
        None => "UNKNOWN",
    };

    let record = BaseRecord::to(topic)
        .key(&space.space_id)
        .payload(&payload)
        .headers(OwnedHeaders::new().insert(Header {
            key: "space-type",
            value: Some(space_type),
        }));

    match producer.send(record) {
        Ok(_) => {
            producer.flush(Duration::from_secs(5))?;
            println!(
                "Space created successfully: {} type",
                space_type
            );
            Ok(())
        }
        Err((e, _)) => {
            Err(Box::new(e))
        }
    }
}

fn send_trust_extension(
    producer: &BaseProducer,
    topic: &str,
    trust_extension: &HermesSpaceTrustExtension,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = Vec::new();
    trust_extension.encode(&mut payload)?;

    let extension_type = match &trust_extension.extension {
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Verified(_)) => "VERIFIED",
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Related(_)) => "RELATED",
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Subtopic(_)) => "SUBTOPIC",
        None => "UNKNOWN",
    };

    let record = BaseRecord::to(topic)
        .key(&trust_extension.source_space_id)
        .payload(&payload)
        .headers(OwnedHeaders::new().insert(Header {
            key: "extension-type",
            value: Some(extension_type),
        }));

    match producer.send(record) {
        Ok(_) => {
            producer.flush(Duration::from_secs(5))?;
            println!(
                "Trust extension sent successfully: {} type",
                extension_type
            );
            Ok(())
        }
        Err((e, _)) => {
            Err(Box::new(e))
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());

    let producer: BaseProducer = ClientConfig::new()
        .set("bootstrap.servers", &broker)
        .set("client.id", "hermes-producer")
        .set("compression.type", "zstd")
        .set("message.timeout.ms", "5000")
        .set("queue.buffering.max.messages", "100000")
        .set("queue.buffering.max.kbytes", "1048576")
        .set("batch.num.messages", "10000")
        .create()?;

    println!("Mock producer connected to {}", broker);

    println!("\n=== Deterministic Flow: Creating 5 spaces with 10 edits each ===");

    // Store created spaces to build trust relationships between them
    let mut created_spaces: Vec<HermesCreateSpace> = Vec::new();

    for space_num in 1..=5 {
        println!("\nCreating space #{}", space_num);
        let space = create_sample_space();
        let space_id_hex = hex::encode(&space.space_id);

        if let Err(e) = send_space(&producer, "space.creations", &space) {
            eprintln!("Failed to send space: {}", e);
            continue;
        }

        created_spaces.push(space.clone());

        thread::sleep(Duration::from_millis(500));

        for edit_num in 1..=10 {
            let edit = create_sample_edit(
                space_id_hex.clone(),
                format!("Space {} Edit #{}", space_num, edit_num),
            );

            if let Err(e) = send_edit(&producer, "knowledge.edits", &edit) {
                eprintln!("Failed to send edit: {}", e);
            }

            thread::sleep(Duration::from_millis(200));
        }
    }

    println!("\n=== Creating trust extensions between spaces ===");

    // Create various trust relationships between the created spaces
    if created_spaces.len() >= 2 {
        // Space 0 -> Space 1: Verified trust
        let verified_ext = create_verified_trust_extension(
            created_spaces[0].space_id.clone(),
            created_spaces[1].space_id.clone(),
        );
        if let Err(e) = send_trust_extension(&producer, "space.trust.extensions", &verified_ext) {
            eprintln!("Failed to send verified trust extension: {}", e);
        }
        thread::sleep(Duration::from_millis(300));

        // Space 1 -> Space 2: Related trust
        if created_spaces.len() >= 3 {
            let related_ext = create_related_trust_extension(
                created_spaces[1].space_id.clone(),
                created_spaces[2].space_id.clone(),
            );
            if let Err(e) = send_trust_extension(&producer, "space.trust.extensions", &related_ext) {
                eprintln!("Failed to send related trust extension: {}", e);
            }
            thread::sleep(Duration::from_millis(300));
        }

        // Space 2 -> Space 3: Verified trust
        if created_spaces.len() >= 4 {
            let verified_ext = create_verified_trust_extension(
                created_spaces[2].space_id.clone(),
                created_spaces[3].space_id.clone(),
            );
            if let Err(e) = send_trust_extension(&producer, "space.trust.extensions", &verified_ext) {
                eprintln!("Failed to send verified trust extension: {}", e);
            }
            thread::sleep(Duration::from_millis(300));
        }

        // Space 0 -> Topic of Space 3: Subtopic trust
        if created_spaces.len() >= 4 {
            let subtopic_ext = create_subtopic_trust_extension(
                created_spaces[0].space_id.clone(),
                created_spaces[3].topic_id.clone(),
            );
            if let Err(e) = send_trust_extension(&producer, "space.trust.extensions", &subtopic_ext) {
                eprintln!("Failed to send subtopic trust extension: {}", e);
            }
            thread::sleep(Duration::from_millis(300));
        }

        // Space 3 -> Space 4: Related trust
        if created_spaces.len() >= 5 {
            let related_ext = create_related_trust_extension(
                created_spaces[3].space_id.clone(),
                created_spaces[4].space_id.clone(),
            );
            if let Err(e) = send_trust_extension(&producer, "space.trust.extensions", &related_ext) {
                eprintln!("Failed to send related trust extension: {}", e);
            }
            thread::sleep(Duration::from_millis(300));
        }

        // Space 4 -> Space 0: Verified trust (completing a trust cycle)
        if created_spaces.len() >= 5 {
            let verified_ext = create_verified_trust_extension(
                created_spaces[4].space_id.clone(),
                created_spaces[0].space_id.clone(),
            );
            if let Err(e) = send_trust_extension(&producer, "space.trust.extensions", &verified_ext) {
                eprintln!("Failed to send verified trust extension: {}", e);
            }
            thread::sleep(Duration::from_millis(300));
        }
    }

    println!("\n=== Deterministic flow complete: 5 spaces, 50 edits, 6 trust extensions ===");
    println!("Producer finished. Exiting.\n");

    Ok(())
    
    // Random flow disabled for now
    // println!("=== Switching to random emission mode ===\n");
    // 
    // let mut edit_counter = 50u64;
    // let mut loop_counter = 0u64;
    // let mut created_spaces: Vec<Vec<u8>> = Vec::new();
    //
    // println!("Creating initial space for random mode...");
    // let initial_space = create_sample_space();
    // created_spaces.push(initial_space.space_id.clone());
    // if let Err(e) = send_space(&producer, "space.creations", &initial_space) {
    //     eprintln!("Failed to send initial space: {}", e);
    // }
    //
    // loop {
    //     thread::sleep(Duration::from_secs(3));
    //     loop_counter += 1;
    //     
    //     edit_counter += 1;
    //     let space_id_bytes = created_spaces[rand::thread_rng().gen_range(0..created_spaces.len())].clone();
    //     let space_id_hex = hex::encode(&space_id_bytes);
    //     let edit = create_sample_edit(
    //         space_id_hex.clone(),
    //         format!("Random Edit #{}", edit_counter),
    //     );
    //
    //     if let Err(e) = send_edit(&producer, "knowledge.edits", &edit) {
    //         eprintln!("Failed to send edit: {}", e);
    //     }
    //
    //     if loop_counter % 3 == 0 {
    //         let space = create_sample_space();
    //         created_spaces.push(space.space_id.clone());
    //         if let Err(e) = send_space(&producer, "space.creations", &space) {
    //             eprintln!("Failed to send space: {}", e);
    //         }
    //     }
    // }
}
