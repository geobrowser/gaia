//! Query benchmark for kg-indexer.
//!
//! Tests query performance for common access patterns derived from GraphQL API.
//! Run with different index configurations to find the minimal required set.
//!
//! ## Workflow
//!
//! ```bash
//! # 1. Generate CSV files (one-time, ~30s for 700K entities)
//! cargo run --example query_bench -p kg-indexer --release -- --dump
//!
//! # 2. Load via psql (one-time, ~10-15s)
//! psql $DATABASE_URL -c "\copy entities(id,created_at,created_at_block,updated_at,updated_at_block) FROM 'bench_data/entities.csv'"
//! psql $DATABASE_URL -c "\copy values(id,entity_id,property_id,space_id,string,number,time) FROM 'bench_data/values.csv'"
//! psql $DATABASE_URL -c "\copy relations(id,entity_id,type_id,from_entity_id,to_entity_id,space_id,position) FROM 'bench_data/relations.csv'"
//! psql $DATABASE_URL -c "ANALYZE entities, values, relations"
//!
//! # 3. Run benchmarks (instant, repeatable)
//! cargo run --example query_bench -p kg-indexer --release -- --query-only
//!
//! # 4. Modify indexes and re-run
//! psql $DATABASE_URL -c "DROP INDEX values_foo_idx"
//! cargo run --example query_bench -p kg-indexer --release -- --query-only
//! ```
//!
//! ## Configuration (env vars)
//!   ENTITY_COUNT=700000  (default 700K to match production)
//!   VALUES_PER_ENTITY=3
//!   RELATIONS_PER_ENTITY=3
//!   NUM_SPACES=50
//!   NUM_TYPES=100
//!   QUERY_ITERATIONS=100
//!   DATA_DIR=./bench_data

use rand::prelude::*;
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;
use uuid::Uuid;

/// Production distributions from real database (sampled 2026-01-09)
/// These create realistic skewed distributions for benchmarking.
struct ProdDistributions {
    /// Property IDs with their relative weights (from production counts)
    properties: Vec<(Uuid, u32)>,
    /// Space IDs with their relative weights
    spaces: Vec<(Uuid, u32)>,
    /// Relation type IDs with their relative weights
    relation_types: Vec<(Uuid, u32)>,
}

