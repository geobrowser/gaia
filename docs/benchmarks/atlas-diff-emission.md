# Atlas Diff Emission Benchmarks

This document tracks benchmark evidence for Atlas canonical diff emission performance.

## Scope

- DiffTracker throughput and allocation behavior
- Canonical recomputation + diff generation under varying graph sizes
- Regression guardrails for future refactors

## How To Run

From repo root:

```bash
cargo bench -p atlas
```

For focused benchmark runs:

```bash
cargo bench -p atlas graph_diff
```

## Environment Template

Record environment when collecting benchmark numbers:

- CPU model:
- Core count:
- Memory:
- OS + kernel:
- Rust toolchain:
- Commit SHA:

## Latest Reference Results

Reference numbers currently used in planning/PR context:

| Nodes | Bootstrap | No Change | Throughput |
|------:|----------:|----------:|-----------:|
| 1,000 | 37 us | 33 us | ~27-31 M nodes/s |
| 10,000 | 479 us | 484 us | ~20-21 M nodes/s |
| 50,000 | 3.2 ms | 3.1 ms | ~15-17 M nodes/s |
| 100,000 | 8.3 ms | 7.0 ms | ~12-14 M nodes/s |

These are directional and hardware-dependent. Use them as sanity bounds, not strict SLOs.

## Interpretation Notes

- Empty/no-change blocks should remain low-overhead.
- Large-node performance should not regress materially across releases.
- Track allocation growth and GC/allocator pressure when changing diff data structures.

## Related

- `docs/specs/atlas-canonical-graph-spec.md`
- `atlas/src/graph/diff.rs`
- `atlas/benches/graph_diff.rs`
