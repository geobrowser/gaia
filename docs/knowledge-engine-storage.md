# Knowledge Engine Storage Design

This document describes a custom storage engine for the Geo Knowledge Graph, optimized for our specific data model and query patterns.

## Design Goals

- Optimized for current-state queries (primary use case)
- Support historical queries for proposals/diffs (secondary)
- Entity-centric access patterns
- Rebuild capability from blockchain WAL
- Simple, minimal dependencies

## Data Characteristics

### Current Scale

```
500k entities × ~12 values avg × ~50 bytes/value = ~300 MB values
500k entities × ~10 relations avg × ~100 bytes/relation = ~500 MB relations
Property metadata: negligible

Current state total: ~1 GB (fits comfortably in RAM)
At 10x scale (5M entities): ~10 GB (still feasible for single node)
```

### Entity Shape

- Entities are narrow: 5-20 values, 0-20 relations each
- Values and relations per entity are stable
- Most queries want full entity with all values/relations

## Query Patterns

| Query | Frequency | Access Pattern |
|-------|-----------|----------------|
| Entity + all values + property names | Hot | Point lookup → inline values → property metadata |
| Entity + relations + to_entity names | Hot | Point lookup → inline relations → point lookups |
| Search by name in space | Hot | Index scan: `(space_id, name) → entity_ids` |
| Filter by property=value | Hot | Index scan: `(property_id, value) → entity_ids` |
| Filter by relation type + to_entity | Hot | Index scan: `(relation_type, to_entity) → entity_ids` |
| Historical state at block X | Cold | Segment lookup, binary search |

## Storage Model Decision

### Why Not EAV Tables

Traditional EAV (like our current Postgres schema):
```sql
SELECT * FROM values WHERE entity_id = ?;
SELECT * FROM relations WHERE entity_id = ?;
-- Multiple joins to reconstruct entity
```

### Entity-Centric Storage

Store entities with values and relations inline:
```rust
struct Entity {
    id: Uuid,
    values: Vec<Value>,
    relations_out: Vec<Relation>,
    created_at_block: u64,
    updated_at_block: u64,
}
```

**Benefit:** One hashmap lookup returns complete entity. Zero joins.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     PRIMARY STORE (In-Memory)                   │
│                        Current State Only                       │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ entities: HashMap<EntityId, Entity>                      │   │
│  │                                                          │   │
│  │ Entity {                                                 │   │
│  │   id: Uuid,                                              │   │
│  │   values: Vec<Value>,                                    │   │
│  │   relations_out: Vec<Relation>,                          │   │
│  │   created_at_block: u64,                                 │   │
│  │   updated_at_block: u64,                                 │   │
│  │ }                                                        │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ INDEXES                                                  │   │
│  │                                                          │   │
│  │ by_space: HashMap<SpaceId, HashSet<EntityId>>            │   │
│  │ by_name: HashMap<(SpaceId, String), HashSet<EntityId>>   │   │
│  │ by_prop_value: HashMap<(PropId, Value), HashSet<EntityId>>│  │
│  │ by_relation: HashMap<(RelTypeId, ToEntityId), HashSet<EntityId>>│
│  │ relations_in: HashMap<EntityId, Vec<Relation>>           │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ METADATA                                                 │   │
│  │ properties: HashMap<PropertyId, PropertyMeta>            │   │
│  │ spaces: HashMap<SpaceId, SpaceMeta>                      │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ historical queries
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                   HISTORY STORE (On-Disk)                       │
│                     Append-Only Segments                        │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ Segment files (one per block range):                     │   │
│  │   blocks_0_10000.segment                                 │   │
│  │   blocks_10001_20000.segment                             │   │
│  │   ...                                                    │   │
│  │                                                          │   │
│  │ Format: sorted by (entity_id, block)                     │   │
│  │ [entity_id][block][entity snapshot or delta]             │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ INDEXES                                                  │   │
│  │ block_index: block_number → segment file + offset        │   │
│  │ entity_block_index: (entity_id, block) → segment + offset│   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ rebuild from
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                      WAL (Source of Truth)                      │
│                   Blockchain + IPFS (unchanged)                 │
└─────────────────────────────────────────────────────────────────┘
```

## Primary Store (In-Memory)

### Data Structures

```rust
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

