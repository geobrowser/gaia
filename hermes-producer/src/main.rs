use chrono::Utc;
use prost::Message;
use rdkafka::config::ClientConfig;
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord, Producer};
use std::env;
use std::time::Duration;
use uuid::Uuid;

// Include the generated protobuf code
pub mod events {
    include!(concat!(env!("OUT_DIR"), "/events.rs"));
}

use events::{EventType, UserEvent, UserEventData};

impl UserEvent {
    fn new(user_id: String, event_type: EventType, data: UserEventData) -> Self {
        Self {
            event_id: Uuid::new_v4().to_string(),
            timestamp: Utc::now().timestamp_millis(),
            user_id,
            event_type: event_type as i32,
            data: Some(data),
        }
    }
}

async fn send_event(
    producer: &FutureProducer,
    topic: &str,
    event: &UserEvent,
) -> Result<(), Box<dyn std::error::Error>> {
    // Serialize to protobuf
    let mut payload = Vec::new();
    event.encode(&mut payload)?;

    let event_type_str = match EventType::try_from(event.event_type) {
        Ok(EventType::UserRegistered) => "USER_REGISTERED",
        Ok(EventType::UserLogin) => "USER_LOGIN",
        Ok(EventType::UserLogout) => "USER_LOGOUT",
        _ => "UNKNOWN",
    };

    let record = FutureRecord::to(topic)
        .key(&event.user_id)
        .payload(&payload)
        .headers(OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some(event_type_str),
        }));

    match producer.send(record, Duration::from_secs(5)).await {
        Ok(delivery) => {
            println!(
                "Event sent successfully: {} - {:?}",
                event.event_id, delivery
            );
            Ok(())
        }
        Err((e, _)) => {
            eprintln!("Error sending event: {:?}", e);
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
        .set("client.id", "my-app")
        .set("compression.type", "zstd") // Enable ZSTD compression
        .set("message.timeout.ms", "5000")
        .set("queue.buffering.max.messages", "100000")
        .set("queue.buffering.max.kbytes", "1048576")
        .set("batch.num.messages", "10000") // Batch messages for better compression
        .create()?;

    println!("Producer connected to {}", broker);

    // Send a single event
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("source".to_string(), "web".to_string());
    metadata.insert("ip".to_string(), "192.168.1.1".to_string());

    let event = UserEvent::new(
        "user-123".to_string(),
        EventType::UserRegistered,
        UserEventData {
            email: Some("user@example.com".to_string()),
            username: Some("john_doe".to_string()),
            metadata,
        },
    );

    send_event(&producer, "user.events", &event).await?;

    // Send multiple events in batch
    println!("Sending 100 events in batch...");
    for i in 0..100 {
        let event = UserEvent::new(
            format!("user-{}", i),
            EventType::UserLogin,
            UserEventData {
                email: None,
                username: None,
                metadata: std::collections::HashMap::new(),
            },
        );
        send_event(&producer, "user.events", &event).await?;
    }

    println!("Sent 100 events in batch");

    // Flush any remaining messages
    producer.flush(Duration::from_secs(5))?;
    println!("Producer flushed and disconnected");

    Ok(())
}
