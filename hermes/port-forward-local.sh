#!/bin/bash

set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

# Check if kubectl is available
if ! command -v kubectl &> /dev/null; then
    print_warning "kubectl not found. Please install kubectl first."
    exit 1
fi

# Check current context
CURRENT_CONTEXT=$(kubectl config current-context)
print_info "Current kubectl context: ${CURRENT_CONTEXT}"

# Only allow port-forward for local (minikube)
if [[ $CURRENT_CONTEXT != *"minikube"* ]]; then
    print_warning "Port-forwarding is only needed for local (minikube) environments."
    print_warning "For DigitalOcean, services are exposed via LoadBalancer."
    print_warning "Run ./connect.sh to get connection information."
    exit 1
fi

print_info "Starting port-forwards for Kafka services..."
echo ""
print_info "Kafka broker: localhost:9092"
print_info "Kafka UI: http://localhost:8080"
echo ""
print_info "Press Ctrl+C to stop all port-forwards"
echo ""

# Trap to clean up background processes on exit
trap 'kill $(jobs -p) 2>/dev/null' EXIT

# Start port-forwards in background
kubectl port-forward -n kafka svc/broker 9092:9092 &
kubectl port-forward -n kafka svc/kafka-ui 8080:8080 &

# Wait for all background jobs
wait
