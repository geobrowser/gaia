#!/usr/bin/env bash
#
# E2E NACK Crash Test
#
# Verifies the NACK → crash → restart → replay guarantee using real
# infrastructure (Kafka, OpenSearch, search-indexer binary).
#
# Prerequisites:
#   Kafka on localhost:9092   — cd hermes && docker-compose up -d kafka
#   OpenSearch on localhost:9200 — cd search-indexer-deploy && docker-compose up -d opensearch
#
# Usage:
#   ./run-test.sh
#
set -euo pipefail

# ── Paths ────────────────────────────────────────────────────────────────────
REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TMPDIR_BASE="${TMPDIR:-/tmp}"
WORK_DIR=$(mktemp -d "${TMPDIR_BASE}/e2e-nack-crash.XXXXXX")

# ── Configuration ────────────────────────────────────────────────────────────
KAFKA_BROKER="localhost:9092"
OPENSEARCH_URL="http://localhost:9200"
INDEX_NAME="staging_entities_v0"

# Consumer group IDs (test-specific to avoid collisions).
# The indexer prepends "staging-" via get_consumer_group_prefix() when
# ENVIRONMENT=staging, so the final groups are staging-nack-test-group-*.
KAFKA_GROUP_EDITS_ID="nack-test-group-edits"
KAFKA_GROUP_SCORES_ID="nack-test-group-scores"
KAFKA_GROUP_SPACE_TOPICS_ID="nack-test-group-space-topics"

# Timeouts (seconds)
READY_TIMEOUT=60
CRASH_TIMEOUT=30
REPLAY_TIMEOUT=30
POLL_INTERVAL=1

# Entity IDs produced by e2e-kafka-search-api
ALICE_HIGH_ID="00000000-0000-0000-0000-0000000000f1"
BOB_ID="00000000-0000-0000-0000-000000000b0b"

# Prefixed Kafka resource names (staging environment)
TOPIC_PREFIX="staging."
GROUP_PREFIX="staging-"
TOPICS=(
    "${TOPIC_PREFIX}knowledge.edits"
    "${TOPIC_PREFIX}curation.scores"
    "${TOPIC_PREFIX}space.topics"
)
KAFKA_GROUPS=(
    "${GROUP_PREFIX}${KAFKA_GROUP_EDITS_ID}"
    "${GROUP_PREFIX}${KAFKA_GROUP_SCORES_ID}"
    "${GROUP_PREFIX}${KAFKA_GROUP_SPACE_TOPICS_ID}"
)

# ── Helpers ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
RESET='\033[0m'

pass()  { echo -e "  ${GREEN}✓ $1${RESET}"; }
fail()  { echo -e "  ${RED}✗ $1${RESET}"; }
info()  { echo -e "  ${YELLOW}→ $1${RESET}"; }
header(){ echo -e "\n${BOLD}$1${RESET}"; }

INDEXER_PID=""
FAILED=0

cleanup() {
    if [[ -n "$INDEXER_PID" ]] && kill -0 "$INDEXER_PID" 2>/dev/null; then
        kill "$INDEXER_PID" 2>/dev/null || true
        wait "$INDEXER_PID" 2>/dev/null || true
    fi
    if (( FAILED )); then
        echo
        echo -e "${RED}── Indexer log (first run) ──${RESET}"
        cat "$WORK_DIR/indexer-run1.log" 2>/dev/null | tail -40 || true
        echo
        echo -e "${RED}── Indexer log (second run) ──${RESET}"
        cat "$WORK_DIR/indexer-run2.log" 2>/dev/null | tail -40 || true
    fi
    # In CI, keep logs for artifact upload; locally, clean up
    if [[ -z "${CI:-}" ]]; then
        rm -rf "$WORK_DIR"
    fi
}
trap cleanup EXIT

# Wait for a pattern in a log file, with timeout.
wait_for_log() {
    local logfile="$1" pattern="$2" timeout="$3" label="$4"
    local elapsed=0
    while (( elapsed < timeout )); do
        if grep -q "$pattern" "$logfile" 2>/dev/null; then
            return 0
        fi
        sleep "$POLL_INTERVAL"
        (( elapsed += POLL_INTERVAL ))
    done
    fail "$label (timed out after ${timeout}s)"
    return 1
}

# ── Phase 1: Setup ───────────────────────────────────────────────────────────
header "Phase 1: Setup"

# Check prerequisites
info "Checking Kafka at $KAFKA_BROKER"
if ! timeout 5 bash -c 'cat < /dev/null > /dev/tcp/localhost/9092' 2>/dev/null; then
    fail "Kafka not reachable at $KAFKA_BROKER"
    echo "  Start it: cd hermes && docker-compose up -d kafka"
    exit 1
fi
pass "Kafka reachable"

info "Checking OpenSearch at $OPENSEARCH_URL"
if ! curl -sf "$OPENSEARCH_URL/_cluster/health" > /dev/null 2>&1; then
    fail "OpenSearch not reachable at $OPENSEARCH_URL"
    echo "  Start it: cd search-indexer-deploy && docker-compose up -d opensearch"
    exit 1
