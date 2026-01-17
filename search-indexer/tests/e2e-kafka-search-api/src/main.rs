mod generators;
mod kafka;

use anyhow::Result;
use clap::Parser;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;
use uuid::Uuid;

use generators::{edits, relations, scores};
use kafka::KafkaProducer;

const DEFAULT_KAFKA_BROKER: &str = "localhost:9092";
const EDITS_TOPIC: &str = "knowledge.edits";
const SCORES_TOPIC: &str = "curation.scores";

#[derive(Parser)]
#[command(name = "e2e-kafka-search-api")]
#[command(about = "E2E test tool for search-indexer Kafka topics and Search API", long_about = None)]
struct Cli {
    /// Kafka broker address
    #[arg(short, long, default_value = DEFAULT_KAFKA_BROKER)]
    broker: String,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up logging
    let log_level = if cli.debug { Level::DEBUG } else { Level::INFO };
    let subscriber = FmtSubscriber::builder().with_max_level(log_level).finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // Create Kafka producer
    let producer = KafkaProducer::new(&cli.broker)?;

    info!("Generating comprehensive test scenario with score-based ordering");

    // Create test space and entities with FIXED IDs for validation
    let test_space = Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
    let person_type_id = Uuid::parse_str("00000000-0000-0000-0000-000000000b01").unwrap();
    let org_type_id = Uuid::parse_str("00000000-0000-0000-0000-000000000b02").unwrap();

    // Multiple Alice entities for score-based testing (FIXED IDs for validation)
    let alice_high_id = Uuid::parse_str("00000000-0000-0000-0000-0000000000f1").unwrap();
    let alice_medium_id = Uuid::parse_str("00000000-0000-0000-0000-0000000000f2").unwrap();
    let alice_low_id = Uuid::parse_str("00000000-0000-0000-0000-0000000000f3").unwrap();
    let alice_zero_id = Uuid::parse_str("00000000-0000-0000-0000-0000000000f4").unwrap();
    let alice_negative_id = Uuid::parse_str("00000000-0000-0000-0000-0000000000f5").unwrap();
    let alice_at_threshold_id = Uuid::parse_str("00000000-0000-0000-0000-0000000000f6").unwrap();
    let alice_below_threshold_id =
        Uuid::parse_str("00000000-0000-0000-0000-0000000000f7").unwrap();

    // Other entities
    let bob_id = Uuid::parse_str("00000000-0000-0000-0000-000000000b0b").unwrap();
    let org_id = Uuid::parse_str("00000000-0000-0000-0000-0000000ac3ec").unwrap();

    // Entities to be deleted (for soft delete testing)
    let charlie_id = Uuid::parse_str("00000000-0000-0000-0000-000000000c01").unwrap();
    let dana_id = Uuid::parse_str("00000000-0000-0000-0000-000000000d01").unwrap();
    let eve_id = Uuid::parse_str("00000000-0000-0000-0000-000000000e01").unwrap();

    info!("Test Space ID: {}", test_space);
    info!("Person Type ID: {}", person_type_id);
    info!("Organization Type ID: {}", org_type_id);
    info!("\nAlice variants:");
    info!(
        "  Alice (High Score) ID: {} - will have multiple types",
        alice_high_id
    );
    info!(
        "  Alice (Medium Score) ID: {} - will have type added then removed",
        alice_medium_id
    );
    info!(
        "  Alice (Low Score) ID: {} - will have partial type removal",
        alice_low_id
    );
    info!("  Alice (Zero Score) ID: {}", alice_zero_id);
    info!("  Alice (Negative Score) ID: {}", alice_negative_id);
    info!("  Alice (At Threshold) ID: {}", alice_at_threshold_id);
    info!(
        "  Alice (Below Threshold) ID: {}",
        alice_below_threshold_id
    );
    info!("\nOther entities:");
    info!("  Bob ID: {}", bob_id);
    info!("  Organization ID: {}", org_id);
    info!("\nDeletion test entities:");
    info!("  Charlie ID: {} (will be deleted)", charlie_id);
    info!("  Dana ID: {} (will be deleted)", dana_id);
    info!("  Eve ID: {} (will be deleted then updated)", eve_id);

