#!/bin/bash
# Test rate limiting against a running API instance.
# Uses a cheap cached query ({__typename}) so every request is fast.
#
# Usage:
#   ./scripts/test-rate-limit.sh                        # default: 20 requests to staging
#   ./scripts/test-rate-limit.sh 50                     # 50 requests
#   ./scripts/test-rate-limit.sh 50 https://custom-api  # 50 requests to custom URL

COUNT=${1:-20}
BASE_URL=${2:-https://api-testnet.geobrowser.io}
ENDPOINT="${BASE_URL}/graphql"

echo "Sending ${COUNT} requests to ${ENDPOINT}"
echo "---"

for i in $(seq 1 "$COUNT"); do
  HEADERS=$(curl -sI -X POST "$ENDPOINT" \
    -H "Content-Type: application/json" \
    -d '{"query":"{__typename}"}')

  STATUS=$(echo "$HEADERS" | head -1 | awk '{print $2}')
  LIMIT=$(echo "$HEADERS" | grep -i "ratelimit-limit:" | tr -d '\r' | awk '{print $2}')
  REMAINING=$(echo "$HEADERS" | grep -i "ratelimit-remaining:" | tr -d '\r' | awk '{print $2}')
  RESET=$(echo "$HEADERS" | grep -i "ratelimit-reset:" | tr -d '\r' | awk '{print $2}')

  printf "#%-4s  status=%s  limit=%s  remaining=%s  reset=%ss\n" \
    "$i" "${STATUS:-?}" "${LIMIT:-n/a}" "${REMAINING:-n/a}" "${RESET:-n/a}"

  if [ "$STATUS" = "429" ]; then
    echo ">>> Rate limited! Stopping."
    break
  fi
done
