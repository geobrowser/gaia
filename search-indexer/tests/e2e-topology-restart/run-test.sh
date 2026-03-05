#!/usr/bin/env bash
#
# E2E Topology Persistence Restart-and-Restore Test
#
# Verifies that the indexer persists topology state to a JSON file and
# correctly restores it on restart — even when the Kafka topic is deleted,
# proving the graph is loaded from disk, not replayed from Kafka.
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
WORK_DIR=$(mktemp -d "${TMPDIR_BASE}/e2e-topology-restart.XXXXXX")

# ── Configuration ────────────────────────────────────────────────────────────
KAFKA_BROKER="${KAFKA_BROKER:-localhost:9092}"
OPENSEARCH_URL="${OPENSEARCH_URL:-http://localhost:9200}"
INDEX_NAME="staging_entities_v0"

# Consumer group IDs (test-specific to avoid collisions).
# The indexer prepends "staging-" via get_consumer_group_prefix() when
# ENVIRONMENT=staging, so the final groups are staging-topo-restart-test-group-*.
KAFKA_GROUP_EDITS_ID="topo-restart-test-group-edits"
KAFKA_GROUP_SCORES_ID="topo-restart-test-group-scores"
KAFKA_GROUP_SPACE_TOPICS_ID="topo-restart-test-group-space-topics"
KAFKA_GROUP_TOPOLOGY_ID="topo-restart-test-group-topology"

# Timeouts (seconds)
READY_TIMEOUT=30
POLL_TIMEOUT=30
POLL_INTERVAL=1

# Fixed UUIDs from e2e-kafka-search-api/src/main.rs
TOPO_ROOT_ID="00000000-0000-4000-8000-000000000c01"
TOPO_REMOVE_ME_ID="00000000-0000-4000-8000-000000000c05"

# Prefixed Kafka resource names (staging environment)
TOPIC_PREFIX="staging."
GROUP_PREFIX="staging-"
TOPICS=(
    "${TOPIC_PREFIX}knowledge.edits"
    "${TOPIC_PREFIX}curation.scores"
    "${TOPIC_PREFIX}space.topics"
    "${TOPIC_PREFIX}topology.canonical"
)
KAFKA_GROUPS=(
    "${GROUP_PREFIX}${KAFKA_GROUP_EDITS_ID}"
    "${GROUP_PREFIX}${KAFKA_GROUP_SCORES_ID}"
    "${GROUP_PREFIX}${KAFKA_GROUP_SPACE_TOPICS_ID}"
    "${GROUP_PREFIX}${KAFKA_GROUP_TOPOLOGY_ID}"
)

# Port counter — each indexer start gets a unique port to avoid bind conflicts
NEXT_PORT=8090

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
        for logfile in "$WORK_DIR"/indexer-*.log; do
            [[ -f "$logfile" ]] || continue
            echo -e "${RED}── $(basename "$logfile") (last 40 lines) ──${RESET}"
            tail -40 "$logfile" || true
            echo
        done
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

# Start the indexer with test env vars. Uses NEXT_PORT (incremented each call)
# to avoid port bind conflicts across restarts.
start_indexer() {
    local logfile="$1"
    local port=$NEXT_PORT
    (( NEXT_PORT++ ))
    CURRENT_PORT=$port
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
    KAFKA_GROUP_TOPOLOGY_ID="$KAFKA_GROUP_TOPOLOGY_ID" \
    TOPOLOGY_STATE_PATH="$WORK_DIR/topology_state.json" \
    HEALTH_PORT="$port" \
    "$INDEXER_BIN" > "$logfile" 2>&1 &
    INDEXER_PID=$!
}

