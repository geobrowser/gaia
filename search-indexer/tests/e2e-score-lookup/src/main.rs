//! E2E test for score indexing via Postgres lookup + bulk doc ID updates.
//!
//! This test:
//! 1. Seeds Postgres `values` table with entity-space mappings
//! 2. Seeds OpenSearch with entity documents directly (bypassing Kafka edits)
//! 3. Produces score update events to Kafka
//! 4. Waits for the search-indexer to process the scores
//! 5. Queries OpenSearch to verify scores were applied correctly
//!
//! Requires: Postgres, Kafka, OpenSearch, and the search-indexer running externally.

use prost::Message;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use sqlx::PgPool;
use std::env;
use std::time::Duration;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

const SPACE_A: &str = "e2e00000-0000-4000-8000-00000000aa01";
const SPACE_B: &str = "e2e00000-0000-4000-8000-00000000bb01";
const SPACE_C: &str = "e2e00000-0000-4000-8000-00000000cc01";

fn space_a() -> Uuid { Uuid::parse_str(SPACE_A).expect("valid") }
fn space_b() -> Uuid { Uuid::parse_str(SPACE_B).expect("valid") }
fn space_c() -> Uuid { Uuid::parse_str(SPACE_C).expect("valid") }

fn entity(prefix: u8, index: u16) -> Uuid {
    Uuid::parse_str(&format!("e2e00000-0000-0000-0000-0000000{:01x}{:04x}", prefix, index))
        .expect("valid")
}

/// All (entity_id, space_id) pairs for the test.
fn test_pairs() -> Vec<(Uuid, Uuid)> {
    let mut pairs = Vec::new();
    // Space A: 5 entities
    for i in 1..=5 { pairs.push((entity(0xa, i), space_a())); }
    // Space B: 3 entities
    for i in 1..=3 { pairs.push((entity(0xb, i), space_b())); }
    // Space C: 2 entities
    for i in 1..=2 { pairs.push((entity(0xc, i), space_c())); }
    // Multi-space entity: exists in both A and B
    let multi = entity(0xd, 1);
    pairs.push((multi, space_a()));
    pairs.push((multi, space_b()));
    pairs
}

fn doc_id(entity_id: &Uuid, space_id: &Uuid) -> String {
    format!("{}_{}", entity_id, space_id)
}

// ---------------------------------------------------------------------------
// Database seeding
// ---------------------------------------------------------------------------

async fn seed_postgres(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM values WHERE id LIKE 'e2e-%'")
        .execute(pool)
        .await?;

    let prop_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").expect("valid");

    for (eid, sid) in test_pairs() {
        let vid = format!("e2e-{}-{}", eid, sid);
        sqlx::query(
            "INSERT INTO values (id, entity_id, property_id, space_id, text) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (id) DO NOTHING",
        )
        .bind(&vid)
        .bind(eid)
        .bind(prop_id)
        .bind(sid)
        .bind(format!("Entity {}", eid))
        .execute(pool)
        .await?;
    }

    println!("  Seeded {} pairs in Postgres", test_pairs().len());
    Ok(())
}

// ---------------------------------------------------------------------------
// OpenSearch seeding
// ---------------------------------------------------------------------------

async fn seed_opensearch(
    url: &str,
    index: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()?;

    // Create index if it doesn't exist (minimal mapping)
    let _ = client
        .put(&format!("{}/{}", url, index))
        .json(&serde_json::json!({
            "mappings": {
                "properties": {
                    "entity_id": { "type": "keyword" },
                    "space_id": { "type": "keyword" },
                    "name": { "type": "text" },
                    "entity_global_score": { "type": "float" },
                    "space_score": { "type": "float" }
                }
            }
        }))
        .send()
        .await;

    // Bulk-insert test documents
    let mut bulk_body = String::new();
    for (eid, sid) in test_pairs() {
        let did = doc_id(&eid, &sid);
        bulk_body.push_str(&serde_json::to_string(&serde_json::json!({
            "index": { "_index": index, "_id": did }
        }))?);
        bulk_body.push('\n');
        bulk_body.push_str(&serde_json::to_string(&serde_json::json!({
            "entity_id": eid.to_string(),
            "space_id": sid.to_string(),
            "name": format!("Test Entity {}", eid)
        }))?);
        bulk_body.push('\n');
    }

    let resp = client
        .post(&format!("{}/_bulk", url))
        .header("Content-Type", "application/x-ndjson")
        .body(bulk_body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let body = resp.text().await?;
        return Err(format!("Bulk insert failed: {}", body).into());
    }

    // Refresh to make documents searchable
    client
        .post(&format!("{}/{}/_refresh", url, index))
        .send()
        .await?;

    println!("  Seeded {} documents in OpenSearch", test_pairs().len());
    Ok(())
}

