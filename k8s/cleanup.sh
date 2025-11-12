#!/bin/bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

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

# Get current context
CURRENT_CONTEXT=$(kubectl config current-context)
print_info "Current kubectl context: ${CURRENT_CONTEXT}"

# Confirm deletion
print_warning "This will delete the entire 'kafka' namespace and all resources."
read -p "Are you sure? (yes/no): " confirmation

if [[ $confirmation != "yes" ]]; then
    print_info "Cleanup cancelled."
    exit 0
fi

print_info "Deleting Kafka infrastructure..."
kubectl delete namespace kafka

print_info "Cleanup complete!"
