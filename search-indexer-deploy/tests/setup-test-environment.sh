#!/bin/bash
set -e

# Get the repository root (two directories up from this script)
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echo "==> Cleaning up any existing test environment..."

# Check if kind cluster exists and delete it
# This ensures a clean state even if a previous test run was interrupted
if kind get clusters 2>/dev/null | grep -q "^search-test$"; then
    echo "  Found existing 'search-test' cluster, deleting..."
    kind delete cluster --name search-test
    echo "  Cluster deleted"
fi

# Clean up any leftover Docker images from previous runs
# This ensures we always build a fresh image with the latest code
if docker images | grep -q "search-admin.*local"; then
    echo "  Removing old search-admin:local images..."
    docker rmi search-admin:local 2>/dev/null || true
fi

echo "==> Creating kind cluster..."
kind create cluster --name search-test

echo "==> Deploying OpenSearch..."
kubectl create namespace search
kubectl apply -f - <<EOF
apiVersion: v1
kind: Service
metadata:
  name: opensearch
  namespace: search
spec:
  ports:
  - port: 9200
    targetPort: 9200
  selector:
    app: opensearch
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: opensearch
  namespace: search
spec:
  replicas: 1
  selector:
    matchLabels:
      app: opensearch
  template:
    metadata:
      labels:
        app: opensearch
    spec:
      containers:
      - name: opensearch
        image: opensearchproject/opensearch:2.17.1
        env:
        - name: discovery.type
          value: single-node
        - name: DISABLE_SECURITY_PLUGIN
          value: "true"
        - name: OPENSEARCH_JAVA_OPTS
          value: "-Xms512m -Xmx512m"
        ports:
        - containerPort: 9200
        resources:
          limits:
            memory: 1Gi
          requests:
            memory: 512Mi
EOF

echo "==> Waiting for OpenSearch (this may take 2-3 minutes)..."
# Wait for pod to be created first
until kubectl get pod -l app=opensearch -n search 2>/dev/null | grep -q opensearch; do
  echo "  Waiting for pod to be created..."
  sleep 2
done
# Now wait for it to be ready
kubectl wait --for=condition=ready pod -l app=opensearch -n search --timeout=300s

echo "==> Creating secret..."
kubectl create secret generic opensearch-credentials \
  --from-literal=OPENSEARCH_URL=http://opensearch.search.svc.cluster.local:9200 \
  -n search

echo "==> Creating ServiceAccount and RBAC for search-admin..."
kubectl apply -f - <<EOF
apiVersion: v1
kind: ServiceAccount
metadata:
  name: search-admin
  namespace: search
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: search-admin-role
  namespace: search
rules:
- apiGroups: ["apps"]
  resources: ["statefulsets"]
  verbs: ["get", "list", "patch", "update"]