    // 1. Create Person type entity
    info!("\n1. Creating Person type entity...");
    let person_type_payload = edits::create_entity_edit(
        "Create Person Type",
        test_space,
        person_type_id,
        Some("Person"),
        Some("A human being"),
        None,
    )?;
    producer
        .send(EDITS_TOPIC, None, person_type_payload)
        .await?;

    // 2. Create Organization type entity
    info!("2. Creating Organization type entity...");
    let org_type_payload = edits::create_entity_edit(
        "Create Organization Type",
        test_space,
        org_type_id,
        Some("Organization"),
        Some("A structured group of people"),
        None,
    )?;
    producer
        .send(EDITS_TOPIC, None, org_type_payload)
        .await?;

    // 3. Create multiple Alice entities with different score profiles
    info!("3. Creating Alice entities with varying characteristics...");

    let alice_high_payload = edits::create_entity_edit(
        "Create Alice High",
        test_space,
        alice_high_id,
        Some("Alice"),
        Some("Software developer with high global score"),
        None,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_high_payload)
        .await?;

    let alice_medium_payload = edits::create_entity_edit(
        "Create Alice Medium",
        test_space,
        alice_medium_id,
        Some("Alice"),
        Some("Software developer with medium global score"),
        None,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_medium_payload)
        .await?;

    let alice_low_payload = edits::create_entity_edit(
        "Create Alice Low",
        test_space,
        alice_low_id,
        Some("Alice"),
        Some("Software developer with low global score"),
        None,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_low_payload)
        .await?;

    let alice_zero_payload = edits::create_entity_edit(
        "Create Alice Zero",
        test_space,
        alice_zero_id,
        Some("Alice"),
        Some("Software developer with zero global score"),
        None,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_zero_payload)
        .await?;

    let alice_negative_payload = edits::create_entity_edit(
        "Create Alice Negative",
        test_space,
        alice_negative_id,
        Some("Alice"),
        Some("Software developer with negative global score"),
        None,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_negative_payload)
        .await?;

    let alice_at_threshold_payload = edits::create_entity_edit(
        "Create Alice At Threshold",
        test_space,
        alice_at_threshold_id,
        Some("Alice"),
        Some("Software developer at score threshold"),
        None,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_at_threshold_payload)
        .await?;

    let alice_below_threshold_payload = edits::create_entity_edit(
        "Create Alice Below Threshold",
        test_space,
        alice_below_threshold_id,
        Some("Alice"),
        Some("Software developer below score threshold"),
        None,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_below_threshold_payload)
        .await?;

    // 4. Create Bob
    info!("4. Creating Bob entity...");
    let bob_payload = edits::create_entity_edit(
        "Create Bob",
        test_space,
        bob_id,
        Some("Bob"),
        Some("A project manager"),
        None,
    )?;
    producer.send(EDITS_TOPIC, None, bob_payload).await?;

    // 5. Create Organization
    info!("5. Creating Acme Corp organization...");
    let org_payload = edits::create_entity_edit(
        "Create Acme Corp",
        test_space,
        org_id,
        Some("Acme Corp"),
        Some("A technology company"),
        None,
    )?;
    producer.send(EDITS_TOPIC, None, org_payload).await?;

    // 6. Create type relations for most Alice entities and others
    info!("6. Creating type relations...");
    for (name, entity_id) in [
        ("Alice High", alice_high_id),
        ("Alice Medium", alice_medium_id),
        ("Alice Low", alice_low_id),
        ("Alice Zero", alice_zero_id),
        ("Alice Negative", alice_negative_id),
        ("Alice At Threshold", alice_at_threshold_id),
        ("Alice Below Threshold", alice_below_threshold_id),
        ("Bob", bob_id),
    ] {
        let type_rel_payload = relations::create_type_relation(
            &format!("{} -> Person Type", name),
            test_space,
            entity_id,
            person_type_id,
        )?;
        producer
            .send(EDITS_TOPIC, None, type_rel_payload)
            .await?;
    }

