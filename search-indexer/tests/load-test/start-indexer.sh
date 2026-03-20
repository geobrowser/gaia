#!/bin/bash
#
# Clean local OpenSearch + Kafka state, then start the search-indexer
# with ENVIRONMENT=staging and configurable log level.
#
# Usage:
#   ./start-indexer.sh          # info-level logs (default)
#   ./start-indexer.sh --debug  # debug-level logs
#
set -e

RUST_LOG="info"
for arg in "$@"; do
    case "$arg" in
        --debug) RUST_LOG="info,search_indexer=debug,search_indexer_repository=debug" ;;
        *) echo "Unknown arg: $arg"; exit 1 ;;
    esac
done

KAFKA_BROKER="localhost:9092"
OPENSEARCH_URL="http://localhost:9200"
INDEX_ALIAS="entities"

# Staging-prefixed names
INDEX_PREFIX="staging_"
TOPIC_PREFIX="staging."
GROUP_PREFIX="staging-"

TOPICS=(
    "${TOPIC_PREFIX}knowledge.edits"
    "${TOPIC_PREFIX}curation.scores"
    "${TOPIC_PREFIX}space.topics"
)
KAFKA_GROUPS=(
    "${GROUP_PREFIX}search-indexer-group-edits-local"
    "${GROUP_PREFIX}search-indexer-group-scores-local"
    "${GROUP_PREFIX}search-indexer-group-space-topics"
)

echo
echo "=== Clean & Start Search Indexer (staging) ==="
echo "  Log level: $RUST_LOG"
echo

# ---- Check prerequisites ----
if ! timeout 5 bash -c 'cat < /dev/null > /dev/tcp/localhost/9092' 2>/dev/null; then
    echo "ERROR: Kafka not reachable at $KAFKA_BROKER"
    echo "Start it: docker compose --profile infra up -d"
    exit 1
fi
if ! curl -sf "$OPENSEARCH_URL/_cluster/health" > /dev/null 2>&1; then
    echo "ERROR: OpenSearch not reachable at $OPENSEARCH_URL"
    echo "Start it: docker compose --profile infra up -d"
    exit 1
fi

# ---- Clean OpenSearch ----
echo "Cleaning OpenSearch index..."
# Resolve concrete indices behind the alias
CONCRETE=$(curl -sf "$OPENSEARCH_URL/_alias/${INDEX_PREFIX}${INDEX_ALIAS}" 2>/dev/null \
    | python3 -c "import sys,json; print(' '.join(json.load(sys.stdin).keys()))" 2>/dev/null || true)

if [ -n "$CONCRETE" ]; then
    for idx in $CONCRETE; do
        echo "  Deleting index: $idx"
        curl -sf -X DELETE "$OPENSEARCH_URL/$idx" > /dev/null
    done
else
    echo "  No existing index found for ${INDEX_PREFIX}${INDEX_ALIAS}"
fi

# ---- Find Kafka container ----
KAFKA_CONTAINER=$(docker ps --format '{{.Names}}' | grep -E '^(kafka|hermes-kafka)' | grep -v ui | head -1)
if [ -z "$KAFKA_CONTAINER" ]; then
    echo "ERROR: Cannot find Kafka container"
    exit 1
fi
echo "Using Kafka container: $KAFKA_CONTAINER"

# ---- Clean Kafka consumer groups ----
echo "Cleaning Kafka consumer groups..."
for group in "${KAFKA_GROUPS[@]}"; do
    echo "  Deleting group: $group"
    docker exec "$KAFKA_CONTAINER" /opt/kafka/bin/kafka-consumer-groups.sh \
        --bootstrap-server localhost:9092 --delete --group "$group" 2>&1 | sed 's/^/    /' || true
done

# ---- Clean Kafka topics ----
echo "Cleaning Kafka topics..."
for topic in "${TOPICS[@]}"; do
    echo "  Deleting topic: $topic"
    docker exec "$KAFKA_CONTAINER" /opt/kafka/bin/kafka-topics.sh \
        --bootstrap-server localhost:9092 --delete --topic "$topic" 2>&1 | sed 's/^/    /' || true
done

echo
echo "Starting search-indexer (ENVIRONMENT=staging, RUST_LOG=$RUST_LOG)..."
echo "  Press Ctrl-C to stop"
echo

cd "$(git rev-parse --show-toplevel)"

cleanup() {
    echo
    echo "Indexer stopped."
    echo
    echo "To stop infrastructure:"
    echo "  docker compose --profile infra down"
    echo
}
trap cleanup EXIT

ENVIRONMENT=staging \
RUST_LOG="$RUST_LOG" \
KAFKA_BROKER="$KAFKA_BROKER" \
OPENSEARCH_URL="$OPENSEARCH_URL" \
INDEX_ALIAS="$INDEX_ALIAS" \
ENTITIES_INDEX_VERSION=0 \
OPENSEARCH_CONNECTION_MODE=retry \
OPENSEARCH_RETRY_INTERVAL_SECS=5 \
KAFKA_GROUP_EDITS_ID=search-indexer-group-edits-local \
KAFKA_GROUP_SCORES_ID=search-indexer-group-scores-local \
KAFKA_GROUP_SPACE_TOPICS_ID=search-indexer-group-space-topics \
cargo run --release -p search-indexer --features search-indexer-repository/auto_index_creation