# Stop the current indexer, waiting for exit.
stop_indexer() {
    if [[ -n "$INDEXER_PID" ]] && kill -0 "$INDEXER_PID" 2>/dev/null; then
        kill "$INDEXER_PID" 2>/dev/null || true
    fi
    wait "$INDEXER_PID" 2>/dev/null || true
    INDEXER_PID=""
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

# Build binaries (CI pre-builds; build locally if needed)
INDEXER_BIN="$REPO_ROOT/target/release/search-indexer"
PRODUCER_BIN="$SCRIPT_DIR/../e2e-kafka-search-api/target/release/e2e-kafka-search-api"

if [[ ! -x "$INDEXER_BIN" ]]; then
    info "Building search-indexer (release)"
    (cd "$REPO_ROOT" && cargo build --release -p search-indexer \
        --features search-indexer-repository/auto_index_creation 2>&1 | tail -1)
    pass "search-indexer built"
fi

if [[ ! -x "$PRODUCER_BIN" ]]; then
    info "Building e2e-kafka-search-api"
    (cd "$SCRIPT_DIR/../e2e-kafka-search-api" && cargo build --release 2>&1 | tail -1)
    pass "e2e-kafka-search-api built"
fi

if [[ ! -x "$INDEXER_BIN" ]]; then
    fail "Indexer binary not found at $INDEXER_BIN"
    exit 1
fi
if [[ ! -x "$PRODUCER_BIN" ]]; then
    fail "Producer binary not found at $PRODUCER_BIN"
    exit 1
fi

# ── Phase 2: Ingest topology + stop ─────────────────────────────────────────
header "Phase 2: Ingest topology + stop"

# Start indexer, produce all events, then poll for topology root.
# The entity consumer may NACK and crash the indexer due to version conflicts
# from concurrent bulk operations. If that happens, we restart on a new port
# and try again — the topology consumer resumes from its last committed offset.
MAX_ATTEMPTS=3
ROOT_FOUND=0
RUN_NUM=0

for attempt in $(seq 1 $MAX_ATTEMPTS); do
    (( RUN_NUM += 1 ))
    info "Starting search-indexer (run 1, attempt $attempt)"
    start_indexer "$WORK_DIR/indexer-run1-${attempt}.log"

    if ! wait_for_log "$WORK_DIR/indexer-run1-${attempt}.log" \
            "Ready to process events" "$READY_TIMEOUT" "Indexer ready (attempt $attempt)"; then
        FAILED=1; exit 1
    fi
    pass "Indexer ready on port $CURRENT_PORT (attempt $attempt)"

    # Only produce events on the first attempt — they persist in Kafka
    if (( attempt == 1 )); then
        info "Producing events via e2e-kafka-search-api"
        ENVIRONMENT=staging "$PRODUCER_BIN" --broker "$KAFKA_BROKER" > "$WORK_DIR/producer.log" 2>&1
        pass "Events produced"
    fi

    # Poll for topology root (also handles indexer crashing mid-poll)
    info "Polling GET /topology/root (timeout ${POLL_TIMEOUT}s)"
    ELAPSED=0
    while (( ELAPSED < POLL_TIMEOUT )); do
        ROOT_RESP=$(curl -sf "http://localhost:${CURRENT_PORT}/topology/root" 2>/dev/null || echo "")
        if echo "$ROOT_RESP" | grep -q "$TOPO_ROOT_ID"; then
            ROOT_FOUND=1
            break 2
        fi
        if ! kill -0 "$INDEXER_PID" 2>/dev/null; then
            info "Indexer crashed (attempt $attempt), will retry"
            wait "$INDEXER_PID" 2>/dev/null || true
            INDEXER_PID=""
            break
        fi
        sleep "$POLL_INTERVAL"
        (( ELAPSED += POLL_INTERVAL ))
    done

    # If still running but timed out, stop before retrying
    stop_indexer
done

if (( ! ROOT_FOUND )); then
    fail "Topology root not set after $MAX_ATTEMPTS attempts"
    FAILED=1; exit 1
fi
pass "Topology root is $TOPO_ROOT_ID"

info "Checking topology state file on disk"
if [[ ! -f "$WORK_DIR/topology_state.json" ]]; then
    fail "Topology state file not found at $WORK_DIR/topology_state.json"
    FAILED=1; exit 1
fi
pass "State file exists ($(wc -c < "$WORK_DIR/topology_state.json") bytes)"

info "Stopping indexer (SIGTERM)"
stop_indexer
pass "Indexer stopped (run 1)"

# ── Phase 3: Restart without topology topic → verify restore ────────────────
header "Phase 3: Restart without topology topic → verify restore"

info "Deleting all Kafka topics (no replay possible)"
for topic in "${TOPICS[@]}"; do
    docker exec "$KAFKA_CONTAINER" /opt/kafka/bin/kafka-topics.sh \
        --bootstrap-server localhost:9092 --delete --topic "$topic" 2>&1 > /dev/null || true
done
pass "All topics deleted"

info "Starting search-indexer (run 2) with same state path"
start_indexer "$WORK_DIR/indexer-run2.log"

info "Waiting for readiness (timeout ${READY_TIMEOUT}s)"
if ! wait_for_log "$WORK_DIR/indexer-run2.log" \
        "Ready to process events" "$READY_TIMEOUT" "Indexer ready (run 2)"; then
    FAILED=1; exit 1
fi
pass "Indexer ready (run 2) on port $CURRENT_PORT"

# Assert: root was restored from JSON
info "Checking GET /topology/root (should be restored from disk)"
ROOT_RESP=$(curl -sf "http://localhost:${CURRENT_PORT}/topology/root" 2>/dev/null || echo "")
if echo "$ROOT_RESP" | grep -q "$TOPO_ROOT_ID"; then
    pass "Root restored: $TOPO_ROOT_ID"
else
    fail "Root not restored. Response: $ROOT_RESP"
    FAILED=1; exit 1
fi

# Assert: correct subspace count (root + child_a + child_b + grandchild + moveable = 5)
info "Checking GET /topology/subspaces/$TOPO_ROOT_ID (expect count=5)"
SUBSPACES_RESP=$(curl -sf "http://localhost:${CURRENT_PORT}/topology/subspaces/$TOPO_ROOT_ID" 2>/dev/null || echo "")
SUBSPACE_COUNT=$(echo "$SUBSPACES_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['count'])" 2>/dev/null || echo "0")
if (( SUBSPACE_COUNT == 5 )); then
    pass "Subspace count is 5 (root + child_a + child_b + grandchild + moveable)"
else
    fail "Expected 5 subspaces, got $SUBSPACE_COUNT. Response: $SUBSPACES_RESP"
    FAILED=1; exit 1
fi

# Assert: removed space returns 404
info "Checking GET /topology/subspaces/$TOPO_REMOVE_ME_ID (expect 404)"
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "http://localhost:${CURRENT_PORT}/topology/subspaces/$TOPO_REMOVE_ME_ID" 2>/dev/null || echo "000")
if [[ "$HTTP_CODE" == "404" ]]; then
    pass "Removed space returns 404"
else
    fail "Expected 404 for removed space, got HTTP $HTTP_CODE"
    FAILED=1; exit 1
fi

# ── Phase 4: Cleanup ────────────────────────────────────────────────────────
header "Phase 4: Cleanup"

info "Stopping indexer"
stop_indexer
pass "Indexer stopped"

# ── Result ───────────────────────────────────────────────────────────────────
echo
echo -e "${GREEN}${BOLD}ALL PHASES PASSED${RESET}"
echo
echo "  Topology persistence restart-and-restore verified."
echo "  State was restored from JSON file, not Kafka replay."
echo