type EntityId = Uuid;
type PropertyId = Uuid;
type SpaceId = Uuid;
type RelationTypeId = Uuid;
type BlockNumber = u64;

#[derive(Clone)]
struct Value {
    property_id: PropertyId,
    value: TypedValue,
    language: Option<String>,
    unit: Option<String>,
}

#[derive(Clone)]
enum TypedValue {
    String(String),
    Number(f64),
    Boolean(bool),
    Time(String),
    Point(String),
}

#[derive(Clone)]
struct Relation {
    id: Uuid,
    relation_type: RelationTypeId,
    from_entity: EntityId,
    to_entity: EntityId,
    from_space: Option<SpaceId>,
    to_space: Option<SpaceId>,
    position: Option<String>,
    verified: Option<bool>,
}

#[derive(Clone)]
struct Entity {
    id: EntityId,
    space_id: SpaceId,
    values: Vec<Value>,
    relations_out: Vec<Relation>,
    created_at_block: BlockNumber,
    updated_at_block: BlockNumber,
}

struct PrimaryStore {
    // Main entity storage
    entities: HashMap<EntityId, Entity>,

    // Indexes for query patterns
    by_space: HashMap<SpaceId, HashSet<EntityId>>,
    by_name: HashMap<(SpaceId, String), HashSet<EntityId>>,
    by_prop_value: HashMap<(PropertyId, TypedValue), HashSet<EntityId>>,
    by_relation_type_target: HashMap<(RelationTypeId, EntityId), HashSet<EntityId>>,
    relations_in: HashMap<EntityId, Vec<Relation>>,

    // Metadata
    properties: HashMap<PropertyId, PropertyMeta>,
    spaces: HashMap<SpaceId, SpaceMeta>,

    // Cursor
    current_block: BlockNumber,
}
```

### Index Maintenance

Indexes are updated during op application:

```rust
impl PrimaryStore {
    fn apply_update_entity(&mut self, entity_id: EntityId, values: Vec<Value>, block: BlockNumber) {
        let entity = self.entities.entry(entity_id).or_insert_with(|| {
            // New entity
            let entity = Entity {
                id: entity_id,
                space_id: /* from context */,
                values: vec![],
                relations_out: vec![],
                created_at_block: block,
                updated_at_block: block,
            };
            // Add to space index
            self.by_space.entry(entity.space_id).or_default().insert(entity_id);
            entity
        });

        entity.updated_at_block = block;

        for value in values {
            // Remove old index entries
            if let Some(old_value) = entity.values.iter().find(|v| v.property_id == value.property_id) {
                self.by_prop_value
                    .entry((value.property_id, old_value.value.clone()))
                    .and_modify(|set| { set.remove(&entity_id); });
            }

            // Add new index entry
            self.by_prop_value
                .entry((value.property_id, value.value.clone()))
                .or_default()
                .insert(entity_id);

            // Update name index if this is the name property
            if is_name_property(value.property_id) {
                if let TypedValue::String(name) = &value.value {
                    self.by_name
                        .entry((entity.space_id, name.clone()))
                        .or_default()
                        .insert(entity_id);
                }
            }

            // Upsert value
            if let Some(existing) = entity.values.iter_mut().find(|v| v.property_id == value.property_id) {
                *existing = value;
            } else {
                entity.values.push(value);
            }
        }
    }

