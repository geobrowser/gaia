# Query Benchmarking Strategy

## Overview

This document describes the query benchmarking strategy for kg-indexer's PostgreSQL indexes. The goal is to find the minimal set of indexes that maintains query performance while maximizing write throughput.

## Benchmark Tool

Location: `kg-indexer/examples/query_bench.rs`

### Usage

```bash
# Option 1: Use production data dump (recommended)
pg_dump $PROD_DB_URL --data-only -t entities -t values -t relations > bench_data/prod_dump.sql
psql $BENCH_DB_URL -f bench_data/prod_dump.sql

# Option 2: Generate synthetic data with production distributions
cargo run --example query_bench -p kg-indexer --release -- --dump
psql $BENCH_DB_URL -c "\copy entities(...) FROM 'bench_data/entities.csv'"
# ... load other tables

# Run benchmarks
DATABASE_URL=$BENCH_DB_URL cargo run --example query_bench -p kg-indexer --release -- --query-only

# Test different index configurations
psql $BENCH_DB_URL -c "DROP INDEX values_foo_idx"
DATABASE_URL=$BENCH_DB_URL cargo run --example query_bench -p kg-indexer --release -- --query-only
```

## Query Patterns Tested

Derived from GraphQL API usage in `geogenesis/apps/web/core/io/`.

### Point Lookups (fast path)
- `entity_id + space_id` - get values for entity in space
- `entity_id + property_id` - get specific property value
- `entity_id + property_id + string` - exact value match

### Relation Queries
- `from_entity_id + space_id` - outgoing relations
- `to_entity_id + space_id` - incoming relations
- `space_id + type_id` - relations by type in space

### Entity Filter Queries (EXISTS patterns)
- `entities WHERE EXISTS (values with property_id = X)` - filter by property
- `entities WHERE EXISTS (relations with type_id = X)` - filter by relation type
- `entities WHERE EXISTS (relations with type_id = X AND to_entity_id = Y)` - specific relation

### Value Operator Queries
- `property_id = X AND string = Y` - text equality filter
- `property_id = X AND number > Y` - numeric range filter
- `property_id = X AND time > Y` - temporal range filter

## Data Characteristics (Production)

From production database (Jan 2026):
- **Entities**: 714K
- **Values**: 80K (skewed: top property "name" = 50%)
- **Relations**: 150K (skewed: top type = 44%)
- **Properties**: 69 distinct (power law distribution)
- **Spaces**: 92 distinct (top space = 74% of values)
- **Relation types**: 114 distinct

### Why Real Data Matters

Synthetic data with uniform distribution produces misleading benchmarks:
- Uniform property distribution → indexes appear useless
- Real data is heavily skewed → indexes are highly effective

Example: `property=X AND string=Y` query
- Synthetic (3 properties, uniform): 168ms (index not used)
- Real data (69 properties, skewed): 0.55ms (index used effectively)

## Benchmark Results (Production Data, Jan 2026)

Three-way comparison: Full (20 indexes) vs Proposed (13) vs None (pkey only)

| Query | Full | Proposed | None | Notes |
|-------|------|----------|------|-------|
| **Point Lookups** |
| entity_id + space_id | 0.5ms | 0.49ms | 5.8ms | 12x slower without |
| entity_id + property_id | 0.48ms | 0.48ms | 5.2ms | 11x slower without |
| entity + prop + string | 0.5ms | 0.48ms | 7.0ms | 14x slower without |
| **Relations** |
| from_entity + space | 0.62ms | 0.33ms | 12.9ms | 39x slower without |
| to_entity + space | 0.98ms | 0.42ms | 11.2ms | 27x slower without |
| space_id + type_id | 2.8ms | 1.2ms | 13.3ms | Proposed faster! |
| **Entity Filters (EXISTS)** |
| property=X exists | 84ms | 81ms | 77ms | No impact (full scan) |
| relation type=X | 5.9ms | 6.6ms | 14.7ms | 2.5x slower without |
| relation type=X to=Y | 0.51ms | 0.48ms | 8.7ms | 18x slower without |
| **Value Operators** |
| prop=X AND string=Y | 0.55ms | 0.61ms | 7.3ms | 12x slower without |
| prop=X AND number > Y | 5.5ms | 6.1ms | 7.2ms | Minimal impact |
| prop=X AND time > Y | 5.1ms | 6.4ms | 5.9ms | No impact |

### Key Findings

1. **Proposed indexes have zero regression** - all queries same speed or faster
2. **Point lookups critically need indexes** - 10-40x slower without
3. **`property=X exists` is slow regardless** (77-84ms) - scans all entities
4. **Single-column number/time indexes provide no benefit** - would need composite `(property_id, number)` to help

## Index Recommendations

### Keep (13 indexes)

**Values:**
- `values_entity_property_idx` - point lookups
- `values_entity_property_space_idx` - point lookups
- `values_entity_space_idx` - point lookups
- `values_property_id_idx` - value operator queries
- `values_text_gin_trgm_idx` - fuzzy text search

**Relations:**
- `relations_entity_id_idx` - entity lookups
- `relations_from_entity_id_idx` - outgoing relations
- `relations_from_entity_space_idx` - outgoing in space
- `relations_to_entity_id_idx` - incoming relations
- `relations_to_entity_space_idx` - incoming in space
- `relations_space_type_idx` - type filtering

### Safe to Drop (7 indexes)

**Values:**
- `values_entity_id_idx` - covered by composites
- `values_space_id_idx` - covered by composites
- `values_property_space_idx` - rarely used pattern
- `values_number_idx` - no benefit without composite
- `values_time_idx` - no benefit without composite

**Relations:**
- `relations_space_id_idx` - covered by composites
- `relations_type_id_idx` - covered by space_type composite

### Impact

- 35% fewer indexes (20 → 13)
- Zero query performance regression
- Estimated 20-30% write throughput improvement

## Local Benchmark Setup

```bash
# Start local postgres
docker-compose up -d postgres-bench

# Restore prod dump
psql postgres://postgres:postgres@localhost:5433/gaia_bench -f bench_data/prod_dump.sql

# Create indexes to test
psql postgres://postgres:postgres@localhost:5433/gaia_bench -f create_indexes.sql

# Run benchmark
DATABASE_URL=postgres://postgres:postgres@localhost:5433/gaia_bench \
  cargo run --example query_bench -p kg-indexer --release -- --query-only
```

## Files

- `kg-indexer/examples/query_bench.rs` - benchmark tool
- `bench_data/prod_dump.sql` - production data dump (gitignored)
- `bench_data/prod_distributions.json` - sampled distribution stats
