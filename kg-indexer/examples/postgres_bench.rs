//! Postgres batch write benchmark for kg-indexer.
//!
//! Measures batch insert throughput for entities, values, and relations
//! at different scales to inform performance budgets.
//!
//! Requires a running Postgres with the gaia schema.
//!
//! Run: DATABASE_URL=postgres://postgres:postgres@localhost:5432/gaia cargo run --example postgres_bench -p kg-indexer

use kg_indexer::error::IndexerError;
use kg_indexer::models::entities::EntityItem;
use kg_indexer::models::relations::SetRelationItem;
use kg_indexer::models::values::{ValueChangeType, ValueOp};
use kg_indexer::storage::Storage;
use sqlx::postgres::PgPoolCopyExt;
use std::time::Instant;
use uuid::Uuid;

fn generate_entities(count: usize) -> Vec<EntityItem> {
    (0..count)
        .map(|i| EntityItem {
            id: Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("entity-{}", i).as_bytes()),
            created_at: "1700000000".to_string(),
            created_at_block: "1000000".to_string(),
            updated_at: "1700000000".to_string(),
            updated_at_block: "1000000".to_string(),
        })
        .collect()
}

fn generate_values(count: usize, space_id: Uuid) -> Vec<ValueOp> {
    (0..count)
        .map(|i| {
            let entity_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("entity-{}", i % 1000).as_bytes());
            let property_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("property-{}", i % 100).as_bytes());
            ValueOp {
                id: Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("value-{}", i).as_bytes()),
                change_type: ValueChangeType::Set,
                entity_id,
                property_id,
                space_id,
                language: Some("en".to_string()),
                unit: None,
                string: Some(format!("Value {} with some content", i)),
                number: None,
                boolean: None,
                time: None,
                point: None,
            }
        })
        .collect()
}

fn generate_relations(count: usize, space_id: Uuid) -> Vec<SetRelationItem> {
    (0..count)
        .map(|i| {
            let from_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("entity-{}", i).as_bytes());
            let to_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("entity-{}", i + 1).as_bytes());
            let type_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"RELATES_TO");
            let entity_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("rel-entity-{}", i).as_bytes());
            SetRelationItem {
                id: Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("relation-{}", i).as_bytes()),
                entity_id,
                type_id,
                from_id,
                from_space_id: None,
                from_version_id: None,
                to_id,
                to_space_id: None,
                to_version_id: None,
                position: Some("a0".to_string()),
                space_id,
                verified: Some(true),
            }
        })
        .collect()
}

async fn cleanup_test_data(storage: &Storage) -> Result<(), IndexerError> {
    // Clean up test entities (cascade should handle values/relations)
    sqlx::query("DELETE FROM values WHERE id LIKE 'value-%' OR entity_id IN (SELECT id FROM entities WHERE created_at = '1700000000')")
        .execute(&storage.pool)
        .await?;
    sqlx::query("DELETE FROM relations WHERE entity_id IN (SELECT id FROM entities WHERE created_at = '1700000000')")
        .execute(&storage.pool)
        .await?;
    sqlx::query("DELETE FROM entities WHERE created_at = '1700000000'")
        .execute(&storage.pool)
        .await?;
    Ok(())
}

async fn bench_entities(storage: &Storage, counts: &[usize]) -> Result<(), IndexerError> {
    println!("\n### Entity Batch Insert");
    println!("| Count | Time (ms) | Throughput (rows/s) |");
    println!("|-------|-----------|---------------------|");

    for &count in counts {
        let entities = generate_entities(count);

        let mut tx = storage.pool.begin().await?;
        let start = Instant::now();
        storage.insert_entities(&entities, &mut tx).await?;
        tx.commit().await?;
        let elapsed = start.elapsed();

        let ms = elapsed.as_secs_f64() * 1000.0;
        let throughput = count as f64 / elapsed.as_secs_f64();
        println!("| {} | {:.2} | {:.0} |", count, ms, throughput);

        // Cleanup between runs
        cleanup_test_data(storage).await?;
    }

    Ok(())
}

async fn bench_values(storage: &Storage, counts: &[usize]) -> Result<(), IndexerError> {
    println!("\n### Value Batch Insert");
    println!("| Count | Time (ms) | Throughput (rows/s) |");
    println!("|-------|-----------|---------------------|");

    let space_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"bench-space");

    // Pre-create entities that values reference
    let entities = generate_entities(1000);
    let mut tx = storage.pool.begin().await?;
    storage.insert_entities(&entities, &mut tx).await?;
    tx.commit().await?;

    for &count in counts {
        let values = generate_values(count, space_id);

        let mut tx = storage.pool.begin().await?;
        let start = Instant::now();
        storage.insert_values(&values, &mut tx).await?;
        tx.commit().await?;
        let elapsed = start.elapsed();

        let ms = elapsed.as_secs_f64() * 1000.0;
        let throughput = count as f64 / elapsed.as_secs_f64();
        println!("| {} | {:.2} | {:.0} |", count, ms, throughput);

        // Cleanup values between runs (keep entities)
        sqlx::query("DELETE FROM values WHERE space_id = $1")
            .bind(space_id)
            .execute(&storage.pool)
            .await?;
    }

    // Final cleanup
    cleanup_test_data(storage).await?;

    Ok(())
}

