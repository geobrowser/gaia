#!/bin/bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

# Check if kubectl is available
if ! command -v kubectl &> /dev/null; then
    print_error "kubectl not found. Please install kubectl first."
    exit 1
fi

# Check cluster connection
if ! kubectl cluster-info &> /dev/null; then
    print_error "Cannot connect to Kubernetes cluster. Please check your connection."
    exit 1
fi

# Get current context
CURRENT_CONTEXT=$(kubectl config current-context)
print_info "Current kubectl context: ${CURRENT_CONTEXT}"

# Detect environment
if [[ $CURRENT_CONTEXT == *"minikube"* ]]; then
    ENVIRONMENT="local"
    print_info "Detected environment: Local (minikube)"
elif [[ $CURRENT_CONTEXT == *"do-"* ]]; then
    ENVIRONMENT="digitalocean"
    print_info "Detected environment: DigitalOcean"
else
    print_warning "Unknown environment. Proceeding anyway..."
    ENVIRONMENT="unknown"
fi

# Deploy resources using Kustomize
print_info "Deploying Kafka infrastructure to ${ENVIRONMENT}..."

if [[ $ENVIRONMENT == "local" ]]; then
    print_info "Applying local overlay..."
    kubectl apply -k overlays/local
elif [[ $ENVIRONMENT == "digitalocean" ]]; then
    print_info "Applying DigitalOcean overlay..."
    kubectl apply -k overlays/digitalocean
else
    print_warning "Unknown environment, applying base configuration..."
    kubectl apply -k base
fi

# Wait for pods to be ready
print_info "Waiting for pods to be ready..."
kubectl wait --for=condition=ready pod -l app=kafka-broker -n kafka --timeout=120s
kubectl wait --for=condition=ready pod -l app=kafka-ui -n kafka --timeout=120s

print_info "Deployment complete!"
echo ""

# Display service information
print_info "Service status:"
kubectl get svc -n kafka

echo ""

# Environment-specific instructions
if [[ $ENVIRONMENT == "local" ]]; then
    print_info "To access services locally:"
    echo ""
    echo "  kafka-ui:"
    echo "    kubectl port-forward -n kafka svc/kafka-ui 8080:8080"
    echo "    Then visit: http://localhost:8080"
    echo ""
    echo "  Kafka broker:"
    echo "    kubectl port-forward -n kafka svc/broker 9092:9092"
    echo "    Then use: KAFKA_BROKER=localhost:9092"
    echo ""
elif [[ $ENVIRONMENT == "digitalocean" ]]; then
    print_info "Waiting for LoadBalancer external IPs..."

    # Wait for external IPs
    sleep 10

    KAFKA_UI_IP=$(kubectl get svc kafka-ui -n kafka -o jsonpath='{.status.loadBalancer.ingress[0].ip}' 2>/dev/null || echo "pending")
    BROKER_IP=$(kubectl get svc broker -n kafka -o jsonpath='{.status.loadBalancer.ingress[0].ip}' 2>/dev/null || echo "pending")

    if [[ $KAFKA_UI_IP != "pending" ]]; then
        print_info "kafka-ui is available at: http://${KAFKA_UI_IP}:8080"
    else
        print_warning "kafka-ui external IP still pending. Run: kubectl get svc -n kafka"
    fi

    if [[ $BROKER_IP != "pending" ]]; then
        print_info "Kafka broker is available at: ${BROKER_IP}:9092"
        echo "    Use: KAFKA_BROKER=${BROKER_IP}:9092"
    else
        print_warning "Kafka broker external IP still pending. Run: kubectl get svc -n kafka"
    fi
fi

echo ""
print_info "View logs with:"
echo "  kubectl logs -n kafka -l app=kafka-broker --tail=50 -f"
echo "  kubectl logs -n kafka -l app=kafka-ui --tail=50 -f"
