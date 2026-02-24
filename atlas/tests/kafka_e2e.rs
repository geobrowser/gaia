//! Full-stack Atlas e2e test with real Kafka.
//!
//! This test runs the real `atlas` binary in mock substream mode and verifies
//! that a canonical diff message is produced to Kafka and can be decoded by a
//! real Kafka consumer.

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hermes_relay::source::mock_events::test_topology::ROOT_SPACE_ID;
use hermes_schema::pb::topology::CanonicalGraphDiff;
use prost::Message;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::message::Message as _;

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    format!("{}-{}", std::process::id(), nanos)
}

fn wait_for_process(mut child: std::process::Child, timeout: Duration) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("failed to poll atlas process") {
            return status;
        }

        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            panic!("atlas process timed out after {:?}", timeout);
        }

        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn test_atlas_mock_to_real_kafka_e2e() {
    if std::env::var("KAFKA_E2E").ok().as_deref() != Some("1") {
        eprintln!("Skipping kafka_e2e (set KAFKA_E2E=1 to enable)");
        return;
    }

    let broker = std::env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let environment = std::env::var("ENVIRONMENT").unwrap_or_else(|_| "staging".to_string());
    let topic_prefix = match environment.as_str() {
        "staging" => "staging.",
        "production" => "",
        other => panic!("ENVIRONMENT must be 'staging' or 'production', got '{other}'"),
    };
    let base_topic = "topology.canonical".to_string();
    let full_topic = format!("{}{}", topic_prefix, base_topic);
    let root_hex = hex::encode(ROOT_SPACE_ID);

    let bin = std::env::var("CARGO_BIN_EXE_atlas")
        .expect("CARGO_BIN_EXE_atlas should be set for integration tests");

    // Run full stack: mock substream source -> atlas pipeline -> real Kafka producer.
    let child = Command::new(bin)
        .env("USE_MOCK", "true")
        .env("KAFKA_BROKER", &broker)
        .env("KAFKA_TOPIC", &base_topic)
        .env("ROOT_SPACE_ID", &root_hex)
        .env("ENVIRONMENT", &environment)
        .env_remove("KAFKA_USERNAME")
        .env_remove("KAFKA_PASSWORD")
        .env_remove("KAFKA_SSL_CA_PEM")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn atlas binary");

    let status = wait_for_process(child, Duration::from_secs(30));
    assert!(
        status.success(),
        "atlas process failed with status {status}"
    );

    // Verify consumer output from real Kafka.
    let group_id = format!("atlas-kafka-e2e-{}", unique_suffix());
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", &broker)
        .set("group.id", &group_id)
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "earliest")
        .create()
        .expect("failed to create test consumer");

    consumer
        .subscribe(&[&full_topic])
        .expect("failed to subscribe to atlas topic");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut decoded: Option<CanonicalGraphDiff> = None;

    while Instant::now() < deadline {
        match consumer.poll(Duration::from_millis(200)) {
            Some(Ok(msg)) => {
                let payload = msg.payload().expect("kafka message should have payload");
                let diff = CanonicalGraphDiff::decode(payload)
                    .expect("payload should decode as CanonicalGraphDiff");
                decoded = Some(diff);
                break;
            }
            Some(Err(e)) => panic!("error while polling kafka: {e}"),
            None => {}
        }
    }

    let diff = decoded.expect("expected at least one canonical diff message from atlas");
    assert_eq!(
        diff.root_id,
        ROOT_SPACE_ID.to_vec(),
        "diff root_id mismatch"
    );
    assert!(!diff.changes.is_empty(), "expected non-empty diff changes");

    let added = diff
        .changes
        .iter()
        .filter(|c| c.change_type == hermes_schema::pb::topology::ChangeType::Added as i32)
        .count();
    assert!(added > 0, "expected at least one ADDED change in diff");
}