impl ProdDistributions {
    fn new() -> Self {
        // Top 18 properties from prod (covers 98%+ of values)
        let properties = vec![
            ("a126ca53-0c8e-48d5-b888-82c734c38935", 40439), // name
            ("8a743832-c094-4a62-b665-0c3cc2f9c7bc", 6501),
            ("5e92c8a4-1714-4ee7-9a09-389ef4336aeb", 6183),
            ("9f1e43fd-63e3-4bde-97b9-b3d88c57031b", 6183),
            ("9b1f76ff-9711-404c-861e-59dc3fa7d037", 5665), // description
            ("eed38e74-e679-46bf-8a42-ea3e4f8fb5fb", 4179),
            ("0d625978-4b3c-4b57-a86f-de45c997c73c", 2671),
            ("e3e363d1-dd29-4ccb-8e6f-f3b76d99bc33", 2343),
            ("77999397-f78d-44a7-bbc5-d93a617af47c", 1072),
            ("87f919d5-560b-408c-be8d-318e2c5c098b", 1068),
            ("76996acc-d10f-4cd5-9ac9-4a705b8e03b4", 962),
            ("14a46854-bfd1-4b18-8215-2785c2dab9f3", 782),
            ("412ff593-e915-4012-a43d-4c27ec5c68b6", 495),
            ("9b5eced9-5c30-473b-8404-f474a777db3a", 272),
            ("94e43fe8-faf2-4100-9eb8-87ab4f999723", 213),
            ("64695ccd-c5ea-4185-87f8-7e335dc1b66b", 179),
            ("78ec09b9-f56f-4898-8db8-6c7f153774f3", 177),
            ("2d696bf0-510f-403e-985b-8cd1e73feb9b", 159),
        ];

        // Top 8 spaces from prod (covers 98%+ of values)
        let spaces = vec![
            ("e252f9e1-d3ad-4460-8bf1-54f93b02f220", 59132),
            ("021265e2-d839-47c3-8d03-0ee3dfb29ffc", 14056),
            ("ff79e3b1-7627-4a09-a55f-42a7acacaef4", 3441),
            ("539a2d1b-0bf6-413e-af5a-46cee9550727", 1321),
            ("2a98e6b4-3728-44a4-9b8e-02e15f0677c8", 848),
            ("701c7637-f5f7-468d-82c6-c1ebca496f74", 169),
            ("b5dab047-d3cf-460c-adc6-f6cbc112826f", 143),
            ("c401811d-701a-4d9b-9e32-9db3ff4c7da6", 90),
        ];

        // Top 10 relation types from prod (covers 90%+ of relations)
        let relation_types = vec![
            ("8f151ba4-de20-4e3c-9cb4-99ddf96f48f1", 65170),
            ("e1371bcd-a704-4396-adb7-ea7ecc8fe3d4", 22129),
            ("458fbc07-0dbf-4c92-8f57-16f3fdde7c32", 16110),
            ("8fcfe5ef-3d91-47bd-8322-3830a998d26b", 8732),
            ("49c5d5e1-679a-4dbd-bfd3-3f618f227c94", 5976),
            ("1155beff-fad5-49b7-a2e0-da4777b8792c", 5084),
            ("1ff59132-2d57-4671-934a-7b662e3cf66a", 3617),
            ("beaba5cb-a677-41a8-b353-77030613fc70", 3107),
            ("1367bac7-dcea-4b80-86ad-a4a4cdd7c2cb", 2128),
            ("4b5bbddf-32b2-47ba-b0a6-dbbab27f457d", 1937),
        ];

        Self {
            properties: properties
                .into_iter()
                .map(|(id, w)| (Uuid::parse_str(id).unwrap(), w))
                .collect(),
            spaces: spaces
                .into_iter()
                .map(|(id, w)| (Uuid::parse_str(id).unwrap(), w))
                .collect(),
            relation_types: relation_types
                .into_iter()
                .map(|(id, w)| (Uuid::parse_str(id).unwrap(), w))
                .collect(),
        }
    }

    /// Pick a random property weighted by production distribution
    fn pick_property(&self, rng: &mut impl Rng) -> Uuid {
        self.weighted_pick(&self.properties, rng)
    }

    /// Pick a random space weighted by production distribution
    fn pick_space(&self, rng: &mut impl Rng) -> Uuid {
        self.weighted_pick(&self.spaces, rng)
    }

    /// Pick a random relation type weighted by production distribution
    fn pick_relation_type(&self, rng: &mut impl Rng) -> Uuid {
        self.weighted_pick(&self.relation_types, rng)
    }

    fn weighted_pick(&self, items: &[(Uuid, u32)], rng: &mut impl Rng) -> Uuid {
        let total: u32 = items.iter().map(|(_, w)| w).sum();
        let mut pick = rng.gen_range(0..total);
        for (id, weight) in items {
            if pick < *weight {
                return *id;
            }
            pick -= weight;
        }
        items[0].0 // fallback
    }
}

struct Config {
    entity_count: usize,
    values_per_entity: usize,
    relations_per_entity: usize,
    query_iterations: usize,
    num_spaces: usize,
    num_types: usize,
    data_dir: PathBuf,
}

