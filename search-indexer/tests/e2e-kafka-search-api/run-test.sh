#!/bin/bash

# Quick test script to generate sample events for the search-indexer
# This script generates a comprehensive test scenario with entities, relations, and scores.

set -e

echo "🚀 Starting E2E Kafka Search API Quick Test"
echo ""
echo "This will generate test events in your local Kafka broker at localhost:9092"
echo "Topics: knowledge.edits and curation.scores"
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

echo "🎯 Generating test scenario..."
echo ""

# Run the event generator using cargo run (builds if needed)
cargo run --release

echo ""
echo "✅ Test events generated successfully!"
echo ""

# Check if search API is running and run TypeScript validation
if timeout 2 bash -c 'cat < /dev/null > /dev/tcp/localhost/3000' 2>/dev/null; then
    echo "🔍 Search API detected at localhost:3000, running validation tests..."
    echo ""

    # Install dependencies if needed
    if [ ! -d "typescript/node_modules" ]; then
        echo "📦 Installing validation script dependencies..."
        cd typescript && npm install --silent && cd ..
        echo ""
    fi

    # Run TypeScript validation
    cd typescript && npm run validate
    VALIDATION_EXIT_CODE=$?
    cd ..
    exit $VALIDATION_EXIT_CODE
else
    echo "ℹ️  Search API not detected at localhost:3000"
    echo ""
    echo "To run validation tests, start the search API and search-indexer:"
    echo ""
    echo "1. Start the search-indexer:"
    echo "   KAFKA_BROKER=localhost:9092 \\"
    echo "   OPENSEARCH_URL=http://localhost:9200 \\"
    echo "   KAFKA_GROUP_ID=search-indexer-test-\$(date +%s) \\"
    echo "   RUST_LOG=debug,search_indexer=debug \\"
    echo "   cargo run -p search-indexer --features search-indexer-repository/auto_index_creation"
    echo ""
    echo "2. Start the search API:"
    echo "   cd api && cargo run"
    echo ""
fi

echo "Additional manual checks:"
echo ""
echo "1. View events in Kafka UI:"
echo "   http://localhost:8080"
echo ""
echo "2. Query indexed entities in OpenSearch:"
echo "   curl -s \"http://localhost:9200/entities/_search?pretty\" | jq '.hits.hits[]._source.name'"
echo ""
echo "3. Query results via API:"
echo "   curl --compressed \"http://localhost:3000/search?query=alice\" | jq"
echo ""