    let org_type_payload_rel = relations::create_type_relation(
        "Acme Corp -> Organization Type",
        test_space,
        org_id,
        org_type_id,
    )?;
    producer
        .send(EDITS_TOPIC, None, org_type_payload_rel)
        .await?;

    // 6.1. TypeIds test scenarios using Alice entities
    info!("6.1. Setting up typeIds test scenarios with Alice entities...");

    // Alice High: Multiple type relations (typeIds should have both Person and Organization)
    info!(
        "  - Alice High: Adding Organization type relation (already has Person)..."
    );
    let alice_high_org_payload = relations::create_type_relation(
        "Alice High -> Organization Type",
        test_space,
        alice_high_id,
        org_type_id,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_high_org_payload)
        .await?;

    // Alice Medium: Create -> Delete -> Create pattern (tests relation recreation)
    info!("  - Alice Medium: Adding Organization type relation (first time)...");
    let alice_medium_org_rel_id_1 = Uuid::new_v4();
    let alice_medium_org_create_1_payload = relations::create_type_relation_with_id(
        "Alice Medium -> Organization Type (first, to be removed)",
        test_space,
        alice_medium_org_rel_id_1,
        alice_medium_id,
        org_type_id,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_medium_org_create_1_payload)
        .await?;

    info!("  - Alice Medium: Removing Organization type relation...");
    let alice_medium_org_delete_payload = relations::delete_relation(
        "Delete Alice Medium -> Organization Type",
        test_space,
        alice_medium_org_rel_id_1,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_medium_org_delete_payload)
        .await?;

    info!("  - Alice Medium: Re-adding Organization type relation (second time, should be final state)...");
    let alice_medium_org_rel_id_2 = Uuid::new_v4();
    let alice_medium_org_create_2_payload = relations::create_type_relation_with_id(
        "Alice Medium -> Organization Type (recreated)",
        test_space,
        alice_medium_org_rel_id_2,
        alice_medium_id,
        org_type_id,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_medium_org_create_2_payload)
        .await?;

    // Alice Low: Two type relations, one removed (typeIds should only have Person)
    info!("  - Alice Low: Adding Organization type relation (to be removed)...");
    let alice_low_org_rel_id = Uuid::new_v4();
    let alice_low_org_create_payload = relations::create_type_relation_with_id(
        "Alice Low -> Organization Type (to be removed)",
        test_space,
        alice_low_org_rel_id,
        alice_low_id,
        org_type_id,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_low_org_create_payload)
        .await?;

    info!("  - Alice Low: Removing Organization type relation...");
    let alice_low_org_delete_payload = relations::delete_relation(
        "Delete Alice Low -> Organization Type",
        test_space,
        alice_low_org_rel_id,
    )?;
    producer
        .send(EDITS_TOPIC, None, alice_low_org_delete_payload)
        .await?;

    // 7. Generate scores with varying values
    info!("7. Generating scores with varying entity and space scores...");
    let score_payload = scores::create_mixed_score_batch(
        vec![
            // Alice entities with different score profiles
            (alice_high_id, 0.95),           // High positive score
            (alice_medium_id, 0.65),         // Medium positive score
            (alice_low_id, 0.15),            // Low positive score
            (alice_zero_id, 0.0),            // Exactly zero
            (alice_negative_id, -0.75),      // Negative score (z-score)
            (alice_at_threshold_id, 0.50),   // At typical threshold
            (alice_below_threshold_id, 0.25), // Below threshold
            // Other entities
            (bob_id, 0.75),
            (org_id, 0.90),
            (person_type_id, 0.70),
            (org_type_id, 0.65),
        ],
        vec![(test_space, 0.95)],
        vec![
            // Perspective scores (entity-space combinations)
            (alice_high_id, test_space, 0.98),
            (alice_medium_id, test_space, 0.70),
            (alice_low_id, test_space, 0.20),
            (alice_zero_id, test_space, 0.0),
            (alice_negative_id, test_space, -0.60),
            (bob_id, test_space, 0.78),
            (org_id, test_space, 0.92),
        ],
        1,
        true,
    )?;
    producer
        .send(SCORES_TOPIC, None, score_payload)
        .await?;

