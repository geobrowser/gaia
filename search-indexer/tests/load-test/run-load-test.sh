#!/bin/bash
set -e
export ENVIRONMENT="${ENVIRONMENT:-staging}"

echo "Search-Indexer Load Test"
echo "Environment: $ENVIRONMENT | Scale: ${SCALE:-1.0} | Seed: ${SEED:-42}"

# Check prerequisites
if ! timeout 5 bash -c 'cat < /dev/null > /dev/tcp/localhost/9092' 2>/dev/null; then
    echo "ERROR: Kafka not reachable at localhost:9092"
    echo "Start it: docker compose --profile infra up -d"
    exit 1
fi
if ! curl -sf http://localhost:9200/_cluster/health > /dev/null 2>&1; then
    echo "ERROR: OpenSearch not reachable at localhost:9200"
    echo "Start it: docker compose --profile infra up -d"
    exit 1
fi

# Default scores group ID matches start-indexer.sh
SCORES_GROUP="${SCORES_GROUP:-search-indexer-group-scores-local}"

# Build and run (edition 2024 requires nightly)
cargo +nightly run --release -- \
    --seed "${SEED:-42}" \
    --scale "${SCALE:-1.0}" \
    --timeout "${TIMEOUT:-300}" \
    --scores-group-id "$SCORES_GROUP" \
    "$@"
