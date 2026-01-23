#!/bin/bash
set -e

# Get the repository root (two directories up from this script)
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Parse arguments
SOURCE_VERSION=${1:-1}
TARGET_VERSION=${2:-2}

if [ -z "$1" ] || [ -z "$2" ]; then
    echo "Usage: $0 <source_version> <target_version>"
    echo ""
    echo "Example: $0 1 2"
    echo ""
    echo "Using defaults: source_version=$SOURCE_VERSION, target_version=$TARGET_VERSION"
    echo ""
fi

echo "==> Testing full-migration command in kind cluster"
echo "    Source: entities_v${SOURCE_VERSION}"
echo "    Target: entities_v${TARGET_VERSION}"
echo ""
echo "Prerequisites:"
echo "  - kind cluster 'search-test' should be running"
echo "  - Run setup-test-environment.sh first to set up the environment"
echo ""

# Check if kind cluster exists
if ! kind get clusters 2>/dev/null | grep -q "^search-test$"; then
    echo "❌ Kind cluster 'search-test' not found"
    echo "   Run ./setup-test-environment.sh first to create the test environment"
    exit 1
fi

echo "==> Rebuilding Docker image with latest code..."
docker build -f "$REPO_ROOT/search-admin/Dockerfile" -t search-admin:local "$REPO_ROOT"

echo "==> Loading image into kind cluster..."
kind load docker-image search-admin:local --name search-test

echo "==> Note: Assumes search-admin ServiceAccount exists with required permissions"

echo "==> Cleaning up any previous migration jobs..."
kubectl delete job opensearch-full-migration -n search 2>/dev/null || true

echo "==> Creating full-migration job..."
kubectl apply -f - <<EOF
---
apiVersion: batch/v1
kind: Job
metadata:
  name: opensearch-full-migration
  namespace: search
  labels:
    app: opensearch-admin
    task: full-migration
spec:
  ttlSecondsAfterFinished: 3600
  backoffLimit: 1
  template:
    metadata:
      labels:
        app: opensearch-admin
        task: full-migration
    spec:
      serviceAccountName: search-admin
      restartPolicy: Never
      containers:
      - name: full-migration
        image: search-admin:local
        imagePullPolicy: Never
        env:
        - name: OPENSEARCH_URL
          valueFrom:
            secretKeyRef:
              name: opensearch-credentials
              key: OPENSEARCH_URL
        - name: INDEX_ALIAS
          value: "entities"
        - name: SOURCE_VERSION
          value: "${SOURCE_VERSION}"
        - name: TARGET_VERSION
          value: "${TARGET_VERSION}"
        - name: NAMESPACE
          value: "search"
        - name: DEPLOYMENT_NAME
          value: "search-indexer"
        - name: RUST_LOG
          value: "info"
        command:
        - search-admin
        - full-migration
        - --source-version
        - "\$(SOURCE_VERSION)"
        - --target-version
        - "\$(TARGET_VERSION)"
        - --namespace
        - "\$(NAMESPACE)"
        - --deployment-name
        - "\$(DEPLOYMENT_NAME)"
        resources:
          requests:
            memory: "256Mi"
            cpu: "200m"
          limits:
            memory: "512Mi"
            cpu: "500m"
EOF

echo ""
echo "==> Job created. Watching logs..."
echo ""

# Wait for pod to be created
echo "  Waiting for pod to be created..."
until kubectl get pod -l job-name=opensearch-full-migration -n search 2>/dev/null | grep -q opensearch-full-migration; do
  sleep 2
done
echo "  ✓ Pod created"

# Wait for pod to be running (not just created)
echo "  Waiting for container to start..."
until kubectl get pod -l job-name=opensearch-full-migration -n search -o jsonpath='{.items[0].status.phase}' 2>/dev/null | grep -qE "Running|Succeeded"; do
  # Check if pod is in error state
  POD_STATUS=$(kubectl get pod -l job-name=opensearch-full-migration -n search -o jsonpath='{.items[0].status.phase}' 2>/dev/null)
  if [ "$POD_STATUS" = "Failed" ]; then
    echo "  ✗ Pod failed to start!"
    kubectl describe pod -l job-name=opensearch-full-migration -n search
    exit 1
  fi
  sleep 2
done
echo "  ✓ Container started"
echo ""

# Follow the logs
kubectl logs -n search -f job/opensearch-full-migration

echo ""
echo "==> Migration complete! Verifying results..."
echo ""

# Check deployment status
echo "=== Deployment Status ==="
kubectl get deployment search-indexer -n search

echo ""
echo "=== ENTITIES_INDEX_VERSION ==="
kubectl get deployment search-indexer -n search -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="ENTITIES_INDEX_VERSION")].value}'
echo ""
echo ""

# List indices using the list-indices job
echo "=== Indices and Aliases ==="
kubectl delete job opensearch-list-indices -n search 2>/dev/null || true

kubectl apply -f - <<EOF
---
apiVersion: batch/v1
kind: Job
metadata:
  name: opensearch-list-indices
  namespace: search
spec:
  ttlSecondsAfterFinished: 300
  backoffLimit: 1
  template:
    spec:
      restartPolicy: Never
      containers:
      - name: list-indices
        image: search-admin:local
        imagePullPolicy: Never
        env:
        - name: OPENSEARCH_URL
          valueFrom:
            secretKeyRef:
              name: opensearch-credentials
              key: OPENSEARCH_URL
        - name: INDEX_ALIAS
          value: "entities"
        - name: RUST_LOG
          value: "warn"
        command:
        - search-admin
        - list-indices
        - --detailed
EOF

# Wait for job to complete
kubectl wait --for=condition=complete job/opensearch-list-indices -n search --timeout=30s 2>/dev/null || true
kubectl logs -n search job/opensearch-list-indices
echo ""

echo ""
echo "==> ✓ Full migration test complete!"
echo ""
echo "To clean up:"
echo "  kubectl delete job opensearch-full-migration -n search"
echo "  kubectl delete namespace search"
echo "  kind delete cluster --name search-test"
