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