async fn bench_relations(storage: &Storage, counts: &[usize]) -> Result<(), IndexerError> {
    println!("\n### Relation Batch Insert");
    println!("| Count | Time (ms) | Throughput (rows/s) |");
    println!("|-------|-----------|---------------------|");

    let space_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"bench-space");

    for &count in counts {
        // Pre-create entities that relations reference
        let entities = generate_entities(count + 1);
        let mut tx = storage.pool.begin().await?;
        storage.insert_entities(&entities, &mut tx).await?;
        tx.commit().await?;

        let relations = generate_relations(count, space_id);

        let mut tx = storage.pool.begin().await?;
        let start = Instant::now();
        storage.insert_relations(&relations, &mut tx).await?;
        tx.commit().await?;
        let elapsed = start.elapsed();

        let ms = elapsed.as_secs_f64() * 1000.0;
        let throughput = count as f64 / elapsed.as_secs_f64();
        println!("| {} | {:.2} | {:.0} |", count, ms, throughput);

        // Cleanup between runs
        sqlx::query("DELETE FROM relations WHERE space_id = $1")
            .bind(space_id)
            .execute(&storage.pool)
            .await?;
        cleanup_test_data(storage).await?;
    }

    Ok(())
}

/// Serialize values to Postgres binary COPY format
/// Returns the binary buffer ready for COPY
fn serialize_values_to_binary(values: &[ValueOp]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(values.len() * 200); // estimate ~200 bytes per row

    // Binary COPY header: "PGCOPY\n\xff\r\n\0" + flags (4 bytes) + header extension (4 bytes)
    buf.extend_from_slice(b"PGCOPY\n\xff\r\n\0");
    buf.extend_from_slice(&0i32.to_be_bytes()); // flags
    buf.extend_from_slice(&0i32.to_be_bytes()); // header extension length

    for value in values {
        // Number of fields (11 columns)
        buf.extend_from_slice(&11i16.to_be_bytes());

        // id (text)
        let id_str = value.id.to_string();
        let id_bytes = id_str.as_bytes();
        buf.extend_from_slice(&(id_bytes.len() as i32).to_be_bytes());
        buf.extend_from_slice(id_bytes);

        // entity_id (uuid - 16 bytes)
        buf.extend_from_slice(&16i32.to_be_bytes());
        buf.extend_from_slice(value.entity_id.as_bytes());

        // property_id (uuid)
        buf.extend_from_slice(&16i32.to_be_bytes());
        buf.extend_from_slice(value.property_id.as_bytes());

        // space_id (uuid)
        buf.extend_from_slice(&16i32.to_be_bytes());
        buf.extend_from_slice(value.space_id.as_bytes());

        // string (text, nullable)
        match &value.string {
            Some(s) => {
                let bytes = s.as_bytes();
                buf.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                buf.extend_from_slice(bytes);
            }
            None => buf.extend_from_slice(&(-1i32).to_be_bytes()), // NULL
        }

        // language (text, nullable)
        match &value.language {
            Some(s) => {
                let bytes = s.as_bytes();
                buf.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                buf.extend_from_slice(bytes);
            }
            None => buf.extend_from_slice(&(-1i32).to_be_bytes()),
        }

        // unit (text, nullable)
        match &value.unit {
            Some(s) => {
                let bytes = s.as_bytes();
                buf.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                buf.extend_from_slice(bytes);
            }
            None => buf.extend_from_slice(&(-1i32).to_be_bytes()),
        }

        // boolean (nullable)
        match value.boolean {
            Some(b) => {
                buf.extend_from_slice(&1i32.to_be_bytes());
                buf.push(if b { 1 } else { 0 });
            }
            None => buf.extend_from_slice(&(-1i32).to_be_bytes()),
        }

        // number (numeric - complex encoding, use NULL for benchmark simplicity)
        buf.extend_from_slice(&(-1i32).to_be_bytes()); // NULL for now

        // point (text, nullable)
        match &value.point {
            Some(s) => {
                let bytes = s.as_bytes();
                buf.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                buf.extend_from_slice(bytes);
            }
            None => buf.extend_from_slice(&(-1i32).to_be_bytes()),
        }

        // time (text, nullable)
        match &value.time {
            Some(s) => {
                let bytes = s.as_bytes();
                buf.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                buf.extend_from_slice(bytes);
            }
            None => buf.extend_from_slice(&(-1i32).to_be_bytes()),
        }
    }

    // Binary COPY trailer: -1 as i16
    buf.extend_from_slice(&(-1i16).to_be_bytes());

    buf
}