    fn apply_create_relation(&mut self, relation: Relation, block: BlockNumber) {
        // Add to source entity
        if let Some(from_entity) = self.entities.get_mut(&relation.from_entity) {
            from_entity.relations_out.push(relation.clone());
            from_entity.updated_at_block = block;
        }

        // Add to reverse index
        self.relations_in
            .entry(relation.to_entity)
            .or_default()
            .push(relation.clone());

        // Add to type+target index
        self.by_relation_type_target
            .entry((relation.relation_type, relation.to_entity))
            .or_default()
            .insert(relation.from_entity);
    }
}
```

### Query Examples

```rust
impl PrimaryStore {
    // Hot query: Get entity with all values and relations
    fn get_entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(&id)
    }

    // Get entity with resolved property names
    fn get_entity_resolved(&self, id: EntityId) -> Option<ResolvedEntity> {
        let entity = self.entities.get(&id)?;
        Some(ResolvedEntity {
            id: entity.id,
            values: entity.values.iter().map(|v| {
                let prop_meta = self.properties.get(&v.property_id);
                ResolvedValue {
                    property_name: prop_meta.map(|p| p.name.clone()),
                    value: v.value.clone(),
                    // ...
                }
            }).collect(),
            // ...
        })
    }

    // Search by name in space
    fn find_by_name(&self, space_id: SpaceId, name: &str) -> Vec<EntityId> {
        self.by_name
            .get(&(space_id, name.to_string()))
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    // Filter by property value
    fn find_by_property_value(&self, property_id: PropertyId, value: &TypedValue) -> Vec<EntityId> {
        self.by_prop_value
            .get(&(property_id, value.clone()))
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    // Filter by relation type and target
    fn find_by_relation(&self, relation_type: RelationTypeId, to_entity: EntityId) -> Vec<EntityId> {
        self.by_relation_type_target
            .get(&(relation_type, to_entity))
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    // Get incoming relations for an entity
    fn get_incoming_relations(&self, entity_id: EntityId) -> Vec<&Relation> {
        self.relations_in
            .get(&entity_id)
            .map(|rels| rels.iter().collect())
            .unwrap_or_default()
    }
}
```

## History Store (On-Disk)

### Segment Format

Each segment covers a range of blocks and stores entity snapshots:

```
Segment Header:
  [magic: 4 bytes] "GEOS"
  [version: 2 bytes]
  [start_block: 8 bytes]
  [end_block: 8 bytes]
  [entry_count: 8 bytes]
  [index_offset: 8 bytes]

Entries (sorted by entity_id, then block):
  [entity_id: 16 bytes]
  [block: 8 bytes]
  [snapshot_len: 4 bytes]
  [snapshot: variable]  // serialized Entity

Footer Index:
  [entity_id: 16 bytes][block: 8 bytes][offset: 8 bytes]
  ... (one per entry, for binary search)
```

### Historical Query

```rust
impl HistoryStore {
    fn get_entity_at_block(&self, entity_id: EntityId, block: BlockNumber) -> Option<Entity> {
        // Find segment containing this block
        let segment = self.find_segment(block)?;

        // Binary search for (entity_id, block <= target)
        let entry = segment.find_latest_before(entity_id, block)?;

        // Deserialize and return
        Some(entry.deserialize())
    }
}
```

## Durability

### Snapshot Strategy

```rust
struct SnapshotManager {
    snapshot_dir: PathBuf,
    snapshot_interval_blocks: u64,
}

impl SnapshotManager {
    fn save_snapshot(&self, store: &PrimaryStore) -> Result<()> {
        let path = self.snapshot_dir.join(format!("snapshot_{}.bin", store.current_block));

        let file = File::create(&path)?;
        let mut writer = BufWriter::new(file);

        // Write header
        writer.write_all(b"GEOS")?;
        writer.write_all(&store.current_block.to_le_bytes())?;
        writer.write_all(&(store.entities.len() as u64).to_le_bytes())?;

        // Write entities
        for entity in store.entities.values() {
            serialize_entity(&mut writer, entity)?;
        }

        // Indexes are rebuilt on load (fast enough for current scale)

        Ok(())
    }

    fn load_latest_snapshot(&self) -> Result<Option<PrimaryStore>> {
        let latest = self.find_latest_snapshot()?;
        if let Some(path) = latest {
            let store = self.load_snapshot(&path)?;
            Ok(Some(store))
        } else {
            Ok(None)
        }
    }
}
```

### Startup Sequence

```rust
impl KnowledgeEngine {
    fn startup(&mut self) -> Result<()> {
        // 1. Try to load latest snapshot
        if let Some(store) = self.snapshot_manager.load_latest_snapshot()? {
            self.primary_store = store;
            println!("Loaded snapshot at block {}", self.primary_store.current_block);
        }

        // 2. Replay WAL from snapshot block to current
        let start_block = self.primary_store.current_block + 1;
        let current_block = self.wal.get_current_block()?;

        println!("Replaying blocks {} to {}", start_block, current_block);

        for block in start_block..=current_block {
            let edits = self.wal.get_edits_for_block(block)?;
            for edit in edits {
                self.apply_edit(edit, block)?;
            }
        }

        // 3. Ready to serve queries
        println!("Ready at block {}", self.primary_store.current_block);

        Ok(())
    }
}
```

## Binary Format for Entities

```rust
fn serialize_entity(writer: &mut impl Write, entity: &Entity) -> Result<()> {
    // Entity ID
    writer.write_all(entity.id.as_bytes())?;

    // Space ID
    writer.write_all(entity.space_id.as_bytes())?;

    // Block numbers
    writer.write_all(&entity.created_at_block.to_le_bytes())?;
    writer.write_all(&entity.updated_at_block.to_le_bytes())?;

    // Values
    writer.write_all(&(entity.values.len() as u32).to_le_bytes())?;
    for value in &entity.values {
        serialize_value(writer, value)?;
    }

    // Relations
    writer.write_all(&(entity.relations_out.len() as u32).to_le_bytes())?;
    for relation in &entity.relations_out {
        serialize_relation(writer, relation)?;
    }

    Ok(())
}

fn serialize_value(writer: &mut impl Write, value: &Value) -> Result<()> {
    writer.write_all(value.property_id.as_bytes())?;

    match &value.value {
        TypedValue::String(s) => {
            writer.write_all(&[0u8])?;  // type tag
            writer.write_all(&(s.len() as u32).to_le_bytes())?;
            writer.write_all(s.as_bytes())?;
        }
        TypedValue::Number(n) => {
            writer.write_all(&[1u8])?;
            writer.write_all(&n.to_le_bytes())?;
        }
        TypedValue::Boolean(b) => {
            writer.write_all(&[2u8])?;
            writer.write_all(&[*b as u8])?;
        }
        // ... other types
    }

    // Optional fields
    serialize_optional_string(writer, &value.language)?;
    serialize_optional_string(writer, &value.unit)?;

    Ok(())
}
```

## Query Engine

The query engine supports arbitrary filtering and ordering without requiring pre-defined indexes for every possible query pattern.

### Design Philosophy

Rather than pre-indexing all possible orderings (which would be combinatorially explosive), we:

1. **Index for filtering** - reduce candidate set using available indexes
2. **Sort in memory** - fast for reduced candidate sets
3. **Cache results** - avoid re-sorting for pagination and repeated queries

### Query Pattern Language

Queries are expressed as patterns that entities match against:

```rust
struct Query {
    space_id: SpaceId,
    filters: Vec<Filter>,
    traversals: Vec<Traversal>,
    ordering: Option<Ordering>,
    limit: usize,
    offset: usize,
}

enum Filter {
    HasProperty(PropertyId),
    HasRelation(RelationTypeId, Direction),
    PropertyEq(PropertyId, TypedValue),
    PropertyGt(PropertyId, TypedValue),
    PropertyLt(PropertyId, TypedValue),
    PropertyIn(PropertyId, Vec<TypedValue>),
    PropertyContains(PropertyId, String),
}

struct Traversal {
    relation_type: RelationTypeId,
    direction: Direction,
    target_filters: Vec<Filter>,
    bind_as: Option<String>,
}

enum Ordering {
    ByProperty(PropertyId, Order),
    ByTraversedProperty(String, PropertyId, Order),  // binding.property
    ByCount(String, Order),                          // count of binding
}

enum Order { Asc, Desc }
enum Direction { Out, In }
```

### Fluent Builder API

```rust
// Simple: People in space X
store.query(space_x)
    .filter_eq(TYPE_PROP, "Person")
    .execute()

// With ordering: People ordered by birthdate
store.query(space_x)
    .filter_eq(TYPE_PROP, "Person")
    .has(BIRTHDATE_PROP)
    .order_by(BIRTHDATE_PROP, Desc)
    .limit(100)
    .execute()

// Traversal filter: People who work at companies founded before 2000
store.query(space_x)
    .filter_eq(TYPE_PROP, "Person")
    .traverse(WORKS_AT)
        .filter_lt(FOUNDING_DATE_PROP, 2000)
        .end()
    .execute()

// Traversal ordering: People ordered by company founding date
store.query(space_x)
    .filter_eq(TYPE_PROP, "Person")
    .traverse(WORKS_AT)
        .bind("company")
        .end()
    .order_by_traversed("company", FOUNDING_DATE_PROP, Desc)
    .limit(100)
    .execute()

// Aggregation ordering: People ordered by follower count
store.query(space_x)
    .filter_eq(TYPE_PROP, "Person")
    .traverse_in(FOLLOWS)
        .bind("followers")
        .end()
    .order_by_count("followers", Desc)
    .limit(100)
    .execute()
```

### Query Execution

```rust
struct QueryEngine {
    store: PrimaryStore,
    result_cache: LruCache<u64, CachedResult>,
}

struct CachedResult {
    entity_ids: Vec<EntityId>,
    total: usize,
    cached_at_block: BlockNumber,
}

impl QueryEngine {
    fn execute(&mut self, query: Query) -> QueryResult {
        let cache_key = query.hash();

        // Check cache (includes full sorted result for pagination)
        if let Some(cached) = self.result_cache.get(&cache_key) {
            if cached.cached_at_block == self.store.current_block {
                return QueryResult {
                    entities: cached.entity_ids.iter()
                        .skip(query.offset)
                        .take(query.limit)
                        .cloned()
                        .collect(),
                    total: cached.total,
                };
            }
        }

        // 1. Filter using indexes (reduce candidate set)
        let candidates = self.apply_filters(&query);

        // 2. Apply traversal filters
        let filtered = self.apply_traversals(candidates, &query.traversals);

        // 3. Check result set size
        if filtered.len() > MAX_SORT_SIZE {
            return QueryResult::TooManyResults {
                count: filtered.len(),
                suggestion: "Add more filters to reduce result set",
            };
        }

        // 4. Sort in memory
        let sorted = self.apply_ordering(filtered, &query.ordering);

        // 5. Cache full sorted result
        self.result_cache.insert(cache_key, CachedResult {
            entity_ids: sorted.clone(),
            total: sorted.len(),
            cached_at_block: self.store.current_block,
        });

        // 6. Paginate and return
        QueryResult::Success {
            entities: sorted.into_iter()
                .skip(query.offset)
                .take(query.limit)
                .collect(),
            total: sorted.len(),
        }
    }

    fn apply_filters(&self, query: &Query) -> HashSet<EntityId> {
        // Start with space filter
        let mut candidates = self.store.by_space
            .get(&query.space_id)
            .cloned()
            .unwrap_or_default();

        // Apply each filter, intersecting results
        for filter in &query.filters {
            let matches = self.evaluate_filter(filter);
            candidates = candidates.intersection(&matches).cloned().collect();

            // Early exit if no candidates remain
            if candidates.is_empty() {
                break;
            }
        }

        candidates
    }

    fn evaluate_filter(&self, filter: &Filter) -> HashSet<EntityId> {
        match filter {
            Filter::PropertyEq(prop, value) => {
                self.store.by_prop_value
                    .get(&(*prop, value.clone()))
                    .cloned()
                    .unwrap_or_default()
            }
            Filter::HasProperty(prop) => {
                // Scan entities that have this property
                self.store.entities.iter()
                    .filter(|(_, e)| e.values.iter().any(|v| v.property_id == *prop))
                    .map(|(id, _)| *id)
                    .collect()
            }
            Filter::HasRelation(rel_type, direction) => {
                match direction {
                    Direction::Out => {
                        self.store.entities.iter()
                            .filter(|(_, e)| e.relations_out.iter().any(|r| r.relation_type == *rel_type))
                            .map(|(id, _)| *id)
                            .collect()
                    }
                    Direction::In => {
                        self.store.relations_in.iter()
                            .filter(|(_, rels)| rels.iter().any(|r| r.relation_type == *rel_type))
                            .map(|(id, _)| *id)
                            .collect()
                    }
                }
            }
            // ... other filter types
        }
    }

    fn apply_traversals(&self, candidates: HashSet<EntityId>, traversals: &[Traversal]) -> Vec<EntityId> {
        let mut result: Vec<EntityId> = candidates.into_iter().collect();

        for traversal in traversals {
            result = result.into_iter().filter(|id| {
                self.entity_matches_traversal(*id, traversal)
            }).collect();
        }

        result
    }

    fn entity_matches_traversal(&self, entity_id: EntityId, traversal: &Traversal) -> bool {
        let relations = match traversal.direction {
            Direction::Out => {
                self.store.entities.get(&entity_id)
                    .map(|e| &e.relations_out)
            }
            Direction::In => {
                self.store.relations_in.get(&entity_id)
            }
        };

        let Some(relations) = relations else { return false };

        // Check if any relation of the right type leads to a target matching filters
        relations.iter()
            .filter(|r| r.relation_type == traversal.relation_type)
            .any(|r| {
                let target_id = match traversal.direction {
                    Direction::Out => r.to_entity,
                    Direction::In => r.from_entity,
                };
                self.entity_matches_filters(target_id, &traversal.target_filters)
            })
    }

    fn apply_ordering(&self, mut entities: Vec<EntityId>, ordering: &Option<Ordering>) -> Vec<EntityId> {
        let Some(ordering) = ordering else { return entities };

        match ordering {
            Ordering::ByProperty(prop, order) => {
                entities.sort_by(|a, b| {
                    let va = self.get_property_value(*a, *prop);
                    let vb = self.get_property_value(*b, *prop);
                    compare_values(&va, &vb, *order)
                });
            }
            Ordering::ByTraversedProperty(binding, prop, order) => {
                // Must evaluate traversal for each entity to get sort key
                entities.sort_by(|a, b| {
                    let va = self.get_traversed_property(*a, binding, *prop);
                    let vb = self.get_traversed_property(*b, binding, *prop);
                    compare_values(&va, &vb, *order)
                });
            }
            Ordering::ByCount(binding, order) => {
                entities.sort_by(|a, b| {
                    let ca = self.count_traversal(*a, binding);
                    let cb = self.count_traversal(*b, binding);
                    match order {
                        Order::Asc => ca.cmp(&cb),
                        Order::Desc => cb.cmp(&ca),
                    }
                });
            }
        }

        entities
    }
}
```

### Ordering Complexity

| Ordering Type | Complexity | Notes |
|---------------|------------|-------|
| By direct property | O(n log n) | n = filtered candidates |
| By traversed property | O(n × m log n) | m = avg relations per entity |
| By count | O(n × m log n) | m = avg relations per entity |

**Key insight:** We cannot use indexes to avoid sorting for arbitrary orderings. Instead, we rely on:

1. Filters being selective (reducing n significantly)
2. Caching sorted results (amortizing sort cost across pagination)
3. Failing gracefully when result sets are too large

### Cache Invalidation

```rust
impl QueryEngine {
    fn on_block_applied(&mut self, block: BlockNumber) {
        // Simple strategy: invalidate all caches on any write
        // More sophisticated: track which entities changed and invalidate affected queries
        self.result_cache.clear();
    }
}
```

For more sophisticated invalidation, track query dependencies:
- Query depends on space S → invalidate if any entity in S changes
- Query filters on property P → invalidate if any value of P changes
- Query traverses relation type R → invalidate if any R relation changes

### Result Size Limits

For queries that match too many entities:

```rust
const MAX_SORT_SIZE: usize = 100_000;

enum QueryResult {
    Success {
        entities: Vec<EntityId>,
        total: usize,
    },
    TooManyResults {
        count: usize,
        suggestion: String,
    },
}
```

Alternative strategies for large result sets:
- **Sampling**: Return approximate results
- **Unordered**: Return without ordering, indicate ordering was skipped
- **Streaming**: Return results as they're found, without global ordering

## Performance Characteristics

| Operation | Complexity | Notes |
|-----------|------------|-------|
| Get entity by ID | O(1) | HashMap lookup |
| Get entity values | O(1) | Inline in entity |
| Get entity relations | O(1) | Inline in entity |
| Search by name in space | O(1) | Index lookup |
| Filter by property=value | O(1) | Index lookup |
| Filter by relation | O(1) | Index lookup |
| Filter + order (same prop) | O(n log n) | n = filtered candidates |
| Filter + order (traversed) | O(n × m log n) | m = relations per entity |
| Historical query | O(log n) | Binary search in segment |
| Apply op | O(k) | k = number of index updates |
| Snapshot save | O(n) | n = total entities |
| Snapshot load | O(n) | Plus index rebuild |

## Comparison to Postgres

| Aspect | This Design | Postgres |
|--------|-------------|----------|
| Entity lookup | 1 hashmap lookup | Multiple table joins |
| Memory usage | ~1 GB (current state only) | Higher (buffers, indexes, overhead) |
| Query planning | None needed (direct index) | Query planner overhead |
| Index updates | Inline with writes | WAL + background |
| Historical queries | Separate path (cold) | Same path (potentially slow) |
| Recovery | Snapshot + WAL replay | WAL replay |
| Operational complexity | Single binary | Separate service |

## Future Considerations

### Scaling Beyond RAM: ByteGraph's Approach

At larger scale, a tiered memory/disk architecture becomes necessary. [ByteGraph](https://www.mydistributed.systems/2023/01/bytegraph-graph-database-for-tiktok.html) (TikTok's graph database) provides a reference architecture:

```
┌─────────────────────────────────────┐
│     BGE (Execution Layer)           │  Query parsing, planning, 2PC
├─────────────────────────────────────┤
│     BGS (Cache Layer)               │  In-memory edge-trees
│                                     │
│     Root nodes ← always in memory   │
│     Meta nodes ← usually in memory  │
│     Data nodes ← paged on demand    │
├─────────────────────────────────────┤
│     Persistent KV Store (RocksDB)   │  Durable storage
└─────────────────────────────────────┘
```

**Key ideas:**

1. **Edge-trees**: B-tree-like structures index edges per vertex. Root/meta nodes stay in memory (~kilobytes), leaf nodes page from disk. A 3-layer tree can index 8 billion edges with only 1 disk I/O.

2. **Natural hot/cold separation**: Tree navigation data is small and always cached. Actual edge data is large and tiered by access pattern.

3. **Per-relation-type trees**: Super-vertices (celebrities with millions of followers) have separate trees per edge type, enabling parallel access.

**When we'd need this:**
- Data exceeds RAM (10+ GB)
- Entities with thousands of relations
- Need instant cold starts without full data load

At current scale (~1 GB), our simpler HashMap approach with checkpoints is sufficient.

### Other Scaling Options

If data grows beyond RAM:
1. **Tiered storage**: Hot entities in RAM, cold on disk (LRU eviction)
2. **Sharding**: Partition by space_id across nodes
3. **Memory-mapped files**: Let OS manage hot/cold

### Compression

For history segments:
- Dictionary encoding for repeated strings (property names, common values)
- Delta encoding for block numbers
- LZ4/Zstd for segment compression

### Replication

For high availability:
- Stream ops to replicas
- Replicas rebuild from same WAL
- Consistent reads from any replica at same block height
