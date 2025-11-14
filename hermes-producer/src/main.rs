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

use hermes_schema::pb::knowledge::HermesEdit;
use wire::pb::grc20::{Op, Entity, Value, Property, DataType, Relation};

fn random_uuid_bytes() -> Vec<u8> {
    Uuid::new_v4().as_bytes().to_vec()
}

fn random_address() -> Vec<u8> {
    let mut rng = rand::thread_rng();
    (0..32).map(|_| rng.gen()).collect()

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
        created_at: Utc::now().timestamp().try_into().expect("timestamp should be positive"),
        created_by: random_address(),
        block_number: rng.gen_range(1000000..9999999),
        cursor: format!("cursor_{}", Uuid::new_v4()),
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
    println!("Emitting random HermesEdit every 3 seconds (Ctrl+C to stop)");

    let mut counter = 0u64;

    loop {
        thread::sleep(Duration::from_secs(3));
        
        counter += 1;
        let space_id = format!("space-{}", rand::thread_rng().gen_range(1..=5));
        let edit = create_sample_edit(
            space_id.clone(),
            format!("Random Edit #{}", counter),
        );

        if let Err(e) = send_edit(&producer, "knowledge.edits", &edit) {
            eprintln!("Failed to send edit: {}", e);
        }
    }
}
