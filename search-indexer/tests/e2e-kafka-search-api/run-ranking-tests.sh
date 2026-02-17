#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

BLUE='\033[0;34m'
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

API_URL="${SEARCH_API_URL:-http://localhost:3000}"

echo -e "${BLUE}1. Checking search API availability...${NC}"
if ! curl -sf --max-time 2 "${API_URL}/search/health" > /dev/null 2>&1; then
  echo -e "${RED}Search API not detected at ${API_URL}${NC}"
  echo "Start the search API first: cd api && cargo run"
  exit 1
fi
echo -e "${GREEN}Search API is running at ${API_URL}${NC}"

echo -e "\n${BLUE}2. Building test publisher...${NC}"
cargo build --release

echo -e "\n${BLUE}3. Publishing ranking test entities to Kafka...${NC}"
ENVIRONMENT="${ENVIRONMENT:-staging}" cargo run --release

WAIT_SECS="${WAIT_SECS:-6}"
echo -e "\n${BLUE}4. Waiting ${WAIT_SECS}s for search-indexer to process...${NC}"
sleep "$WAIT_SECS"

echo -e "\n${BLUE}5. Validating rankings...${NC}\n"
cd typescript
SEARCH_API_URL="$API_URL" npx tsx validate-rankings.ts
