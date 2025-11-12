#!/bin/bash

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_header() {
    echo -e "${BLUE}=== $1 ===${NC}"
}

# Get current context
CURRENT_CONTEXT=$(kubectl config current-context)
print_info "Current context: ${CURRENT_CONTEXT}"

# Detect environment
if [[ $CURRENT_CONTEXT == *"minikube"* ]]; then
    ENVIRONMENT="local"
elif [[ $CURRENT_CONTEXT == *"do-"* ]]; then
    ENVIRONMENT="digitalocean"
else
    ENVIRONMENT="unknown"
fi

echo ""
print_header "Service Connection Information"
echo ""

if [[ $ENVIRONMENT == "local" ]]; then
    print_info "Environment: Local (minikube)"
    echo ""
    echo "To access kafka-ui:"
    echo -e "  ${YELLOW}kubectl port-forward -n kafka svc/kafka-ui 8080:8080${NC}"
    echo "  Then visit: http://localhost:8080"
    echo ""
    echo "To access Kafka broker:"
    echo -e "  ${YELLOW}kubectl port-forward -n kafka svc/broker 9092:9092${NC}"
    echo -e "  Then use: ${YELLOW}KAFKA_BROKER=localhost:9092${NC}"
    echo ""
    echo "Run producer:"
    echo -e "  ${YELLOW}KAFKA_BROKER=localhost:9092 cargo run${NC}"
    echo ""

elif [[ $ENVIRONMENT == "digitalocean" ]]; then
    print_info "Environment: DigitalOcean"
    echo ""

    BROKER_IP=$(kubectl get svc broker -n kafka -o jsonpath='{.status.loadBalancer.ingress[0].ip}' 2>/dev/null || echo "")

    echo "kafka-ui (use port-forward):"
    echo -e "  ${YELLOW}kubectl port-forward -n kafka svc/kafka-ui 8080:8080${NC}"
    echo "  Then visit: http://localhost:8080"
    echo ""

    if [[ -n $BROKER_IP ]]; then
        echo "Kafka broker:"
        echo -e "  ${YELLOW}${BROKER_IP}:9092${NC}"
        echo ""
        echo "Run producer:"
        echo -e "  ${YELLOW}KAFKA_BROKER=${BROKER_IP}:9092 cargo run${NC}"
        echo ""
    else
        echo -e "Kafka broker: ${YELLOW}External IP pending...${NC}"
        echo ""
        echo "Run this command to check status:"
        echo -e "  ${YELLOW}kubectl get svc -n kafka${NC}"
        echo ""
    fi
fi

print_header "Useful Commands"
echo ""
echo "View pods:"
echo -e "  ${YELLOW}kubectl get pods -n kafka${NC}"
echo ""
echo "View logs:"
echo -e "  ${YELLOW}kubectl logs -n kafka -l app=kafka-broker --tail=50 -f${NC}"
echo -e "  ${YELLOW}kubectl logs -n kafka -l app=kafka-ui --tail=50 -f${NC}"
echo ""
echo "Cleanup:"
echo -e "  ${YELLOW}./k8s/cleanup.sh${NC}"
echo ""
