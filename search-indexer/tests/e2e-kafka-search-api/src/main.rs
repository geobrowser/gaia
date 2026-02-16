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

/// Returns the topic prefix based on the ENVIRONMENT variable.
/// - ENVIRONMENT=staging -> "staging."
/// - ENVIRONMENT=production -> ""
/// - ENVIRONMENT not set -> panics (fail-safe)
fn get_topic_prefix() -> &'static str {
    match std::env::var("ENVIRONMENT").as_deref() {
        Ok("staging") => "staging.",
        Ok("production") => "",
        Ok(other) => panic!(
            "ENVIRONMENT variable must be set to 'staging' or 'production', got: '{}'",
            other
        ),
        Err(_) => panic!(
            "ENVIRONMENT variable must be set to 'staging' or 'production': NotPresent"
        ),
    }
}

/// Returns the prefixed topic name based on the ENVIRONMENT variable.
fn prefixed_topic(topic: &str) -> String {
    format!("{}{}", get_topic_prefix(), topic)
}

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

    // Get prefixed topic names based on ENVIRONMENT variable
    let edits_topic = prefixed_topic("knowledge.edits");
    let scores_topic = prefixed_topic("curation.scores");
    let topic_prefix = get_topic_prefix();

    info!("Topic prefix: '{}'", topic_prefix);
    info!("Using topics: edits={}, scores={}", edits_topic, scores_topic);

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
    let charlie_id = Uuid::parse_str("00000000-0000-0000-0000-000000000c1c").unwrap();
    let org_id = Uuid::parse_str("00000000-0000-0000-0000-0000000ac3ec").unwrap();

    // Entities to be deleted (for soft delete testing)
    let delete_charlie_id = Uuid::parse_str("00000000-0000-0000-0000-000000000c01").unwrap();
    let delete_dana_id = Uuid::parse_str("00000000-0000-0000-0000-000000000d01").unwrap();
    let delete_eve_id = Uuid::parse_str("00000000-0000-0000-0000-000000000e01").unwrap();

    // Entity created via CreateEntity op (for testing CreateEntity handling)
    let create_entity_test_id = Uuid::parse_str("00000000-0000-0000-0000-00000000ce01").unwrap();

    // Text match scoring test entities (all get the same entity_global_score so scoreBoost is equal)
    // Group A: Name match vs description-only match (query: "Wonderland")
    let tm_name_match_id = Uuid::parse_str("00000000-0000-0000-0000-00000000aa01").unwrap();
    let tm_desc_match_id = Uuid::parse_str("00000000-0000-0000-0000-00000000aa02").unwrap();
    // Group B: Exact match vs fuzzy match (query: "Blockchain")
    let tm_exact_match_id = Uuid::parse_str("00000000-0000-0000-0000-00000000aa03").unwrap();
    let tm_fuzzy_match_id = Uuid::parse_str("00000000-0000-0000-0000-00000000aa04").unwrap();
    // Group C: Multi-word match vs single-word match (query: "San Francisco")
    let tm_multi_word_id = Uuid::parse_str("00000000-0000-0000-0000-00000000aa05").unwrap();
    let tm_single_word_id = Uuid::parse_str("00000000-0000-0000-0000-00000000aa06").unwrap();
    // Group D: Name+description match vs name-only match (query: "Quantum")
    let tm_name_and_desc_id = Uuid::parse_str("00000000-0000-0000-0000-00000000aa07").unwrap();
    let tm_name_only_id = Uuid::parse_str("00000000-0000-0000-0000-00000000aa08").unwrap();
    // Group E: High global score vs low global score, both match in name (query: "Velociraptor")
    // Both have "Velociraptor" in name, but low-score entity is an exact single-word name match
    // (slightly higher BM25 score). High score boost should overcome the small text match difference.
    let tm_high_score_id = Uuid::parse_str("00000000-0000-0000-0000-00000000aa09").unwrap();
    let tm_low_score_id = Uuid::parse_str("00000000-0000-0000-0000-00000000aa0a").unwrap();
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
    info!("  Charlie ID: {} - will have NO global score", charlie_id);
    info!("  Organization ID: {}", org_id);
    info!("\nDeletion test entities:");
    info!("  Delete Charlie ID: {} (will be deleted)", delete_charlie_id);
    info!("  Delete Dana ID: {} (will be deleted)", delete_dana_id);
    info!("  Delete Eve ID: {} (will be deleted then updated)", delete_eve_id);
    info!("\nCreateEntity test:");
    info!("  CreateEntity Test ID: {} (created via CreateEntity op)", create_entity_test_id);
    info!("\nText match scoring test entities:");
    info!("  Group A (query 'Wonderland'): name match {} vs desc match {}", tm_name_match_id, tm_desc_match_id);
    info!("  Group B (query 'Blockchain'): exact {} vs fuzzy {}", tm_exact_match_id, tm_fuzzy_match_id);
    info!("  Group C (query 'San Francisco'): multi-word {} vs single-word {}", tm_multi_word_id, tm_single_word_id);
    info!("  Group D (query 'Quantum'): name+desc {} vs name-only {}", tm_name_and_desc_id, tm_name_only_id);
    info!("  Group E (query 'Velociraptor'): high-score {} vs low-score {}", tm_high_score_id, tm_low_score_id);
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
        .send(&edits_topic, None, person_type_payload)
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
        .send(&edits_topic, None, org_type_payload)
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
        .send(&edits_topic, None, alice_high_payload)
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
        .send(&edits_topic, None, alice_medium_payload)
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
        .send(&edits_topic, None, alice_low_payload)
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
        .send(&edits_topic, None, alice_zero_payload)
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
        .send(&edits_topic, None, alice_negative_payload)
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
        .send(&edits_topic, None, alice_at_threshold_payload)
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
        .send(&edits_topic, None, alice_below_threshold_payload)
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
    producer.send(&edits_topic, None, bob_payload).await?;

    // 5. Create Charlie (no global score)
    info!("5. Creating Charlie entity (will have NO global score)...");
    let charlie_payload = edits::create_entity_edit(
        "Create Charlie",
        test_space,
        charlie_id,
        Some("Charlie"),
        Some("A designer with no global score"),
        None,
    )?;
    producer.send(&edits_topic, None, charlie_payload).await?;

    // 6. Create Organization
    info!("6. Creating Acme Corp organization...");
    let org_payload = edits::create_entity_edit(
        "Create Acme Corp",
        test_space,
        org_id,
        Some("Acme Corp"),
        Some("A technology company"),
        None,
    )?;
    producer.send(&edits_topic, None, org_payload).await?;

    // 7. Create type relations for most Alice entities and others
    info!("7. Creating type relations...");
    for (name, entity_id) in [
        ("Alice High", alice_high_id),
        ("Alice Medium", alice_medium_id),
        ("Alice Low", alice_low_id),
        ("Alice Zero", alice_zero_id),
        ("Alice Negative", alice_negative_id),
        ("Alice At Threshold", alice_at_threshold_id),
        ("Alice Below Threshold", alice_below_threshold_id),
        ("Bob", bob_id),
        ("Charlie", charlie_id),
    ] {
        let type_rel_payload = relations::create_type_relation(
            &format!("{} -> Person Type", name),
            test_space,
            entity_id,
            person_type_id,
        )?;
        producer
            .send(&edits_topic, None, type_rel_payload)
            .await?;
    }

    let org_type_payload_rel = relations::create_type_relation(
        "Acme Corp -> Organization Type",
        test_space,
        org_id,
        org_type_id,
    )?;
    producer
        .send(&edits_topic, None, org_type_payload_rel)
        .await?;

    // 7.1. TypeIds test scenarios using Alice entities
    info!("7.1. Setting up typeIds test scenarios with Alice entities...");

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
        .send(&edits_topic, None, alice_high_org_payload)
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
        .send(&edits_topic, None, alice_medium_org_create_1_payload)
        .await?;

    info!("  - Alice Medium: Removing Organization type relation...");
    let alice_medium_org_delete_payload = relations::delete_relation(
        "Delete Alice Medium -> Organization Type",
        test_space,
        alice_medium_org_rel_id_1,
    )?;
    producer
        .send(&edits_topic, None, alice_medium_org_delete_payload)
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
        .send(&edits_topic, None, alice_medium_org_create_2_payload)
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
        .send(&edits_topic, None, alice_low_org_create_payload)
        .await?;

    info!("  - Alice Low: Removing Organization type relation...");
    let alice_low_org_delete_payload = relations::delete_relation(
        "Delete Alice Low -> Organization Type",
        test_space,
        alice_low_org_rel_id,
    )?;
    producer
        .send(&edits_topic, None, alice_low_org_delete_payload)
        .await?;

    // 7.5. Create text match scoring test entities
    // All entities in each group have the SAME score (0.50) so textMatchScore comparisons
    // reflect only text matching quality, not score boost differences.
    info!("7.5. Creating text match scoring test entities...");

    // Group A: Name match vs description-only match (query: "Wonderland")
    // tm_name_match: name IS the query → should score highest
    // tm_desc_match: name is unrelated, query appears in short description → should score lower
    let tm_name_match_payload = edits::create_entity_edit(
        "Create TM Name Match (Wonderland)",
        test_space,
        tm_name_match_id,
        Some("Wonderland"),
        Some("A magical place in fiction"),
        None,
    )?;
    producer.send(&edits_topic, None, tm_name_match_payload).await?;

    let tm_desc_match_payload = edits::create_entity_edit(
        "Create TM Desc Match (Rex)",
        test_space,
        tm_desc_match_id,
        Some("Rex"),
        Some("Researcher @Wonderland"),
        None,
    )?;
    producer.send(&edits_topic, None, tm_desc_match_payload).await?;

    // Group B: Exact match vs fuzzy match (query: "Blockchain")
    // tm_exact_match: name exactly matches query → should score highest
    // tm_fuzzy_match: name has typo, matches only via fuzziness → should score lower
    let tm_exact_match_payload = edits::create_entity_edit(
        "Create TM Exact Match (Blockchain)",
        test_space,
        tm_exact_match_id,
        Some("Blockchain"),
        Some("A distributed ledger technology"),
        None,
    )?;
    producer.send(&edits_topic, None, tm_exact_match_payload).await?;

    let tm_fuzzy_match_payload = edits::create_entity_edit(
        "Create TM Fuzzy Match (Blockchan)",
        test_space,
        tm_fuzzy_match_id,
        Some("Blockchan"),
        Some("A distributed ledger technology"),
        None,
    )?;
    producer.send(&edits_topic, None, tm_fuzzy_match_payload).await?;

    // Group C: Multi-word match vs single-word match (query: "San Francisco")
    // tm_multi_word: name matches both words "San" and "Francisco" → should score highest
    // tm_single_word: name matches only "San" → should score lower
    let tm_multi_word_payload = edits::create_entity_edit(
        "Create TM Multi Word (San Francisco)",
        test_space,
        tm_multi_word_id,
        Some("San Francisco"),
        Some("A city in California"),
        None,
    )?;
    producer.send(&edits_topic, None, tm_multi_word_payload).await?;

    let tm_single_word_payload = edits::create_entity_edit(
        "Create TM Single Word (San Diego)",
        test_space,
        tm_single_word_id,
        Some("San Diego"),
        Some("A city in southern California"),
        None,
    )?;
    producer.send(&edits_topic, None, tm_single_word_payload).await?;

    // Group D: Name+description match vs name-only match (query: "Quantum")
    // tm_name_and_desc: name matches AND description also matches → should score highest
    // tm_name_only: name matches but description has no matching terms → should score lower
    let tm_name_and_desc_payload = edits::create_entity_edit(
        "Create TM Name+Desc (Quantum Computing)",
        test_space,
        tm_name_and_desc_id,
        Some("Quantum Computing"),
        Some("Quantum physics applied to computation"),
        None,
    )?;
    producer.send(&edits_topic, None, tm_name_and_desc_payload).await?;

    let tm_name_only_payload = edits::create_entity_edit(
        "Create TM Name Only (Quantum Mechanics)",
        test_space,
        tm_name_only_id,
        Some("Quantum Mechanics"),
        Some("The study of subatomic particles"),
        None,
    )?;
    producer.send(&edits_topic, None, tm_name_only_payload).await?;

    // Group E: High global score vs low global score, both match "Velociraptor" in name
    // tm_high_score: multi-word name containing query (slightly lower BM25 due to longer name)
    // tm_low_score: exact single-word name match (slightly higher BM25), but very low global score
    let tm_high_score_payload = edits::create_entity_edit(
        "Create TM High Score (Velociraptor Research)",
        test_space,
        tm_high_score_id,
        Some("Velociraptor Research"),
        Some("Academic center for dinosaur studies"),
        None,
    )?;
    producer.send(&edits_topic, None, tm_high_score_payload).await?;

    let tm_low_score_payload = edits::create_entity_edit(
        "Create TM Low Score (Velociraptor)",
        test_space,
        tm_low_score_id,
        Some("Velociraptor"),
        Some("A small feathered dinosaur"),
        None,
    )?;
    producer.send(&edits_topic, None, tm_low_score_payload).await?;

    // 8. Generate scores with varying values (Charlie intentionally excluded)
    info!("8. Generating scores with varying entity and space scores (Charlie has no global score)...");
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
            // Text match scoring test entities (all same score for fair textMatchScore comparison)
            (tm_name_match_id, 0.50),
            (tm_desc_match_id, 0.50),
            (tm_exact_match_id, 0.50),
            (tm_fuzzy_match_id, 0.50),
            (tm_multi_word_id, 0.50),
            (tm_single_word_id, 0.50),
            (tm_name_and_desc_id, 0.50),
            (tm_name_only_id, 0.50),
            // Group E: Different scores to test score boost outranking text match
            (tm_high_score_id, 0.90),   // High score, "Velociraptor Research"
            (tm_low_score_id, 0.20),    // Low score, "Velociraptor"
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
        .send(&scores_topic, None, score_payload)
        .await?;

    // 9. Create entities that will be soft deleted (for delete testing)
    info!("9. Creating entities for deletion testing...");
    let delete_charlie_payload_create = edits::create_entity_edit(
        "Create Delete Charlie",
        test_space,
        delete_charlie_id,
        Some("Delete Charlie"),
        Some("This entity will be deleted"),
        None,
    )?;
    producer.send(&edits_topic, None, delete_charlie_payload_create).await?;

    let delete_dana_payload_create = edits::create_entity_edit(
        "Create Delete Dana",
        test_space,
        delete_dana_id,
        Some("Delete Dana"),
        Some("This entity will also be deleted"),
        None,
    )?;
    producer.send(&edits_topic, None, delete_dana_payload_create).await?;

    let delete_eve_payload_create = edits::create_entity_edit(
        "Create Delete Eve",
        test_space,
        delete_eve_id,
        Some("Delete Eve"),
        Some("This entity will be deleted then updated"),
        None,
    )?;
    producer.send(&edits_topic, None, delete_eve_payload_create).await?;

    // 10. Delete the test entities (soft delete)
    info!("10. Soft deleting Delete Charlie, Delete Dana, and Delete Eve...");
    let delete_charlie_payload = edits::delete_entity(
        "Delete Delete Charlie",
        test_space,
        delete_charlie_id,
    )?;
    producer.send(&edits_topic, None, delete_charlie_payload).await?;

    let delete_dana_payload = edits::delete_entity(
        "Delete Delete Dana",
        test_space,
        delete_dana_id,
    )?;
    producer.send(&edits_topic, None, delete_dana_payload).await?;

    let delete_eve_payload = edits::delete_entity(
        "Delete Delete Eve",
        test_space,
        delete_eve_id,
    )?;
    producer.send(&edits_topic, None, delete_eve_payload).await?;

    // 11. Update a deleted entity (Delete Eve) - should remain deleted
    info!("11. Updating Delete Eve after deletion (testing delete-then-update behavior)...");
    let update_eve_payload = edits::create_entity_edit(
        "Update Delete Eve After Delete",
        test_space,
        delete_eve_id,
        Some("Delete Eve Updated"),
        Some("This entity was updated after being deleted - should remain deleted"),
        None,
    )?;
    producer.send(&edits_topic, None, update_eve_payload).await?;

    // 12. Test CreateEntity GRC-20 operation
    info!("12. Testing CreateEntity GRC-20 operation...");
    let create_entity_payload = edits::create_entity_grc20_op(
        "Create Entity via CreateEntity Op",
        test_space,
        create_entity_test_id,
        Some("CreateEntity Test"),
        Some("Entity created using the GRC-20 CreateEntity operation"),
        Some("https://example.com/create-entity-avatar.png"),
    )?;
    producer.send(&edits_topic, None, create_entity_payload).await?;

    // 11.1. Restore Delete Dana (testing restore after delete)
    info!("11.1. Restoring Delete Dana after deletion (testing restore behavior)...");
    let restore_dana_payload = edits::restore_entity(
        "Restore Delete Dana",
        test_space,
        delete_dana_id,
    )?;
    producer.send(&edits_topic, None, restore_dana_payload).await?;

    info!("\n✅ Test scenario complete!");
    info!("Created:");
    info!("  - 18 entities (13 active + 2 deleted + 3 unset test entities)");
    info!("    • 7 Alice variants (high, medium, low, zero, negative, at threshold, below threshold)");
    info!("    • Bob, Charlie, Acme Corp");
    info!("    • Person type, Organization type");
    info!("    • CreateEntity test entity (created via CreateEntity GRC-20 op)");
    info!("    • Charlie (soft deleted)");
    info!("    • Dana (soft deleted, then restored - active again)");
    info!("    • Eve (soft deleted, then updated - remains deleted)");
    info!("    • 3 unset property test entities");
    info!("  - Type relation scenarios:");
    info!("    • Alice High: Multiple types (Person + Organization)");
    info!("    • Alice Medium: Create->Delete->Create pattern (Person + Organization recreated)");
    info!("    • Alice Low: Partial type removal (Person kept, Org added + deleted)");
    info!("    • Other Alice entities, Bob, Charlie: Single type (Person)");
    info!("    • Acme Corp: Single type (Organization)");
    info!("  - 15 type relation events (11 creates, 2 deletes, 1 recreate for testing typeIds)");
    info!("  - 21 entity scores (including negative and zero)");
    info!("  - Charlie has NO global score (tests default score behavior)");
    info!("  - 10 text match scoring test entities (8 at score 0.50, 2 with different scores)");
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

    // 13. Test unset_properties functionality
    info!("\n13. Testing unset_properties functionality...");

    // Test Case 1: Unset 1 property (name)
    let unset_test_1_id = Uuid::parse_str("00000000-0000-0000-0000-000000001111").unwrap();
    info!("  Test Case 1: Create entity with name and description, then unset name");
    info!("    Entity ID: {}", unset_test_1_id);

    let unset_test_1_key = unset_test_1_id.to_string();
    let unset_test_1_create = edits::create_entity_edit(
        "Create Entity for Unset Test 1",
        test_space,
        unset_test_1_id,
        Some("Entity With Name To Unset"),
        Some("This entity will have its name unset"),
        None,
    )?;
    producer
        .send(&edits_topic, Some(&unset_test_1_key), unset_test_1_create)
        .await?;

    info!("    Unsetting name property...");
    let unset_name_payload = edits::unset_entity_properties(
        "Unset Name Property",
        test_space,
        unset_test_1_id,
        vec![sdk::core::ids::NAME_PROPERTY_ID],
    )?;
    producer
        .send(&edits_topic, Some(&unset_test_1_key), unset_name_payload)
        .await?;

    // Test Case 2: Unset 2 properties (name and description)
    let unset_test_2_id = Uuid::parse_str("00000000-0000-0000-0000-000000002222").unwrap();
    info!("  Test Case 2: Create entity with name, description, and avatar, then unset name and description");
    info!("    Entity ID: {}", unset_test_2_id);

    let unset_test_2_key = unset_test_2_id.to_string();
    let unset_test_2_create = edits::create_entity_edit(
        "Create Entity for Unset Test 2",
        test_space,
        unset_test_2_id,
        Some("Entity With Name And Description To Unset"),
        Some("This entity will have its name and description unset"),
        Some("https://example.com/avatar.png"),
    )?;
    producer
        .send(&edits_topic, Some(&unset_test_2_key), unset_test_2_create)
        .await?;

    info!("    Unsetting name and description properties...");
    let unset_name_desc_payload = edits::unset_entity_properties(
        "Unset Name and Description Properties",
        test_space,
        unset_test_2_id,
        vec![
            sdk::core::ids::NAME_PROPERTY_ID,
            sdk::core::ids::DESCRIPTION_PROPERTY_ID,
        ],
    )?;
    producer
        .send(&edits_topic, Some(&unset_test_2_key), unset_name_desc_payload)
        .await?;

    // Test Case 3: Mixed set/unset + LWW (Last-Writer-Wins) test
    let lww_test_id = Uuid::parse_str("00000000-0000-0000-0000-000000003333").unwrap();
    info!("  Test Case 3: Mixed set/unset in one operation + LWW with multiple sets");
    info!("    Entity ID: {}", lww_test_id);

    let lww_test_key = lww_test_id.to_string();
    let lww_test_create = edits::create_entity_edit(
        "Create Entity for LWW Test",
        test_space,
        lww_test_id,
        Some("Initial Name"),
        Some("Initial Description"),
        Some("https://example.com/lww-avatar.png"),
    )?;
    producer
        .send(&edits_topic, Some(&lww_test_key), lww_test_create)
        .await?;

    info!("    Step 1: Mixed operation - set name='First Update', unset description (different properties)...");
    let lww_mixed = edits::update_entity_with_set_and_unset(
        "LWW Mixed Set and Unset",
        test_space,
        lww_test_id,
        Some("First Update"),     // Set name to first value
        None,                      // Don't set description
        None,                      // Don't set avatar
        vec![
            sdk::core::ids::DESCRIPTION_PROPERTY_ID, // Unset description (no overlap with set)
        ],
    )?;
    producer
        .send(&edits_topic, Some(&lww_test_key), lww_mixed)
        .await?;

    info!("    Step 2: Set name again to 'Second Update' (LWW: this should win)...");
    let lww_second_set = edits::create_entity_edit(
        "LWW Second Set",
        test_space,
        lww_test_id,
        Some("Second Update"),    // Set name again - last write should win
        None,                      // Don't set description (remains unset)
        None,                      // Don't set avatar (keep existing)
    )?;
    producer
        .send(&edits_topic, Some(&lww_test_key), lww_second_set)
        .await?;

    info!("\n✅ Test scenario complete!");
    info!("Created:");
    info!("  - 15 entities (12 from before + 3 property operation test entities)");
    info!("    • 7 Alice variants (high, medium, low, zero, negative, at threshold, below threshold)");
    info!("    • Bob, Charlie, Acme Corp");
    info!("    • Person type, Organization type");
    info!("    • 2 entities for unset property testing");
    info!("  - Type relation scenarios:");
    info!("    • Alice High: Multiple types (Person + Organization)");
    info!("    • Alice Medium: Create->Delete->Create pattern (Person + Organization recreated)");
    info!("    • Alice Low: Partial type removal (Person kept, Org added + deleted)");
    info!("    • Other Alice entities, Bob, Charlie: Single type (Person)");
    info!("    • Acme Corp: Single type (Organization)");
    info!("  - 15 type relation events (11 creates, 2 deletes, 1 recreate for testing typeIds)");
    info!("  - 21 entity scores (including negative and zero)");
    info!("  - Charlie has NO global score (tests default score behavior)");
    info!("  - 10 text match scoring test entities (8 at score 0.50, 2 with different scores)");
    info!("  - 1 space score");
    info!("  - 7 perspective scores");
    info!("  - 3 property operation test cases:");
    info!("    • Test 1 ({}): name unset, description remains", unset_test_1_id);
    info!("    • Test 2 ({}): name and description unset, avatar remains", unset_test_2_id);
    info!("    • Test 3 ({}): mixed set/unset + LWW test (name='Second Update' wins, description unset)", lww_test_id);
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
