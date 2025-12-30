#!/bin/bash
# Run load tests with various configurations
# Usage: ./scripts/run-test.sh <test-type> [options]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

usage() {
    echo "Usage: $0 <command> [options]"
    echo ""
    echo "Commands:"
    echo "  http        Run HTTP load test"
    echo "  kafka       Run Kafka load test"
    echo "  combined    Run combined HTTP + Kafka load test"
    echo "  seed        Seed the index with documents"
    echo "  docker      Run test in Docker"
    echo ""
    echo "Options:"
    echo "  --profile <name>      Load profile: light, moderate, heavy, stress (default: moderate)"
    echo "  --api-url <url>       API URL (default: http://localhost:3000)"
    echo "  --kafka-brokers <str> Kafka brokers (default: localhost:9092)"
    echo "  --target-docs <n>     Target docs for seeding (default: 10000)"
    echo "  --metrics             Enable InfluxDB metrics output"
    echo "  --help                Show this help"
    echo ""
    echo "Examples:"
    echo "  $0 http --profile heavy --api-url http://search.staging:3000"
    echo "  $0 kafka --profile moderate --kafka-brokers kafka.staging:9092"
    echo "  $0 seed --target-docs 1000000"
    echo "  $0 docker http --profile light"
}

# Default values
PROFILE="moderate"
API_URL="http://localhost:3000"
KAFKA_BROKERS="localhost:9092"
KAFKA_TOPIC="knowledge.edits"
TARGET_DOCS="10000"
METRICS=""

# Parse arguments
COMMAND=""
while [[ $# -gt 0 ]]; do
    case $1 in
        http|kafka|combined|seed|docker)
            COMMAND=$1
            shift
            ;;
        --profile)
            PROFILE="$2"
            shift 2
            ;;
        --api-url)
            API_URL="$2"
            shift 2
            ;;
        --kafka-brokers)
            KAFKA_BROKERS="$2"
            shift 2
            ;;
        --kafka-topic)
            KAFKA_TOPIC="$2"
            shift 2
            ;;
        --target-docs)
            TARGET_DOCS="$2"
            shift 2
            ;;
        --metrics)
            METRICS="--out influxdb=http://localhost:8086/k6"
            shift
            ;;
        --help)
            usage
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            usage
            exit 1
            ;;
    esac
done

if [[ -z "$COMMAND" ]]; then
    echo -e "${RED}Error: No command specified${NC}"
    usage
    exit 1
fi

cd "$PROJECT_DIR"

case $COMMAND in
    http)
        echo -e "${GREEN}Running HTTP load test...${NC}"
        echo -e "  Profile: ${YELLOW}$PROFILE${NC}"
        echo -e "  API URL: ${YELLOW}$API_URL${NC}"
        k6 run http-load-test.js \
            -e API_URL="$API_URL" \
            -e PROFILE="$PROFILE" \
            $METRICS
        ;;
    kafka)
        echo -e "${GREEN}Running Kafka load test...${NC}"
        echo -e "  Profile: ${YELLOW}$PROFILE${NC}"
        echo -e "  Brokers: ${YELLOW}$KAFKA_BROKERS${NC}"
        k6 run kafka-load-test.js \
            -e KAFKA_BROKERS="$KAFKA_BROKERS" \
            -e KAFKA_TOPIC="$KAFKA_TOPIC" \
            -e KAFKA_PROFILE="$PROFILE" \
            $METRICS
        ;;
    combined)
        echo -e "${GREEN}Running combined load test...${NC}"
        echo -e "  API URL: ${YELLOW}$API_URL${NC}"
        echo -e "  Brokers: ${YELLOW}$KAFKA_BROKERS${NC}"
        k6 run combined-load-test.js \
            -e API_URL="$API_URL" \
            -e KAFKA_BROKERS="$KAFKA_BROKERS" \
            -e KAFKA_TOPIC="$KAFKA_TOPIC" \
            $METRICS
        ;;
    seed)
        echo -e "${GREEN}Seeding index with $TARGET_DOCS documents...${NC}"
        echo -e "  Brokers: ${YELLOW}$KAFKA_BROKERS${NC}"
        k6 run seed-index.js \
            -e KAFKA_BROKERS="$KAFKA_BROKERS" \
            -e KAFKA_TOPIC="$KAFKA_TOPIC" \
            -e TARGET_DOCS="$TARGET_DOCS" \
            $METRICS
        ;;
    docker)
        echo -e "${GREEN}Running test in Docker...${NC}"
        shift  # Remove 'docker' from args
        docker compose run --rm k6 run "$@"
        ;;
    *)
        echo -e "${RED}Unknown command: $COMMAND${NC}"
        usage
        exit 1
        ;;
esac

echo -e "${GREEN}Test complete!${NC}"

