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