fi
pass "OpenSearch reachable"

# Find Kafka container for admin commands
if [[ -z "${KAFKA_CONTAINER:-}" ]]; then
    KAFKA_CONTAINER=$(docker ps --format '{{.Names}}' | grep -i kafka | grep -v ui | head -1)
fi
if [[ -z "$KAFKA_CONTAINER" ]]; then
    fail "Cannot find Kafka container (set KAFKA_CONTAINER env var in CI)"
    exit 1
fi
pass "Kafka container: $KAFKA_CONTAINER"

# Clean state
info "Cleaning OpenSearch index ($INDEX_NAME)"
curl -sf -X DELETE "$OPENSEARCH_URL/$INDEX_NAME" > /dev/null 2>&1 || true
pass "Index deleted (or didn't exist)"

info "Cleaning Kafka consumer groups"
for group in "${KAFKA_GROUPS[@]}"; do
    docker exec "$KAFKA_CONTAINER" /opt/kafka/bin/kafka-consumer-groups.sh \
        --bootstrap-server localhost:9092 --delete --group "$group" 2>&1 > /dev/null || true
done
pass "Consumer groups cleaned"

info "Cleaning Kafka topics"
for topic in "${TOPICS[@]}"; do
    docker exec "$KAFKA_CONTAINER" /opt/kafka/bin/kafka-topics.sh \
        --bootstrap-server localhost:9092 --delete --topic "$topic" 2>&1 > /dev/null || true
done
pass "Topics cleaned"

# Build binaries
info "Building search-indexer (release)"
(cd "$REPO_ROOT" && cargo build --release -p search-indexer \
    --features search-indexer-repository/auto_index_creation 2>&1 | tail -1)
pass "search-indexer built"

info "Building e2e-kafka-search-api"
(cd "$SCRIPT_DIR/../e2e-kafka-search-api" && cargo build --release 2>&1 | tail -1)
pass "e2e-kafka-search-api built"

INDEXER_BIN="$REPO_ROOT/target/release/search-indexer"
PRODUCER_BIN="$SCRIPT_DIR/../e2e-kafka-search-api/target/release/e2e-kafka-search-api"

if [[ ! -x "$INDEXER_BIN" ]]; then
    fail "Indexer binary not found at $INDEXER_BIN"
    exit 1
fi
if [[ ! -x "$PRODUCER_BIN" ]]; then
    fail "Producer binary not found at $PRODUCER_BIN"
    exit 1
fi

# ── Phase 2: NACK → Crash ───────────────────────────────────────────────────
header "Phase 2: NACK → Crash"

info "Starting search-indexer (run 1)"
# Point topology persistence at our per-run work directory. The indexer
# defaults to /data/topology_state.json, which isn't writable on CI runners
# and isn't a sensible shared location between test runs anyway.
ENVIRONMENT=staging \
RUST_LOG="info,search_indexer=debug" \
KAFKA_BROKER="$KAFKA_BROKER" \
OPENSEARCH_URL="$OPENSEARCH_URL" \
INDEX_ALIAS="entities" \
ENTITIES_INDEX_VERSION=0 \
OPENSEARCH_CONNECTION_MODE=retry \
OPENSEARCH_RETRY_INTERVAL_SECS=2 \
KAFKA_GROUP_EDITS_ID="$KAFKA_GROUP_EDITS_ID" \
KAFKA_GROUP_SCORES_ID="$KAFKA_GROUP_SCORES_ID" \
KAFKA_GROUP_SPACE_TOPICS_ID="$KAFKA_GROUP_SPACE_TOPICS_ID" \
TOPOLOGY_STATE_PATH="$WORK_DIR/topology_state.json" \
"$INDEXER_BIN" > "$WORK_DIR/indexer-run1.log" 2>&1 &
INDEXER_PID=$!

info "Waiting for readiness (timeout ${READY_TIMEOUT}s)"
if ! wait_for_log "$WORK_DIR/indexer-run1.log" \
        "Ready to process events" "$READY_TIMEOUT" "Indexer ready"; then
    FAILED=1; exit 1
fi
pass "Indexer ready"

info "Setting index write block on $INDEX_NAME"
BLOCK_RESP=$(curl -sf -X PUT "$OPENSEARCH_URL/$INDEX_NAME/_settings" \
    -H 'Content-Type: application/json' \
    -d '{"index.blocks.write": true}' 2>&1)
if ! echo "$BLOCK_RESP" | grep -q '"acknowledged":true'; then
    fail "Failed to set write block: $BLOCK_RESP"
    FAILED=1; exit 1
fi
pass "Write block set"

info "Producing events via e2e-kafka-search-api"
ENVIRONMENT=staging "$PRODUCER_BIN" --broker "$KAFKA_BROKER" > "$WORK_DIR/producer.log" 2>&1
pass "Events produced"