    // 8. Create entities that will be soft deleted (for delete testing)
    info!("8. Creating entities for deletion testing...");
    let charlie_payload = edits::create_entity_edit(
        "Create Charlie",
        test_space,
        charlie_id,
        Some("Charlie"),
        Some("This entity will be deleted"),
        None,
    )?;
    producer.send(EDITS_TOPIC, None, charlie_payload).await?;

    let dana_payload = edits::create_entity_edit(
        "Create Dana",
        test_space,
        dana_id,
        Some("Dana"),
        Some("This entity will also be deleted"),
        None,
    )?;
    producer.send(EDITS_TOPIC, None, dana_payload).await?;

    let eve_payload = edits::create_entity_edit(
        "Create Eve",
        test_space,
        eve_id,
        Some("Eve"),
        Some("This entity will be deleted then updated"),
        None,
    )?;
    producer.send(EDITS_TOPIC, None, eve_payload).await?;

    // 9. Delete the test entities (soft delete)
    info!("9. Soft deleting Charlie, Dana, and Eve...");
    let delete_charlie_payload = edits::delete_entity(
        "Delete Charlie",
        test_space,
        charlie_id,
    )?;
    producer.send(EDITS_TOPIC, None, delete_charlie_payload).await?;

    let delete_dana_payload = edits::delete_entity(
        "Delete Dana",
        test_space,
        dana_id,
    )?;
    producer.send(EDITS_TOPIC, None, delete_dana_payload).await?;

    let delete_eve_payload = edits::delete_entity(
        "Delete Eve",
        test_space,
        eve_id,
    )?;
    producer.send(EDITS_TOPIC, None, delete_eve_payload).await?;

    // 10. Update a deleted entity (Eve) - should remain deleted
    info!("10. Updating Eve after deletion (testing delete-then-update behavior)...");
    let update_eve_payload = edits::create_entity_edit(
        "Update Eve After Delete",
        test_space,
        eve_id,
        Some("Eve Updated"),
        Some("This entity was updated after being deleted - should remain deleted"),
        None,
    )?;
    producer.send(EDITS_TOPIC, None, update_eve_payload).await?;

    info!("\n✅ Test scenario complete!");
    info!("Created:");
    info!("  - 14 entities (11 active + 3 deleted)");
    info!("    • 7 Alice variants (high, medium, low, zero, negative, at threshold, below threshold)");
    info!("    • Bob, Acme Corp");
    info!("    • Person type, Organization type");
    info!("    • Charlie, Dana (soft deleted)");
    info!("    • Eve (soft deleted, then updated - remains deleted)");
    info!("  - Type relation scenarios:");
    info!("    • Alice High: Multiple types (Person + Organization)");
    info!("    • Alice Medium: Create->Delete->Create pattern (Person + Organization recreated)");
    info!("    • Alice Low: Partial type removal (Person kept, Org added + deleted)");
    info!("    • Other Alice entities, Bob: Single type (Person)");
    info!("    • Acme Corp: Single type (Organization)");
    info!("  - 14 type relation events (10 creates, 2 deletes, 1 recreate for testing typeIds)");
    info!("  - 11 entity scores (including negative and zero)");
    info!("  - 1 space score");
    info!("  - 7 perspective scores");
    info!("\nScore ranges:");
    info!("  • High: 0.95");
    info!("  • Medium: 0.65");
    info!("  • Low: 0.15");
    info!("  • Zero: 0.0");
    info!("  • Negative: -0.75");
    info!("  • At Threshold: 0.50");
    info!("  • Below Threshold: 0.25");

    Ok(())
}