/// Benchmark: CPU cost of serializing to binary format
fn bench_binary_serialization(counts: &[usize]) {
    println!("\n### Binary Serialization (CPU only)");
    println!("| Count | Time (ms) | Throughput (rows/s) | Size (MB) |");
    println!("|-------|-----------|---------------------|-----------|");

    let space_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"bench-space");

    for &count in counts {
        let values = generate_values(count, space_id);

        let start = Instant::now();
        let binary_data = serialize_values_to_binary(&values);
        let elapsed = start.elapsed();

        let ms = elapsed.as_secs_f64() * 1000.0;
        let throughput = count as f64 / elapsed.as_secs_f64();
        let size_mb = binary_data.len() as f64 / (1024.0 * 1024.0);
        println!("| {} | {:.2} | {:.0} | {:.2} |", count, ms, throughput, size_mb);
    }
}

/// Benchmark: Binary COPY to staging table (no indexes, no constraints)
async fn bench_binary_copy(storage: &Storage, counts: &[usize]) -> Result<(), IndexerError> {
    println!("\n### Binary COPY to Staging Table (no indexes)");
    println!("| Count | Time (ms) | Throughput (rows/s) |");
    println!("|-------|-----------|---------------------|");

    let space_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"bench-space");

    // Create unlogged staging table with no indexes
    sqlx::query(
        "CREATE UNLOGGED TABLE IF NOT EXISTS values_staging (
            id text,
            entity_id uuid,
            property_id uuid,
            space_id uuid,
            string text,
            language text,
            unit text,
            boolean boolean,
            number numeric,
            point text,
            time text
        )"
    )
    .execute(&storage.pool)
    .await?;

    for &count in counts {
        let values = generate_values(count, space_id);

        // Pre-serialize (not counted in timing)
        let binary_data = serialize_values_to_binary(&values);

        // Truncate staging table
        sqlx::query("TRUNCATE values_staging")
            .execute(&storage.pool)
            .await?;

        let start = Instant::now();

        // Use raw COPY with binary format
        let mut copy = storage.pool
            .copy_in_raw("COPY values_staging (id, entity_id, property_id, space_id, string, language, unit, boolean, number, point, time) FROM STDIN WITH (FORMAT binary)")
            .await?;

        copy.send(binary_data).await?;
        copy.finish().await?;

        let elapsed = start.elapsed();

        let ms = elapsed.as_secs_f64() * 1000.0;
        let throughput = count as f64 / elapsed.as_secs_f64();
        println!("| {} | {:.2} | {:.0} |", count, ms, throughput);
    }

    // Cleanup
    sqlx::query("DROP TABLE IF EXISTS values_staging")
        .execute(&storage.pool)
        .await?;

    Ok(())
}

/// Benchmark: Binary COPY directly to indexed table (no upsert, fresh inserts)
async fn bench_copy_indexed(storage: &Storage, counts: &[usize]) -> Result<(), IndexerError> {
    println!("\n### Binary COPY to Indexed Table (no upsert)");
    println!("| Count | Time (ms) | Throughput (rows/s) |");
    println!("|-------|-----------|---------------------|");

    let space_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"bench-space-copy");

    // Pre-create entities
    let entities = generate_entities(1000);
    let mut tx = storage.pool.begin().await?;
    storage.insert_entities(&entities, &mut tx).await?;
    tx.commit().await?;

    for &count in counts {
        // Generate unique values for each run (no conflicts)
        let values: Vec<ValueOp> = (0..count)
            .map(|i| {
                let entity_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("entity-{}", i % 1000).as_bytes());
                let property_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("property-{}", i % 100).as_bytes());
                ValueOp {
                    // Unique ID per run to avoid conflicts
                    id: Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("copy-value-{}-{}", count, i).as_bytes()),
                    change_type: ValueChangeType::Set,
                    entity_id,
                    property_id,
                    space_id,
                    language: Some("en".to_string()),
                    unit: None,
                    string: Some(format!("Value {} with some content", i)),
                    number: None,
                    boolean: None,
                    time: None,
                    point: None,
                }
            })
            .collect();

        let binary_data = serialize_values_to_binary(&values);

        let start = Instant::now();

        // COPY directly to the real values table (with indexes)
        let mut copy = storage.pool
            .copy_in_raw("COPY values (id, entity_id, property_id, space_id, string, language, unit, boolean, number, point, time) FROM STDIN WITH (FORMAT binary)")
            .await?;
        copy.send(binary_data).await?;
        copy.finish().await?;

        let elapsed = start.elapsed();

        let ms = elapsed.as_secs_f64() * 1000.0;
        let throughput = count as f64 / elapsed.as_secs_f64();
        println!("| {} | {:.2} | {:.0} |", count, ms, throughput);

        // Cleanup
        sqlx::query("DELETE FROM values WHERE space_id = $1")
            .bind(space_id)
            .execute(&storage.pool)
            .await?;
    }

    cleanup_test_data(storage).await?;

    Ok(())
}

