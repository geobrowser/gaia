#!/bin/bash
set -e

# Parse arguments
KUBECONFIG_FLAG=""
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
      echo "Unknown option: $1"
      echo "Usage: $0 [--kubeconfig=<path>]"
      exit 1
      ;;
  esac
done

echo "==> Testing Kubernetes permissions and OpenSearch connectivity"
echo ""
if [ -n "$KUBECONFIG_FLAG" ]; then
  echo "Using kubeconfig: $KUBECONFIG_PATH"
else
  echo "Using default kubeconfig"
  echo "Current context: $(kubectl config current-context)"
fi
echo ""

# Clean up any previous test job
echo "==> Cleaning up previous test job (if exists)..."
kubectl $KUBECONFIG_FLAG delete job test-permissions -n search 2>/dev/null || true
echo ""

echo "==> Creating test job..."
kubectl $KUBECONFIG_FLAG apply -f test-permissions-job.yaml

echo ""
echo "==> Waiting for job to complete..."
echo ""

# Wait for pod to be created
until kubectl $KUBECONFIG_FLAG get pod -l job-name=test-permissions -n search 2>/dev/null | grep -q test-permissions; do
  sleep 2
done

# Follow the logs
kubectl $KUBECONFIG_FLAG logs -n search -f job/test-permissions

echo ""
echo "==> Test complete! Check the output above."
echo ""
echo "To clean up:"
if [ -n "$KUBECONFIG_FLAG" ]; then
  echo "  kubectl $KUBECONFIG_FLAG delete job test-permissions -n search"
else
  echo "  kubectl delete job test-permissions -n search"
fi