impl Config {
    fn from_env() -> Self {
        Self {
            entity_count: std::env::var("ENTITY_COUNT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(700_000), // Match production
            values_per_entity: std::env::var("VALUES_PER_ENTITY")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            relations_per_entity: std::env::var("RELATIONS_PER_ENTITY")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            query_iterations: std::env::var("QUERY_ITERATIONS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
            num_spaces: std::env::var("NUM_SPACES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(50),
            num_types: std::env::var("NUM_TYPES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
            data_dir: std::env::var("DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("./bench_data")),
        }
    }

    fn entities_csv(&self) -> PathBuf {
        self.data_dir.join("entities.csv")
    }

    fn values_csv(&self) -> PathBuf {
        self.data_dir.join("values.csv")
    }

    fn relations_csv(&self) -> PathBuf {
        self.data_dir.join("relations.csv")
    }

    fn metadata_file(&self) -> PathBuf {
        self.data_dir.join("metadata.txt")
    }
}

fn generate_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, seed.as_bytes())
}

struct TestData {
    spaces: Vec<Uuid>,
    entities: Vec<Uuid>,
    type_ids: Vec<Uuid>,
    properties: Vec<Uuid>,
}

/// Dump test data to CSV files for fast loading via COPY
fn dump_test_data(cfg: &Config) -> Result<TestData, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(&cfg.data_dir)?;

    println!("Generating test data to CSV files...");
    println!(
        "  {} entities, {} values/entity, {} relations/entity",
        cfg.entity_count, cfg.values_per_entity, cfg.relations_per_entity
    );
    println!(
        "  Total: ~{} values, ~{} relations",
        cfg.entity_count * cfg.values_per_entity,
        cfg.entity_count * cfg.relations_per_entity
    );
    println!("  Using production distributions for properties/spaces/types");

    // Use production distributions for realistic selectivity
    let dist = ProdDistributions::new();
    let mut rng = StdRng::seed_from_u64(42); // Deterministic for reproducibility

    let entities: Vec<Uuid> = (0..cfg.entity_count)
        .map(|i| generate_uuid(&format!("entity-{}", i)))
        .collect();

    // Extract the UUIDs from distributions for TestData
    let spaces: Vec<Uuid> = dist.spaces.iter().map(|(id, _)| *id).collect();
    let properties: Vec<Uuid> = dist.properties.iter().map(|(id, _)| *id).collect();
    let type_ids: Vec<Uuid> = dist.relation_types.iter().map(|(id, _)| *id).collect();

    // Write entities CSV
    println!("  Writing entities.csv...");
    let start = Instant::now();
    {
        let file = std::fs::File::create(cfg.entities_csv())?;
        let mut writer = BufWriter::new(file);
        for entity_id in &entities {
            writeln!(writer, "{}\t1700000000\t1000000\t1700000000\t1000000", entity_id)?;
        }
        // Also write relation entities
        for ei in 0..cfg.entity_count {
            for ri in 0..cfg.relations_per_entity {
                let rel_entity = generate_uuid(&format!("rel-entity-{}-{}", ei, ri));
                writeln!(writer, "{}\t1700000000\t1000000\t1700000000\t1000000", rel_entity)?;
            }
        }
    }
    println!("    Done in {:.2}s", start.elapsed().as_secs_f64());

    // Write values CSV with weighted distributions
    // Columns: id, entity_id, property_id, space_id, string, number, time
    println!("  Writing values.csv...");
    let start = Instant::now();
    {
        let file = std::fs::File::create(cfg.values_csv())?;
        let mut writer = BufWriter::new(file);
        for (ei, entity_id) in entities.iter().enumerate() {
            // Pick space weighted by production distribution
            let entity_space = dist.pick_space(&mut rng);
            for vi in 0..cfg.values_per_entity {
                let value_id = generate_uuid(&format!("value-{}-{}", ei, vi));
                // Pick property weighted by production distribution
                let property_id = dist.pick_property(&mut rng);
                // Generate unique string values (important for trigram/text index testing)
                let string_val = format!("Entity_{}_Prop_{}_Val_{}", ei, property_id.as_simple(), vi);
                // Generate realistic number and time values
                let number_val = (ei * 100 + vi) as f64;
                // Time as unix timestamp string (spread across 2 years)
                let base_time = 1700000000i64;
                let time_val = base_time + (ei as i64 * 1000) + (vi as i64 * 100);
                writeln!(
                    writer,
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    value_id, entity_id, property_id, entity_space, string_val, number_val, time_val
                )?;
            }
        }
    }
    println!("    Done in {:.2}s", start.elapsed().as_secs_f64());

    // Write relations CSV with weighted distributions
    println!("  Writing relations.csv...");
    let start = Instant::now();
    {
        let file = std::fs::File::create(cfg.relations_csv())?;
        let mut writer = BufWriter::new(file);
        for (ei, entity_id) in entities.iter().enumerate() {
            // Pick space weighted by production distribution
            let entity_space = dist.pick_space(&mut rng);
            for ri in 0..cfg.relations_per_entity {
                let to_entity = &entities[(ei + ri + 1) % cfg.entity_count];
                let rel_id = generate_uuid(&format!("relation-{}-{}", ei, ri));
                let rel_entity = generate_uuid(&format!("rel-entity-{}-{}", ei, ri));
                // Pick type weighted by production distribution
                let type_id = dist.pick_relation_type(&mut rng);
                writeln!(
                    writer,
                    "{}\t{}\t{}\t{}\t{}\t{}\ta0",
                    rel_id, rel_entity, type_id, entity_id, to_entity, entity_space
                )?;
            }
        }
    }
    println!("    Done in {:.2}s", start.elapsed().as_secs_f64());

    // Write metadata
    {
        let file = std::fs::File::create(cfg.metadata_file())?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "entity_count={}", cfg.entity_count)?;
        writeln!(writer, "values_per_entity={}", cfg.values_per_entity)?;
        writeln!(writer, "relations_per_entity={}", cfg.relations_per_entity)?;
        writeln!(writer, "num_spaces={}", cfg.num_spaces)?;
        writeln!(writer, "num_types={}", cfg.num_types)?;
        for (i, space) in spaces.iter().enumerate() {
            writeln!(writer, "space_{}={}", i, space)?;
        }
        for (i, entity) in entities.iter().take(10).enumerate() {
            writeln!(writer, "entity_{}={}", i, entity)?;
        }
        // Write middle entity for sampling
        writeln!(writer, "entity_middle={}", entities[cfg.entity_count / 2])?;
        for (i, type_id) in type_ids.iter().enumerate() {
            writeln!(writer, "type_{}={}", i, type_id)?;
        }
        for (i, prop) in properties.iter().enumerate() {
            writeln!(writer, "property_{}={}", i, prop)?;
        }
    }

    println!("CSV files written to {:?}", cfg.data_dir);
    Ok(TestData { spaces, entities, type_ids, properties })
}

async fn bench_query<F, Fut>(
    name: &str,
    _pool: &PgPool,
    iterations: usize,
    query_fn: F,
) -> Result<f64, sqlx::Error>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<i64, sqlx::Error>>,
{
    // Warmup
    for _ in 0..5 {
        query_fn().await?;
    }

    let start = Instant::now();
    let mut total_rows = 0i64;
    for _ in 0..iterations {
        total_rows += query_fn().await?;
    }
    let elapsed = start.elapsed();

    let avg_ms = elapsed.as_secs_f64() * 1000.0 / iterations as f64;
    let avg_rows = total_rows as f64 / iterations as f64;
    println!("  {}: {:.3}ms avg ({:.1} rows avg)", name, avg_ms, avg_rows);

    Ok(avg_ms)
}

/// Fetch actual sample values from the database to ensure queries hit real data
async fn fetch_sample_data(pool: &PgPool) -> Result<(Uuid, Uuid, Uuid, Uuid, Uuid, String), sqlx::Error> {
    // Get a sample value with all its IDs
    let row = sqlx::query(
        "SELECT v.entity_id, v.property_id, v.space_id, v.string
         FROM values v
         WHERE v.string IS NOT NULL
         LIMIT 1 OFFSET 40000"  // Pick from middle of dataset
    )
    .fetch_one(pool)
    .await?;

    let sample_entity: Uuid = row.get("entity_id");
    let sample_property: Uuid = row.get("property_id");
    let sample_space: Uuid = row.get("space_id");
    let sample_string: String = row.get("string");

    // Get a sample relation type and target
    let rel_row = sqlx::query(
        "SELECT r.type_id, r.to_entity_id
         FROM relations r
         WHERE r.space_id = $1
         LIMIT 1"
    )
    .bind(sample_space)
    .fetch_one(pool)
    .await?;

    let sample_type: Uuid = rel_row.get("type_id");
    let sample_target: Uuid = rel_row.get("to_entity_id");

    println!("Sample data from DB:");
    println!("  entity_id: {}", sample_entity);
    println!("  property_id: {}", sample_property);
    println!("  space_id: {}", sample_space);
    println!("  type_id: {}", sample_type);

    Ok((sample_entity, sample_property, sample_space, sample_type, sample_target, sample_string))
}

async fn run_benchmarks(
    pool: &PgPool,
    _data: &TestData,
    cfg: &Config,
) -> Result<(), sqlx::Error> {
    println!("\n## Query Benchmarks ({} iterations each)\n", cfg.query_iterations);

    // Fetch actual sample values from DB to ensure queries hit real data
    let (sample_entity, sample_property, sample_space, sample_type, sample_target, sample_string) =
        fetch_sample_data(pool).await?;
    let iterations = cfg.query_iterations;

    // Values queries
    println!("### Values Queries");
    {
        let pool = pool.clone();
        bench_query("entity_id + space_id", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                let row = sqlx::query("SELECT COUNT(*) as count FROM values WHERE entity_id = $1 AND space_id = $2")
                    .bind(sample_entity)
                    .bind(sample_space)
                    .fetch_one(&pool)
                    .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    {
        let pool = pool.clone();
        bench_query("entity_id + property_id", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                let row = sqlx::query("SELECT COUNT(*) as count FROM values WHERE entity_id = $1 AND property_id = $2")
                    .bind(sample_entity)
                    .bind(sample_property)
                    .fetch_one(&pool)
                    .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    {
        let pool = pool.clone();
        let search_string = sample_string.clone();
        bench_query("entity_id + property_id + string", &pool, iterations, || {
            let pool = pool.clone();
            let s = search_string.clone();
            async move {
                let row = sqlx::query("SELECT COUNT(*) as count FROM values WHERE entity_id = $1 AND property_id = $2 AND string = $3")
                    .bind(sample_entity)
                    .bind(sample_property)
                    .bind(&s)
                    .fetch_one(&pool)
                    .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    // Skip trigram benchmark - synthetic data all starts with "Value" so GIN returns all rows
    // Real-world trigram performance depends heavily on actual data distribution
    println!("  string % query (trigram): SKIPPED (synthetic data not representative)");

    // Relations queries
    println!("\n### Relations Queries");

    {
        let pool = pool.clone();
        bench_query("from_entity_id + space_id", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                let row = sqlx::query("SELECT COUNT(*) as count FROM relations WHERE from_entity_id = $1 AND space_id = $2")
                    .bind(sample_entity)
                    .bind(sample_space)
                    .fetch_one(&pool)
                    .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    {
        let pool = pool.clone();
        bench_query("to_entity_id + space_id", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                let row = sqlx::query("SELECT COUNT(*) as count FROM relations WHERE to_entity_id = $1 AND space_id = $2")
                    .bind(sample_entity)
                    .bind(sample_space)
                    .fetch_one(&pool)
                    .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    {
        let pool = pool.clone();
        bench_query("from_entity_id + type_id + to_entity_id", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                let row = sqlx::query("SELECT COUNT(*) as count FROM relations WHERE from_entity_id = $1 AND type_id = $2 AND to_entity_id = $3")
                    .bind(sample_entity)
                    .bind(sample_type)
                    .bind(sample_target)
                    .fetch_one(&pool)
                    .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    {
        let pool = pool.clone();
        bench_query("to_entity_id + type_id", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                let row = sqlx::query("SELECT COUNT(*) as count FROM relations WHERE to_entity_id = $1 AND type_id = $2")
                    .bind(sample_entity)
                    .bind(sample_type)
                    .fetch_one(&pool)
                    .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    {
        let pool = pool.clone();
        bench_query("space_id + type_id", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                let row = sqlx::query("SELECT COUNT(*) as count FROM relations WHERE space_id = $1 AND type_id = $2")
                    .bind(sample_space)
                    .bind(sample_type)
                    .fetch_one(&pool)
                    .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    // Entity EXISTS queries (GraphQL filter patterns)
    println!("\n### Entity Filter Queries (EXISTS patterns)");

    {
        let pool = pool.clone();
        bench_query("entities with value property=X", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                let row = sqlx::query(
                    "SELECT COUNT(*) as count FROM entities e
                     WHERE EXISTS (
                         SELECT 1 FROM values v
                         WHERE v.entity_id = e.id AND v.property_id = $1 AND v.space_id = $2
                     )"
                )
                .bind(sample_property)
                .bind(sample_space)
                .fetch_one(&pool)
                .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    {
        let pool = pool.clone();
        bench_query("entities with relation type=X", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                let row = sqlx::query(
                    "SELECT COUNT(*) as count FROM entities e
                     WHERE EXISTS (
                         SELECT 1 FROM relations r
                         WHERE r.from_entity_id = e.id AND r.type_id = $1 AND r.space_id = $2
                     )"
                )
                .bind(sample_type)
                .bind(sample_space)
                .fetch_one(&pool)
                .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    {
        let pool = pool.clone();
        bench_query("entities with relation type=X to=Y", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                let row = sqlx::query(
                    "SELECT COUNT(*) as count FROM entities e
                     WHERE EXISTS (
                         SELECT 1 FROM relations r
                         WHERE r.from_entity_id = e.id AND r.type_id = $1 AND r.to_entity_id = $2
                     )"
                )
                .bind(sample_type)
                .bind(sample_target)
                .fetch_one(&pool)
                .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    // Value operator queries (filtering by property + value)
    println!("\n### Value Operator Queries (property + value filter)");

    {
        let pool = pool.clone();
        let search_string = sample_string.clone();
        bench_query("entities with property=X AND string=Y", &pool, iterations, || {
            let pool = pool.clone();
            let s = search_string.clone();
            async move {
                let row = sqlx::query(
                    "SELECT COUNT(*) as count FROM entities e
                     WHERE EXISTS (
                         SELECT 1 FROM values v
                         WHERE v.entity_id = e.id
                           AND v.property_id = $1
                           AND v.string = $2
                           AND v.space_id = $3
                     )"
                )
                .bind(sample_property)
                .bind(&s)
                .bind(sample_space)
                .fetch_one(&pool)
                .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    {
        let pool = pool.clone();
        bench_query("entities with property=X AND number > Y", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                // Filter for number > 35000000 (roughly half the entities)
                let row = sqlx::query(
                    "SELECT COUNT(*) as count FROM entities e
                     WHERE EXISTS (
                         SELECT 1 FROM values v
                         WHERE v.entity_id = e.id
                           AND v.property_id = $1
                           AND v.number > $2
                           AND v.space_id = $3
                     )"
                )
                .bind(sample_property)
                .bind(35000000.0f64)
                .bind(sample_space)
                .fetch_one(&pool)
                .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    {
        let pool = pool.clone();
        bench_query("entities with property=X AND time > Y", &pool, iterations, || {
            let pool = pool.clone();
            async move {
                // Filter for time > 1700350000 (roughly half the entities)
                let row = sqlx::query(
                    "SELECT COUNT(*) as count FROM entities e
                     WHERE EXISTS (
                         SELECT 1 FROM values v
                         WHERE v.entity_id = e.id
                           AND v.property_id = $1
                           AND v.time > $2
                           AND v.space_id = $3
                     )"
                )
                .bind(sample_property)
                .bind("1700350000")
                .bind(sample_space)
                .fetch_one(&pool)
                .await?;
                Ok(row.get::<i64, _>("count"))
            }
        }).await?;
    }

    Ok(())
}

async fn show_current_indexes(pool: &PgPool) -> Result<(), sqlx::Error> {
    println!("\n## Current Indexes\n");

    println!("### Values");
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT indexname FROM pg_indexes WHERE tablename = 'values' ORDER BY indexname"
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        println!("  - {}", row);
    }

    println!("\n### Relations");
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT indexname FROM pg_indexes WHERE tablename = 'relations' ORDER BY indexname"
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        println!("  - {}", row);
    }

    Ok(())
}

enum Mode {
    Dump,       // Generate CSV files only
    QueryOnly,  // Run benchmarks only (assume data loaded via psql)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Query benchmark for kg-indexer\n");
        println!("Usage:");
        println!("  --dump        Generate CSV files to bench_data/");
        println!("  --query-only  Run benchmarks (data must be loaded via psql first)");
        println!("\nSee module docs for full workflow.");
        return Ok(());
    }

    let mode = if args.iter().any(|a| a == "--dump") {
        Mode::Dump
    } else if args.iter().any(|a| a == "--query-only") {
        Mode::QueryOnly
    } else {
        eprintln!("Usage: --dump or --query-only");
        eprintln!("Run with --help for details.");
        std::process::exit(1);
    };

    let cfg = Config::from_env();
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5433/gaia_bench".to_string());

    println!("# Query Performance Benchmark\n");

    match mode {
        Mode::Dump => {
            println!("Mode: Dump CSV files\n");
            let _data = dump_test_data(&cfg)?;
            println!("\nCSV files ready at {:?}", cfg.data_dir);
            println!("\nNext: load via psql:");
            println!("  psql $DATABASE_URL -c \"\\copy entities(id,created_at,created_at_block,updated_at,updated_at_block) FROM 'bench_data/entities.csv'\"");
            println!("  psql $DATABASE_URL -c \"\\copy values(id,entity_id,property_id,space_id,string,number,time) FROM 'bench_data/values.csv'\"");
            println!("  psql $DATABASE_URL -c \"\\copy relations(id,entity_id,type_id,from_entity_id,to_entity_id,space_id,position) FROM 'bench_data/relations.csv'\"");
            println!("  psql $DATABASE_URL -c \"ANALYZE entities, values, relations\"");
            println!("\nThen run: cargo run --example query_bench -p kg-indexer --release -- --query-only");
        }
        Mode::QueryOnly => {
            println!("Mode: Query-only\n");
            println!("Connecting to: {}", database_url);

            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&database_url)
                .await?;

            // Read metadata to reconstruct TestData
            let metadata = std::fs::read_to_string(cfg.metadata_file())
                .map_err(|e| format!("Failed to read metadata file {:?}: {}. Run --dump first.", cfg.metadata_file(), e))?;

            let mut spaces = Vec::new();
            let mut type_ids = Vec::new();
            let mut properties = Vec::new();
            let mut entity_count = 0usize;

            for line in metadata.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    if key.starts_with("space_") {
                        spaces.push(value.parse::<Uuid>()?);
                    } else if key.starts_with("type_") {
                        type_ids.push(value.parse::<Uuid>()?);
                    } else if key.starts_with("property_") {
                        properties.push(value.parse::<Uuid>()?);
                    } else if key == "entity_count" {
                        entity_count = value.parse()?;
                    }
                }
            }

            let entities: Vec<Uuid> = (0..entity_count)
                .map(|i| generate_uuid(&format!("entity-{}", i)))
                .collect();

            let data = TestData { spaces, entities, type_ids, properties };

            show_current_indexes(&pool).await?;
            run_benchmarks(&pool, &data, &cfg).await?;
            println!("\nDone.");
        }
    }

    Ok(())
}
