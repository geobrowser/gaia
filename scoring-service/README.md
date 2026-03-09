# Scoring Service

A Python runtime for data processing.

## Prerequisites

- Python 3.12+
- [uv](https://docs.astral.sh/uv/) package manager
- [just](https://github.com/casey/just) command runner

## Commands

| Command | Description |
|---------|-------------|
| `just setup` | Install dependencies |
| `just setup-dev` | Install all dependencies including dev |
| `just run` | Run the application |
| `just test` | Run tests |
| `just test-cov` | Run tests with coverage report |

## Environment Variables

Copy `.env.example` to `.env` and configure the following variables:

### Required

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |

### Logging

| Variable | Default | Description |
|----------|---------|-------------|
| `LOG_LEVEL` | `INFO` | Logging verbosity |

### Monitoring & Telemetry (Optional)

Both **vote-indexer** (Rust) and **cronjob** (Python) support optional Sentry monitoring for production error tracking and performance monitoring.

**Without Sentry configured:**
- vote-indexer: Uses Console backend (logs spans to stdout)
- cronjob: Uses standard Python logging

**With Sentry configured:**
- Distributed tracing across pipeline stages
- Automatic error capture with context
- Performance monitoring and bottleneck identification

| Variable | Default | Description |
|----------|---------|-------------|
| `SENTRY_DSN` | - | Sentry ingest URL. If not set, monitoring is disabled |
| `SENTRY_TRACES_SAMPLE_RATE` | `1.0` | Trace sampling rate (0.0-1.0) |
| `SENTRY_ENVIRONMENT` | `production` | Environment tag (e.g., "production", "staging") |
| `SENTRY_RELEASE` | - | Release version (e.g., "vote-indexer@1.0.0") |
| `SENTRY_SEND_DEFAULT_PII` | `false` | Include personally identifiable information |
| `SENTRY_DEBUG` | `false` | Also log spans to stdout (useful for debugging) |

**Example local development with Sentry:**
```bash
export SENTRY_DSN="https://...@o0.ingest.sentry.io/..."
export SENTRY_ENVIRONMENT="development"
export SENTRY_DEBUG="true"  # Also see spans in stdout
```

### Memory Usage

The scoring cronjob loads all entities, perspectives, votes, users, and spaces into memory
for the full duration of the pipeline. Peak memory occurs during the Kafka emit phase, when
protobuf score objects are allocated alongside the original data structures.

#### Per-object memory estimates (CPython overhead included)

| Object | Approx. size | Count source |
|--------|-------------|--------------|
| Entity | ~200 bytes | `entities` table rows |
| Perspective | ~280 bytes | Distinct `(entity_id, space_id)` pairs in `values` table |
| Vote | ~200 bytes | `user_votes` rows (excluding removes) |
| User | ~250 bytes | Distinct members/editors |
| Space | ~300 bytes | `spaces` table rows |
| EntityScore (protobuf) | ~80 bytes | 1 per entity (emit phase) |
| PerspectiveScore (protobuf) | ~100 bytes | 1 per perspective (emit phase) |
| Space distance dict entry | ~120 bytes | S x S pairs (S = number of spaces) |

#### Example: 2M entities, 700 spaces (staging as of March 2026)

Assuming ~3M perspectives, 300 users, <100 votes:

| Phase | Estimated memory |
|-------|-----------------|
| After data fetch | ~1.3 GB (entities 400MB + perspectives 840MB + overhead) |
| During space ranking | ~1.4 GB (+ space distance dict ~60MB for 700 spaces) |
| During entity ranking | ~1.6 GB (+ normalization working sets, sorted copies) |
| During Kafka emit (peak) | ~2.1 GB (+ 160MB entity protos + 300MB perspective protos) |

This is why the job OOMs at a 2Gi limit. The **4Gi limit** provides ~2x headroom.

#### Example: 5M entities, 2K spaces

Assuming ~8M perspectives:

| Phase | Estimated memory |
|-------|-----------------|
| After data fetch | ~3.5 GB |
| During Kafka emit (peak) | ~5.0 GB |

At this scale, the memory limit would need to be bumped to **8Gi**.

#### Optimization opportunities

- **Stream Kafka emit**: Yield scores lazily instead of pre-building full lists, eliminating
  the protobuf duplication at peak (~20% reduction).
- **Chunk entity processing**: Process entities in batches during ranking to allow GC between
  chunks.
- **Perspective lazy loading**: Load only perspective IDs upfront, fetch full data per-batch.

### Scoring Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `USE_CONTESTATION_SCORE` | `False` | Enable contestation scoring |
| `USE_TIME_DECAY` | `False` | Enable time-based score decay |
| `TIME_DECAY_FACTOR` | `0.1` | Decay rate for time-based scoring |
| `INCLUDE_SUBSPACE_VOTES` | `False` | Include votes from subspaces |
| `USE_ACTIVITY_METRICS` | `True` | Factor in user activity |
| `USE_DISTANCE_WEIGHTING` | `True` | Weight scores by graph distance |
| `DISTANCE_WEIGHT_BASE` | `0.8` | Base weight for distance calculations |
| `MAX_DISTANCE` | `10` | Maximum graph traversal distance |
| `NORMALIZE_SCORES` | `True` | Normalize final scores |
| `NORMALIZATION_METHOD` | `z_score` | Method for score normalization |
| `FILTER_NON_MEMBERS` | `False` | Exclude non-member votes |
| `REQUIRE_SPACE_MEMBERSHIP` | `False` | Require space membership for voting |
