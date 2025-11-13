use chrono::Utc;
use prost::Message;
use rdkafka::config::ClientConfig;
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord, Producer};
use std::env;
use std::time::Duration;

use hermes_schema::pb::knowledge::HermesEdit;

trait Hermes {

}


fn create_sample_edit(space_id: String, name: String) -> HermesEdit {
    HermesEdit {
        id: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16], // Sample UUID bytes
        name,
        ops: vec![], // Empty ops for demo
        authors: vec![vec![0; 32]], // Sample author ID
        language: Some(vec![0; 16]), // Sample language ID
        space_id,
        is_canonical: true,
        created_at: Utc::now().timestamp().try_into().expect("timestamp should be positive"),
        created_by: vec![0; 32], // Sample creator address
        block_number: 12345,
        cursor: "sample_cursor".to_string(),
    }
}

async fn send_edit(
    producer: &FutureProducer,
    topic: &str,
    edit: &HermesEdit,
) -> Result<(), Box<dyn std::error::Error>> {
    // Serialize to protobuf
    let mut payload = Vec::new();
    edit.encode(&mut payload)?;

    let record = FutureRecord::to(topic)
        .key(&edit.space_id)
        .payload(&payload)
        .headers(OwnedHeaders::new().insert(Header {
            key: "edit-name",
            value: Some(&edit.name),
        }));

    match producer.send(record, Duration::from_secs(5)).await {
        Ok(delivery) => {
            println!(
                "Edit sent successfully: {} in space {} - {:?}",
                edit.name, edit.space_id, delivery
            );
            Ok(())
        }
        Err((e, _)) => {
            eprintln!("Error sending edit: {:?}", e);
            Err(Box::new(e))
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get Kafka broker from environment variable or use default
    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());

    // Create producer with ZSTD compression
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &broker)
        .set("client.id", "hermes-producer")
        .set("compression.type", "zstd") // Enable ZSTD compression
        .set("message.timeout.ms", "5000")
        .set("queue.buffering.max.messages", "100000")
        .set("queue.buffering.max.kbytes", "1048576")
        .set("batch.num.messages", "10000") // Batch messages for better compression
        .create()?;

    println!("Producer connected to {}", broker);

    // Send a single edit
    let edit = create_sample_edit(
        "space-123".to_string(),
        "Initial Edit".to_string(),
    );

    send_edit(&producer, "knowledge.edits", &edit).await?;

    // Send multiple edits in batch
    println!("Sending 10 edits in batch...");
    for i in 0..10 {
        let edit = create_sample_edit(
            format!("space-{}", i),
            format!("Edit {}", i),
        );
        send_edit(&producer, "knowledge.edits", &edit).await?;
    }

    println!("Sent 10 edits in batch");

    // Flush any remaining messages
    producer.flush(Duration::from_secs(5))?;
    println!("Producer flushed and disconnected");

    Ok(())
}