/// Benchmark: Full COPY + upsert pipeline (staging -> real table)
async fn bench_copy_upsert(storage: &Storage, counts: &[usize]) -> Result<(), IndexerError> {
    println!("\n### Binary COPY + Upsert (full pipeline)");
    println!("| Count | Time (ms) | Throughput (rows/s) |");
    println!("|-------|-----------|---------------------|");

    let space_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"bench-space");

    // Pre-create entities
    let entities = generate_entities(1000);
    let mut tx = storage.pool.begin().await?;
    storage.insert_entities(&entities, &mut tx).await?;
    tx.commit().await?;

    // Create unlogged staging table
    sqlx::query(
        "CREATE UNLOGGED TABLE IF NOT EXISTS values_staging (
            id text,
            entity_id uuid,
            property_id uuid,
            space_id uuid,
            string text,
            language text,
            unit text,
            boolean boolean,
            number numeric,
            point text,
            time text
        )"
    )
    .execute(&storage.pool)
    .await?;

    for &count in counts {
        let values = generate_values(count, space_id);
        let binary_data = serialize_values_to_binary(&values);

        sqlx::query("TRUNCATE values_staging")
            .execute(&storage.pool)
            .await?;

        let start = Instant::now();

        // Step 1: COPY to staging
        let mut copy = storage.pool
            .copy_in_raw("COPY values_staging (id, entity_id, property_id, space_id, string, language, unit, boolean, number, point, time) FROM STDIN WITH (FORMAT binary)")
            .await?;
        copy.send(binary_data).await?;
        copy.finish().await?;

        // Step 2: Upsert from staging to real table
        sqlx::query(
            "INSERT INTO values (id, entity_id, property_id, space_id, string, language, unit, boolean, number, point, time)
             SELECT id, entity_id, property_id, space_id, string, language, unit, boolean, number, point, time
             FROM values_staging
             ON CONFLICT (id) DO UPDATE SET
                 string = EXCLUDED.string,
                 language = EXCLUDED.language,
                 unit = EXCLUDED.unit,
                 boolean = EXCLUDED.boolean,
                 number = EXCLUDED.number,
                 point = EXCLUDED.point,
                 time = EXCLUDED.time"
        )
        .execute(&storage.pool)
        .await?;

        let elapsed = start.elapsed();

        let ms = elapsed.as_secs_f64() * 1000.0;
        let throughput = count as f64 / elapsed.as_secs_f64();
        println!("| {} | {:.2} | {:.0} |", count, ms, throughput);

        // Cleanup values
        sqlx::query("DELETE FROM values WHERE space_id = $1")
            .bind(space_id)
            .execute(&storage.pool)
            .await?;
    }

    // Cleanup
    sqlx::query("DROP TABLE IF EXISTS values_staging")
        .execute(&storage.pool)
        .await?;
    cleanup_test_data(storage).await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/gaia".to_string());

    println!("Connecting to: {}", database_url);
    let storage = Storage::new(&database_url).await?;

    println!("# Postgres Batch Write Benchmarks");
    println!("\nMeasuring batch insert throughput for kg-indexer storage operations.");
    println!("Target: < 62ms for I/O budget (250ms block time - 188ms CPU)");

    let counts = vec![1_000, 5_000, 10_000, 50_000, 100_000];

    bench_entities(&storage, &counts).await?;
    bench_values(&storage, &counts).await?;
    bench_relations(&storage, &counts).await?;

    println!("\n## Binary COPY Benchmarks");

    // CPU-only: serialization cost
    bench_binary_serialization(&counts);

    // I/O: COPY to unlogged staging table (theoretical max - no indexes)
    bench_binary_copy(&storage, &counts).await?;

    // COPY directly to indexed table (no upsert, just insert)
    bench_copy_indexed(&storage, &counts).await?;

    // Full pipeline: COPY + upsert (realistic with ON CONFLICT)
    bench_copy_upsert(&storage, &counts).await?;

    println!("\n## Summary");
    println!("Bottleneck analysis: Compare throughput against ops/block estimates.");
    println!("At 250K ops/block with ~62ms I/O budget: need ~4M rows/sec aggregate.");

    Ok(())
}