// ---------------------------------------------------------------------------
// Kafka event production
// ---------------------------------------------------------------------------

/// The scores consumer uses `{environment}.curation.scores` (not hermes_kafka prefix convention).
fn scores_topic() -> String {
    let env = env::var("ENVIRONMENT").unwrap_or_else(|_| "production".to_string());
    format!("{}.curation.scores", env)
}

async fn produce_score_events(
    producer: &FutureProducer,
    topic: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entity_scores = Vec::new();
    let mut space_scores = Vec::new();

    // Unique entity global scores
    let mut seen = std::collections::HashSet::new();
    for (eid, _) in test_pairs() {
        if seen.insert(eid) {
            entity_scores.push(hermes_schema::pb::scoring::EntityScore {
                entity_id: eid.as_bytes().to_vec(),
                score: 0.75,
                updated_at: 1700000000,
            });
        }
    }

    // Space scores
    for (sid, score) in [(space_a(), 0.9), (space_b(), 0.5), (space_c(), 0.3)] {
        space_scores.push(hermes_schema::pb::scoring::SpaceScore {
            space_id: sid.as_bytes().to_vec(),
            score,
            updated_at: 1700000000,
        });
    }

    let batch = hermes_schema::pb::scoring::HermesScoresBatch {
        entity_scores: entity_scores.clone(),
        perspective_scores: vec![],
        space_scores,
        computed_at: 1700000000,
        batch_sequence: 1,
        is_final: true,
    };

    let payload = batch.encode_to_vec();
    producer
        .send(
            FutureRecord::to(topic)
                .payload(&payload)
                .key("e2e-scores"),
            Duration::from_secs(10),
        )
        .await
        .map_err(|(e, _)| e)?;

    println!(
        "  Produced score batch ({} entity + 3 space scores)",
        entity_scores.len()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct SearchResponse {
    hits: SearchHits,
}

#[derive(serde::Deserialize)]
struct SearchHits {
    hits: Vec<SearchHit>,
}

#[derive(serde::Deserialize)]
struct SearchHit {
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_source")]
    source: serde_json::Value,
}

async fn verify_scores(
    url: &str,
    index: &str,
) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()?;

    // Refresh first
    client.post(&format!("{}/{}/_refresh", url, index)).send().await?;

    let mut passed = 0u32;
    let mut failed = 0u32;

    // 1. Check entity_global_score = 0.75 on all test docs
    let resp: SearchResponse = client
        .post(&format!("{}/{}/_search", url, index))
        .json(&serde_json::json!({
            "size": 20,
            "query": {
                "prefix": { "entity_id": "e2e00000-" }
            },
            "_source": ["entity_id", "space_id", "entity_global_score", "space_score"]
        }))
        .send()
        .await?
        .json()
        .await?;

    let total_docs = resp.hits.hits.len();
    let docs_with_global_score = resp.hits.hits.iter().filter(|h| {
        h.source["entity_global_score"]
            .as_f64()
            .map_or(false, |s| (s - 0.75).abs() < 0.01)
    }).count();

    if docs_with_global_score == total_docs && total_docs == test_pairs().len() {
        println!("  PASS: all {} docs have entity_global_score=0.75", total_docs);
        passed += 1;
    } else {
        eprintln!(
            "  FAIL: {}/{} docs have entity_global_score=0.75 (expected {})",
            docs_with_global_score, total_docs, test_pairs().len()
        );
        failed += 1;
    }

    // 2. Check space_score per space
    for (sid, expected, name) in [
        (space_a(), 0.9, "A"),
        (space_b(), 0.5, "B"),
        (space_c(), 0.3, "C"),
    ] {
        let resp: SearchResponse = client
            .post(&format!("{}/{}/_search", url, index))
            .json(&serde_json::json!({
                "size": 10,
                "query": { "term": { "space_id": sid.to_string() } },
                "_source": ["space_id", "space_score"]
            }))
            .send()
            .await?
            .json()
            .await?;

        let all_correct = !resp.hits.hits.is_empty()
            && resp.hits.hits.iter().all(|h| {
                h.source["space_score"]
                    .as_f64()
                    .map_or(false, |s| (s - expected).abs() < 0.01)
            });

        if all_correct {
            println!("  PASS: space {} — all docs have space_score={:.1}", name, expected);
            passed += 1;
        } else {
            eprintln!("  FAIL: space {} — not all docs have space_score={:.1}", name, expected);
            for h in &resp.hits.hits {
                eprintln!("    doc {}: space_score={:?}", h.id, h.source["space_score"]);
            }
            failed += 1;
        }
    }

    // 3. Multi-space entity should have score in both spaces
    let multi = entity(0xd, 1);
    let resp: SearchResponse = client
        .post(&format!("{}/{}/_search", url, index))
        .json(&serde_json::json!({
            "size": 5,
            "query": { "term": { "entity_id": multi.to_string() } },
            "_source": ["entity_id", "space_id", "entity_global_score", "space_score"]
        }))
        .send()
        .await?
        .json()
        .await?;

    if resp.hits.hits.len() >= 2 {
        println!("  PASS: multi-space entity has {} docs", resp.hits.hits.len());
        passed += 1;

        let all_have_score = resp.hits.hits.iter().all(|h| {
            h.source["entity_global_score"].as_f64() == Some(0.75)
        });
        if all_have_score {
            println!("  PASS: multi-space entity has entity_global_score in all spaces");
            passed += 1;
        } else {
            eprintln!("  FAIL: multi-space entity missing entity_global_score in some spaces");
            failed += 1;
        }

        // Check different space_scores on the two docs
        let has_different_space_scores = {
            let scores: Vec<f64> = resp.hits.hits.iter()
                .filter_map(|h| h.source["space_score"].as_f64())
                .collect();
            scores.len() == 2 && (scores[0] - scores[1]).abs() > 0.01
        };
        if has_different_space_scores {
            println!("  PASS: multi-space entity has different space_scores per space");
            passed += 1;
        } else {
            eprintln!("  FAIL: multi-space entity should have different space_scores (A=0.9, B=0.5)");
            failed += 1;
        }
    } else {
        eprintln!("  FAIL: multi-space entity has {} docs (expected 2+)", resp.hits.hits.len());
        failed += 1;
    }

    Ok((passed, failed))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let kafka_broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let opensearch_url =
        env::var("OPENSEARCH_URL").unwrap_or_else(|_| "http://localhost:9200".to_string());
    let index = env::var("INDEX_ALIAS").unwrap_or_else(|_| "entities".to_string());
    let timeout_secs: u64 = env::var("E2E_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);

    println!("=== Score Lookup E2E Test ===");
    println!("  database:   {}", database_url);
    println!("  kafka:      {}", kafka_broker);
    println!("  opensearch: {}", opensearch_url);
    println!("  index:      {}", index);
    println!("  timeout:    {}s\n", timeout_secs);

    // 1. Seed Postgres
    println!("[1/5] Seeding Postgres...");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to Postgres");
    seed_postgres(&pool)
        .await
        .expect("Failed to seed Postgres");

    // 2. Seed OpenSearch with documents
    println!("[2/5] Seeding OpenSearch...");
    seed_opensearch(&opensearch_url, &index)
        .await
        .expect("Failed to seed OpenSearch");

    // 3. Produce score events to Kafka
    println!("[3/5] Producing score events...");
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &kafka_broker)
        .set("message.timeout.ms", "30000")
        .create()
        .expect("Failed to create Kafka producer");

    let scores_topic = scores_topic();
    produce_score_events(&producer, &scores_topic)
        .await
        .expect("Failed to produce score events");

    // 4. Wait for processing
    let wait_secs = timeout_secs / 2;
    println!("[4/5] Waiting {}s for search-indexer to process...", wait_secs);
    tokio::time::sleep(Duration::from_secs(wait_secs)).await;

    // 5. Verify
    println!("[5/5] Verifying scores in OpenSearch...\n");
    let (passed, failed) = verify_scores(&opensearch_url, &index)
        .await
        .expect("Failed to verify");

    println!("\n=== Results: {} passed, {} failed ===", passed, failed);

    if failed > 0 {
        std::process::exit(1);
    }

    println!("\nAll score lookup e2e tests passed!");
}
