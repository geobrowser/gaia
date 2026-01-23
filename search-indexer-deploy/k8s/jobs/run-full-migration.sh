#!/bin/bash
set -e

# Parse arguments
KUBECONFIG_FLAG=""
SOURCE_VERSION=""
TARGET_VERSION=""

while [[ $# -gt 0 ]]; do
  case $1 in
    --kubeconfig=*)
      KUBECONFIG_PATH="${1#*=}"
      KUBECONFIG_FLAG="--kubeconfig=$KUBECONFIG_PATH"
      shift
      ;;
    --kubeconfig)
      KUBECONFIG_PATH="$2"
      KUBECONFIG_FLAG="--kubeconfig=$KUBECONFIG_PATH"
      shift 2
      ;;
    *)
      if [ -z "$SOURCE_VERSION" ]; then
        SOURCE_VERSION="$1"
      elif [ -z "$TARGET_VERSION" ]; then
        TARGET_VERSION="$1"
      else
        echo "Unknown argument: $1"
        echo "Usage: $0 [--kubeconfig=<path>] <source_version> <target_version>"
        exit 1
      fi
      shift
      ;;
  esac
done

if [ -z "$SOURCE_VERSION" ] || [ -z "$TARGET_VERSION" ]; then
    echo "Usage: $0 [--kubeconfig=<path>] <source_version> <target_version>"
    echo ""
    echo "Example: $0 2 3"
    echo "Example: $0 --kubeconfig=~/.kube/prod-config 2 3"
    echo ""
    echo "This will migrate from entities_v{source} to entities_v{target}"
    exit 1
fi

echo "==> Running full migration"
echo "    Source: entities_v${SOURCE_VERSION}"
echo "    Target: entities_v${TARGET_VERSION}"
if [ -n "$KUBECONFIG_FLAG" ]; then
  echo "    Kubeconfig: $KUBECONFIG_PATH"
fi
echo ""

# Clean up any previous job
echo "==> Cleaning up previous migration job (if exists)..."
kubectl $KUBECONFIG_FLAG delete job opensearch-full-migration -n search 2>/dev/null || true
echo ""

echo "==> Creating migration job..."
kubectl $KUBECONFIG_FLAG apply -f - <<EOF
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
  ttlSecondsAfterFinished: 86400
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
        image: registry.digitalocean.com/geo/search-admin:latest
        env:
        - name: OPENSEARCH_URL
          valueFrom:
            secretKeyRef:
              name: search-indexer-secrets
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
echo "==> Following migration logs..."
echo "    (Press Ctrl+C to stop following logs - migration will continue)"
echo ""

# Wait for pod to be created
until kubectl $KUBECONFIG_FLAG get pod -l job-name=opensearch-full-migration -n search 2>/dev/null | grep -q opensearch-full-migration; do
  sleep 2
done

# Follow the logs
kubectl $KUBECONFIG_FLAG logs -n search -f job/opensearch-full-migration

echo ""
echo "==> Migration complete!"
echo ""
echo "To verify:"
if [ -n "$KUBECONFIG_FLAG" ]; then
  echo "  kubectl $KUBECONFIG_FLAG get deployment search-indexer -n search -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name==\"ENTITIES_INDEX_VERSION\")].value}'"
  echo ""
  echo "To clean up the job:"
  echo "  kubectl $KUBECONFIG_FLAG delete job opensearch-full-migration -n search"
else
  echo "  kubectl get deployment search-indexer -n search -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name==\"ENTITIES_INDEX_VERSION\")].value}'"
  echo ""
  echo "To clean up the job:"
  echo "  kubectl delete job opensearch-full-migration -n search"
fi
