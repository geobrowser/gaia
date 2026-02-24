#!/bin/bash
set -e
export ENVIRONMENT="${ENVIRONMENT:-staging}"

echo "Search-Indexer Load Test"
echo "Environment: $ENVIRONMENT | Scale: ${SCALE:-1.0} | Seed: ${SEED:-42}"

# Check prerequisites
if ! timeout 5 bash -c 'cat < /dev/null > /dev/tcp/localhost/9092' 2>/dev/null; then
    echo "ERROR: Kafka not reachable at localhost:9092"
    echo "Start it: cd hermes && docker-compose up -d kafka kafka-ui"
    exit 1
fi
if ! curl -sf http://localhost:9200/_cluster/health > /dev/null 2>&1; then
    echo "ERROR: OpenSearch not reachable at localhost:9200"
    echo "Start it: cd search-indexer-deploy && docker-compose up -d opensearch"
    exit 1
fi

# Build and run
cargo run --release -- \
    --seed "${SEED:-42}" \
    --scale "${SCALE:-1.0}" \
    --timeout "${TIMEOUT:-120}" \
    "$@"
