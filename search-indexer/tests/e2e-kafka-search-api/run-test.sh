#!/bin/bash

# Quick test script to generate sample events for the search-indexer
# This script generates a comprehensive test scenario with entities, relations, and scores.

set -e

# Default to staging if ENVIRONMENT is not set
export ENVIRONMENT="${ENVIRONMENT:-staging}"

echo "🚀 Starting E2E Kafka Search API Quick Test"
echo ""
echo "Environment: $ENVIRONMENT"
echo "This will generate test events in your local Kafka broker at localhost:9092"

# Show prefixed topic names based on environment
if [ "$ENVIRONMENT" = "staging" ]; then
    echo "Topics: staging.knowledge.edits, staging.curation.scores, and staging.space.topics"
else
    echo "Topics: knowledge.edits, curation.scores, and space.topics"
fi
echo ""

# Check if Kafka is accessible
if ! timeout 5 bash -c 'cat < /dev/null > /dev/tcp/localhost/9092' 2>/dev/null; then
    echo "⚠️  Warning: Cannot connect to Kafka at localhost:9092"
    echo "   Make sure Kafka is running:"
    echo "   cd hermes && docker-compose up -d kafka kafka-ui"
    echo ""
    read -p "Continue anyway? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Check if search API is running before generating events
SEARCH_API_URL="${SEARCH_API_URL:-http://localhost:3000}"
echo "🔍 Checking Search API at $SEARCH_API_URL..."
if ! timeout 2 bash -c "cat < /dev/null > /dev/tcp/${SEARCH_API_URL#http://}" 2>/dev/null && \
   ! curl -sf "$SEARCH_API_URL/search/health" > /dev/null 2>&1; then
    echo "⚠️  Search API not detected at $SEARCH_API_URL"
    echo ""
    echo "To run the full test, start the search API and search-indexer first:"
    echo ""
    echo "1. Start the search-indexer:"
    echo "   ENVIRONMENT=staging \\"
    echo "   KAFKA_BROKER=localhost:9092 \\"
    echo "   OPENSEARCH_URL=http://localhost:9200 \\"
    echo "   KAFKA_GROUP_EDITS_ID=search-indexer-group-edits-test-\$(date +%s) \\"
    echo "   KAFKA_GROUP_SCORES_ID=search-indexer-group-scores-test-\$(date +%s) \\"
    echo "   RUST_LOG=debug,search_indexer=debug \\"
    echo "   cargo run -p search-indexer --features search-indexer-repository/auto_index_creation"
    echo ""
    echo "2. Start the search API:"
    echo "   cd api && bun run main.ts"
    echo ""
    exit 1
fi
echo "✅ Search API is reachable"
echo ""

# Install validation dependencies early if needed
if [ ! -d "typescript/node_modules" ]; then
    echo "📦 Installing validation script dependencies..."
    cd typescript && npm install --silent && cd ..
    echo ""
fi

echo "🎯 Generating test scenario..."
echo ""

# Run the event generator using cargo run (builds if needed)
cargo run --release

echo ""
echo "✅ Test events generated successfully!"
echo ""

echo "🔍 Running validation tests..."
echo ""

# Run TypeScript validation
cd typescript && npm run validate
VALIDATION_EXIT_CODE=$?
cd ..
exit $VALIDATION_EXIT_CODE