- apiGroups: [""]
  resources: ["pods"]
  verbs: ["get", "list", "watch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: search-admin-binding
  namespace: search
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: search-admin-role
subjects:
- kind: ServiceAccount
  name: search-admin
  namespace: search
EOF

echo "==> Building and loading image..."
docker build -f "$REPO_ROOT/search-admin/Dockerfile" -t search-admin:local "$REPO_ROOT"
kind load docker-image search-admin:local --name search-test

echo "==> Creating initial index..."
kubectl apply -f - <<EOF
apiVersion: batch/v1
kind: Job
metadata:
  name: opensearch-create-index-v1
  namespace: search
spec:
  ttlSecondsAfterFinished: 300
  backoffLimit: 3
  template:
    spec:
      restartPolicy: OnFailure
      containers:
      - name: create-index
        image: search-admin:local
        imagePullPolicy: Never
        env:
        - name: OPENSEARCH_URL
          value: "http://opensearch.search.svc.cluster.local:9200"
        - name: INDEX_ALIAS
          value: "entities"
        - name: INDEX_VERSION
          value: "1"
        - name: RUST_LOG
          value: "info"
        command:
        - search-admin
        - create-index
        - --version
        - "1"
        - --skip-if-exists
EOF

kubectl wait --for=condition=complete job/opensearch-create-index-v1 -n search --timeout=60s
kubectl logs -n search job/opensearch-create-index-v1

echo "==> Adding test data..."
kubectl port-forward -n search svc/opensearch 9200:9200 &
PF_PID=$!
sleep 3

curl -X POST "http://localhost:9200/entities_v1/_bulk" -H "Content-Type: application/x-ndjson" -d '
{"index":{"_id":"1"}}
{"id":"0x1234","name":"Test Entity 1","entity_type":"address","chain_id":1}
{"index":{"_id":"2"}}
{"id":"0x5678","name":"Test Entity 2","entity_type":"address","chain_id":1}
{"index":{"_id":"3"}}
{"id":"0x9abc","name":"Test Proposal 1","entity_type":"proposal","chain_id":1}
{"index":{"_id":"4"}}
{"id":"0xdef0","name":"Test Entity 4","entity_type":"address","chain_id":1}
{"index":{"_id":"5"}}
{"id":"0x1111","name":"Test Entity 5","entity_type":"address","chain_id":1}
'

curl -X POST "http://localhost:9200/entities_v1/_refresh"
echo ""
echo "=== Document count ==="
curl -s "http://localhost:9200/entities_v1/_count" | jq
echo ""

kill $PF_PID 2>/dev/null || true

echo "==> Updating alias..."
kubectl apply -f - <<EOF
apiVersion: batch/v1
kind: Job
metadata:
  name: opensearch-update-alias-v1
  namespace: search
spec:
  ttlSecondsAfterFinished: 300
  backoffLimit: 3
  template:
    spec:
      restartPolicy: OnFailure
      containers:
      - name: update-alias
        image: search-admin:local
        imagePullPolicy: Never
        env:
        - name: OPENSEARCH_URL
          value: "http://opensearch.search.svc.cluster.local:9200"
        - name: INDEX_ALIAS
          value: "entities"
        - name: TARGET_VERSION
          value: "1"
        - name: RUST_LOG
          value: "info"
        command:
        - search-admin
        - update-alias
        - --version
        - "1"
EOF

kubectl wait --for=condition=complete job/opensearch-update-alias-v1 -n search --timeout=60s
kubectl logs -n search job/opensearch-update-alias-v1

echo "==> Deploying test indexer..."
kubectl apply -f - <<EOF
apiVersion: v1
kind: Service
metadata:
  name: search-indexer
  namespace: search
spec:
  clusterIP: None
  selector:
    app: search-indexer
  ports:
    - name: http
      port: 8080
      targetPort: 8080
---
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: search-indexer
  namespace: search
spec:
  serviceName: search-indexer
  replicas: 1
  selector:
    matchLabels:
      app: search-indexer
  template:
    metadata:
      labels:
        app: search-indexer
    spec:
      containers:
      - name: search-indexer
        image: busybox:latest
        command: ["sleep", "infinity"]
        env:
        - name: ENTITIES_INDEX_VERSION
          value: "1"
EOF

# Wait for pod to be created first
until kubectl get pod -l app=search-indexer -n search 2>/dev/null | grep -q search-indexer; do
  echo "  Waiting for search-indexer pod to be created..."
  sleep 2
done
kubectl wait --for=condition=ready pod -l app=search-indexer -n search --timeout=60s

echo ""
echo "==> ✓ Setup complete!"
echo ""
echo "Your local test environment is ready:"
echo "  - OpenSearch running with 5 test documents in entities_v1"
echo "  - Alias 'entities' pointing to entities_v1"
echo "  - Test search-indexer StatefulSet running"
echo ""
echo "Next steps:"
echo ""
echo "  Test full migration (v1 → v2):"
echo "     cd $REPO_ROOT/search-indexer-deploy/tests"
echo "     ./test-full-migration.sh 1 2"
echo ""
echo "  The test script will:"
echo "     - Rebuild and load the latest Docker image"
echo "     - Run the full migration job"
echo "     - Follow logs and show results"
echo ""
echo "  4. Delete the previous index after verification:"
echo "     kubectl apply -f ../k8s/jobs/delete-index-job.yaml"
echo "     # (after editing to set INDEX_VERSION=1 and CONFIRM_DELETE=true)"
echo ""
echo "When done, cleanup with:"
echo "  kubectl delete namespace search"
echo "  kind delete cluster --name search-test"
