# Atlas Benchmarks

Performance benchmarks for Atlas graph processing.

## Running Benchmarks

```bash
cargo bench -p atlas
```

Results are saved to `target/criterion/` with HTML reports.

## Benchmark Suites

### Canonical Graph Benchmarks (`benches/canonical.rs`)

Measures canonical graph computation performance.

| Benchmark | Description |
|-----------|-------------|
| `canonical_computation` | Full canonical graph computation at various sizes |
| `canonical_phase1` | Phase 1 only (explicit-edge BFS traversal) |
| `canonical_with_topics` | Full computation with topic edges (Phase 1 + Phase 2) |
| `affects_canonical` | Check if an event affects the canonical set |
| `change_detection` | Hash-based change detection between computes |
| `subtree_filtering` | Filtering with varying canonical set densities |
| `end_to_end` | Realistic scenario with 1000 canonical + 500 non-canonical nodes |

### Transitive Graph Benchmarks (`benches/transitive.rs`)

Measures BFS traversal and caching performance.

| Benchmark | Description |
|-----------|-------------|
| `bfs_linear_chain` | Linear chain: 0 → 1 → 2 → ... → n |
| `bfs_wide_graph` | Wide graph: root → [1, 2, 3, ..., n] |
| `bfs_binary_tree` | Binary tree of varying depths |
| `bfs_random_graph` | Random graphs with varying edge density |
| `full_vs_explicit_only` | Full transitive vs explicit-only traversal |
| `cache` | Cache miss vs cache hit performance |
| `tree_hashing` | Hash computation for change detection |
| `graph_state_events` | Event application (space creation, edge addition) |
| `cache_invalidation` | Cache invalidation on edge addition |
| `memory_sizes` | Memory usage measurements |

## Results

Benchmarks run on Apple Silicon (M-series). Times are median values.

### Canonical Graph Computation

| Nodes | Time | Throughput |
|------:|-----:|-----------:|
| 100 | 40 µs | 2.5 M nodes/s |
| 500 | 252 µs | 2.0 M nodes/s |
| 1,000 | 494 µs | 2.0 M nodes/s |
| 5,000 | 2.5 ms | 2.0 M nodes/s |

### Phase 1 Only (Explicit Edges)

| Nodes | Time | Throughput |
|------:|-----:|-----------:|
| 100 | 37 µs | 2.7 M nodes/s |
| 500 | 192 µs | 2.6 M nodes/s |
| 1,000 | 403 µs | 2.5 M nodes/s |
| 5,000 | 2.2 ms | 2.3 M nodes/s |

### With Topic Edges

| Scenario | Canonical | Non-Canonical | Time |
|----------|----------:|--------------:|-----:|
| Small | 100 | 50 | 333 µs |
| Medium | 500 | 200 | 4.8 ms |
| Large | 1,000 | 500 | 41 ms |

### BFS Traversal (Linear Chain)

| Nodes | Time | Throughput |
|------:|-----:|-----------:|
| 100 | 42 µs | 2.4 M nodes/s |
| 500 | 254 µs | 2.0 M nodes/s |
| 1,000 | 502 µs | 2.0 M nodes/s |
| 5,000 | 2.4 ms | 2.1 M nodes/s |

### BFS Traversal (Wide Graph)

| Nodes | Time | Throughput |
|------:|-----:|-----------:|
| 100 | 33 µs | 3.0 M nodes/s |
| 500 | 184 µs | 2.7 M nodes/s |
| 1,000 | 360 µs | 2.8 M nodes/s |
| 5,000 | 1.7 ms | 3.0 M nodes/s |

### BFS Traversal (Binary Tree)

| Nodes | Depth | Time | Throughput |
|------:|------:|-----:|-----------:|
| 127 | 6 | 55 µs | 2.3 M nodes/s |
| 511 | 8 | 221 µs | 2.3 M nodes/s |
| 2,047 | 10 | 914 µs | 2.2 M nodes/s |
| 8,191 | 12 | 3.8 ms | 2.1 M nodes/s |

### Cache Performance

| Operation | Nodes | Time |
|-----------|------:|-----:|
| Cache miss | 1,000 | 641 µs |
| Cache hit | 1,000 | 43 ns |

Cache hits are ~15,000x faster than misses.

### Change Detection

| Operation | Time |
|-----------|-----:|
| First compute | 446 µs |
| Second compute (no change) | 47 µs |

~9x faster when no changes detected.

### Event Checks

| Operation | Time |
|-----------|-----:|
| affects_canonical (canonical source) | 16 ns |
| affects_canonical (non-canonical source) | 16 ns |

### Tree Hashing

| Nodes | Time | Throughput |
|------:|-----:|-----------:|
| 100 | 2.5 µs | 40 M nodes/s |
| 500 | 12 µs | 40 M nodes/s |
| 1,000 | 29 µs | 35 M nodes/s |
| 5,000 | 148 µs | 34 M nodes/s |

## Memory Usage

### GraphState

| Nodes | Edges | Total | Per Node | Per Edge |
|------:|------:|------:|---------:|---------:|
| 100 | 195 | 39 KB | 401 B | 205 B |
| 1,000 | 3,997 | 564 KB | 577 B | 144 B |
| 5,000 | 19,996 | 2.4 MB | 500 B | 125 B |
| 10,000 | 39,994 | 4.8 MB | 500 B | 125 B |
| 50,000 | 199,997 | 21 MB | 438 B | 109 B |

Memory per node/edge decreases at scale due to HashMap amortization.

### TransitiveGraph (Single Graph)

| Nodes | Total | Tree | FlatSet |
|------:|------:|-----:|--------:|
| 100 | 28 KB | 25 KB | 3.5 KB |
| 1,000 | 306 KB | 250 KB | 56 KB |
| 5,000 | 1.4 MB | 1.2 MB | 224 KB |
| 10,000 | 2.9 MB | 2.4 MB | 448 KB |

Tree structure dominates memory (~85% of total).

### Cache Memory

| Cached Graphs | Nodes/Graph | Cache Total | Per Graph |
|--------------:|------------:|------------:|----------:|
| 10 | 100 | 1.6 MB | 163 KB |
| 100 | 100 | 155 MB | 1.6 MB |
| 10 | 1,000 | 14 MB | 1.4 MB |
| 100 | 1,000 | 1.6 GB | 16 MB |

Cache memory grows linearly with both graph count and size.

## Performance Characteristics

### Scaling

- **Linear scaling**: Throughput is roughly constant (~2M nodes/s) across graph sizes
- **Wide graphs faster**: Shallow traversal (wide graph) is ~30% faster than deep (linear chain)
- **Topic edges expensive**: Adding topic resolution increases latency significantly

### Caching

- **Cache hits are critical**: 15,000x speedup justifies the memory cost
- **Selective invalidation**: Only affected caches are invalidated on events
- **Memory tradeoff**: Large caches (100+ graphs) can consume significant memory

### Change Detection

- **Hash-based**: Tree hashing enables fast no-change detection
- **~9x speedup**: When graph hasn't changed, recomputation is avoided