info "Waiting for indexer to crash (timeout ${CRASH_TIMEOUT}s)"
ELAPSED=0
INDEXER_EXITED=0
while (( ELAPSED < CRASH_TIMEOUT )); do
    if ! kill -0 "$INDEXER_PID" 2>/dev/null; then
        INDEXER_EXITED=1
        break
    fi
    sleep "$POLL_INTERVAL"
    (( ELAPSED += POLL_INTERVAL ))
done

if (( ! INDEXER_EXITED )); then
    fail "Indexer did not exit within ${CRASH_TIMEOUT}s"
    kill "$INDEXER_PID" 2>/dev/null || true
    FAILED=1; exit 1
fi

EXIT_CODE=0
wait "$INDEXER_PID" 2>/dev/null || EXIT_CODE=$?
INDEXER_PID=""

if (( EXIT_CODE == 0 )); then
    fail "Indexer exited with code 0 (expected non-zero)"
    FAILED=1; exit 1
fi
pass "Indexer exited with code $EXIT_CODE (non-zero)"

if grep -q "NACK" "$WORK_DIR/indexer-run1.log"; then
    pass "Log contains NACK"
else
    fail "Log does not contain NACK"
    FAILED=1; exit 1
fi

# ── Phase 3: Restart → Replay ───────────────────────────────────────────────
header "Phase 3: Restart → Replay"

info "Removing write block from $INDEX_NAME"
UNBLOCK_RESP=$(curl -sf -X PUT "$OPENSEARCH_URL/$INDEX_NAME/_settings" \
    -H 'Content-Type: application/json' \
    -d '{"index.blocks.write": null}' 2>&1)
if ! echo "$UNBLOCK_RESP" | grep -q '"acknowledged":true'; then
    fail "Failed to remove write block: $UNBLOCK_RESP"
    FAILED=1; exit 1
fi
pass "Write block removed"

info "Starting search-indexer (run 2)"
# Reuse the same TOPOLOGY_STATE_PATH as run 1 so the second run actually
# picks up the persisted state from the crashed run (that's the whole
# point of the restart-replay test).
ENVIRONMENT=staging \
RUST_LOG="info,search_indexer=debug" \
KAFKA_BROKER="$KAFKA_BROKER" \
OPENSEARCH_URL="$OPENSEARCH_URL" \
INDEX_ALIAS="entities" \
ENTITIES_INDEX_VERSION=0 \
OPENSEARCH_CONNECTION_MODE=retry \
OPENSEARCH_RETRY_INTERVAL_SECS=2 \
KAFKA_GROUP_EDITS_ID="$KAFKA_GROUP_EDITS_ID" \
KAFKA_GROUP_SCORES_ID="$KAFKA_GROUP_SCORES_ID" \
KAFKA_GROUP_SPACE_TOPICS_ID="$KAFKA_GROUP_SPACE_TOPICS_ID" \
TOPOLOGY_STATE_PATH="$WORK_DIR/topology_state.json" \
"$INDEXER_BIN" > "$WORK_DIR/indexer-run2.log" 2>&1 &
INDEXER_PID=$!

info "Waiting for readiness (timeout ${READY_TIMEOUT}s)"
if ! wait_for_log "$WORK_DIR/indexer-run2.log" \
        "Ready to process events" "$READY_TIMEOUT" "Indexer ready (run 2)"; then
    FAILED=1; exit 1
fi
pass "Indexer ready (run 2)"

info "Polling OpenSearch for replayed documents (timeout ${REPLAY_TIMEOUT}s)"
ELAPSED=0
DOC_FOUND=0
while (( ELAPSED < REPLAY_TIMEOUT )); do
    HITS=$(curl -sf "$OPENSEARCH_URL/$INDEX_NAME/_search" \
        -H 'Content-Type: application/json' \
        -d "{\"query\":{\"terms\":{\"entity_id\":[\"$ALICE_HIGH_ID\",\"$BOB_ID\"]}}}" 2>/dev/null \
        | python3 -c "import sys,json; print(json.load(sys.stdin)['hits']['total']['value'])" 2>/dev/null || echo "0")
    if (( HITS >= 2 )); then
        DOC_FOUND=1
        break
    fi
    sleep "$POLL_INTERVAL"
    (( ELAPSED += POLL_INTERVAL ))
done

if (( DOC_FOUND )); then
    pass "Found $HITS replayed documents (Alice High + Bob)"
else
    fail "Documents not found after ${REPLAY_TIMEOUT}s (got $HITS hits)"
    FAILED=1; exit 1
fi

# ── Phase 4: Cleanup ────────────────────────────────────────────────────────
header "Phase 4: Cleanup"

info "Stopping indexer"
kill "$INDEXER_PID" 2>/dev/null || true
wait "$INDEXER_PID" 2>/dev/null || true
INDEXER_PID=""
pass "Indexer stopped"

# ── Result ───────────────────────────────────────────────────────────────────
echo
echo -e "${GREEN}${BOLD}ALL PHASES PASSED${RESET}"
echo
echo "  NACK → crash → restart → replay guarantee verified."
echo
