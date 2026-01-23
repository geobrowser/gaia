#!/bin/bash

# Unified Search Admin Script
# - Runs search-admin CLI commands via kubectl (using CI/CD-built image)
# - Orchestrates full deployment workflows (stop/start indexer, migrations)

set -e

NAMESPACE="search"
KUBECONFIG_FILE=""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

print_header() {
    echo ""
    echo -e "${BLUE}================================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}================================================${NC}"
    echo ""
}

print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠ $1${NC}"
}

print_error() {
    echo -e "${RED}❌ $1${NC}"
}

print_info() {
    echo -e "${BLUE}ℹ $1${NC}"
}

print_separator() {
    echo -e "${BLUE}────────────────────────────────────────────────${NC}"
}

# Confirmation prompt with detailed information
confirm_operation() {
    local operation_name="$1"
    shift
    local details=("$@")

    echo ""
    print_separator
    echo -e "${YELLOW}⚠  CONFIRMATION REQUIRED${NC}"
    print_separator
    echo ""
    echo -e "${BLUE}Operation:${NC} ${YELLOW}$operation_name${NC}"
    echo ""
    echo -e "${BLUE}Configuration:${NC}"

    # Show namespace and kubeconfig
    echo "  Namespace: ${NAMESPACE}"
    if [ -n "$KUBECONFIG_FILE" ]; then
        echo "  Kubeconfig: ${KUBECONFIG_FILE}"
    else
        echo "  Kubeconfig: (default kubectl config)"
    fi

    # Show operation-specific details
    if [ ${#details[@]} -gt 0 ]; then
        echo ""
        echo -e "${BLUE}Details:${NC}"
        for detail in "${details[@]}"; do
            echo "  $detail"
        done
    fi

    echo ""
    print_separator
    echo ""

    read -p "$(echo -e ${YELLOW}Do you want to proceed? Type \'yes\' to continue: ${NC})" confirm
    echo ""

    if [ "$confirm" != "yes" ]; then
        print_warning "Operation cancelled by user"
        exit 0
    fi

    print_success "Confirmed. Proceeding with operation..."
    echo ""
}

show_usage() {
    cat <<EOF
Search Admin - Unified Index Management Tool

This script provides both individual CLI commands and full deployment workflows.

Usage: $0 [--kubeconfig PATH] <command> [arguments]

Global Options:
    --kubeconfig PATH    Path to kubeconfig file (default: uses default kubectl config)

═══════════════════════════════════════════════════════════════
CLI Commands (runs via kubectl with CI/CD-built image)
═══════════════════════════════════════════════════════════════

    create-index --version <VERSION>
        Create a new versioned index

    reindex --source-version <SRC> --target-version <TGT> [--wait-for-completion]
        Reindex data from source to target version

    monitor-reindex --task-id <TASK_ID>
        Monitor an async reindex task

    delete-index --version <VERSION> --confirm [--yes]
        Delete an old index version

    list-indices [--detailed]
        List all indices and aliases

    update-alias --version <VERSION>
        Update the alias to point to a new index version

    help
        Show search-admin CLI help

═══════════════════════════════════════════════════════════════
Deployment Commands (kubectl orchestration)
═══════════════════════════════════════════════════════════════

    stop-indexer
        Scale down the search-indexer deployment to 0 replicas

    start-indexer [NEW_VERSION]
        Scale up the search-indexer deployment to 1 replica
        Optionally update ENTITIES_INDEX_VERSION first

    status
        Show current index and deployment status

    full-migration <SOURCE_VERSION> <TARGET_VERSION>
        Run complete migration workflow:
        1. Create new index
        2. Stop search-indexer
        3. Reindex data
        4. Start search-indexer with new version
        (Does NOT delete old index - do that manually after verification)

Examples:
    # List indices
    $0 list-indices

    # Create new index
    $0 create-index --version 3

    # Full migration from v2 to v3
    $0 full-migration 2 3

    # Reindex only (async)
    $0 reindex --source-version 2 --target-version 3

    # Stop indexer
    $0 stop-indexer

    # Start indexer with new version
    $0 start-indexer 3

    # Check status
    $0 status

    # Delete old index (after verification)
    $0 delete-index --version 2 --confirm --yes

Environment Variables:
    KUBECONFIG          Path to kubeconfig file (can also use --kubeconfig flag)
    RUST_LOG            Log level for CLI commands (default: info)

EOF
    exit 1
}

check_kubectl() {
    if ! command -v kubectl &> /dev/null; then
        print_error "kubectl is not installed or not in PATH"
        exit 1
    fi

    # If kubeconfig is specified, verify it exists
    if [ -n "$KUBECONFIG_FILE" ]; then
        if [ ! -f "$KUBECONFIG_FILE" ]; then
            print_error "Kubeconfig file not found: $KUBECONFIG_FILE"
            exit 1
        fi
        print_info "Using kubeconfig: $KUBECONFIG_FILE"
    fi
}

# Build kubectl command with optional kubeconfig
kubectl_cmd() {
    if [ -n "$KUBECONFIG_FILE" ]; then
        kubectl --kubeconfig="$KUBECONFIG_FILE" "$@"
    else
        kubectl "$@"
    fi
}

get_opensearch_url() {
    # Get from secret
    OPENSEARCH_URL=$(kubectl_cmd get secret opensearch-credentials -n "$NAMESPACE" -o jsonpath='{.data.OPENSEARCH_URL}' 2>/dev/null | base64 -d 2>/dev/null || echo "")

    if [ -z "$OPENSEARCH_URL" ]; then
        print_error "Could not retrieve OPENSEARCH_URL from secret"
        print_info "Please ensure opensearch-credentials secret exists"
        exit 1
    fi
}

# Run a search-admin CLI command via kubectl
run_cli_command() {
    local cmd="$@"
    local first_arg="$1"

    # Build confirmation details based on command
    local details=()
    details+=("Command: search-admin $cmd")

    case "$first_arg" in
        create-index)
            local version=""
            # Parse version from args
            for i in "${!@}"; do
                if [ "${!i}" = "--version" ]; then
                    local next=$((i+1))
                    version="${!next}"
                    break
                fi
            done
            details+=("Index to create: entities_v${version}")
            confirm_operation "CREATE INDEX" "${details[@]}"
            ;;
        reindex)
            local source="" target="" mode="asynchronous"
            # Parse versions from args
            for i in "${!@}"; do
                if [ "${!i}" = "--source-version" ]; then
                    local next=$((i+1))
                    source="${!next}"
                fi
                if [ "${!i}" = "--target-version" ]; then
                    local next=$((i+1))
                    target="${!next}"
                fi
                if [ "${!i}" = "--wait-for-completion" ]; then
                    mode="synchronous"
                fi
            done
            details+=("Source: entities_v${source}")
            details+=("Target: entities_v${target}")
            details+=("Mode: ${mode}")
            confirm_operation "REINDEX DATA" "${details[@]}"
            ;;
        delete-index)
            local version=""
            # Parse version from args
            for i in "${!@}"; do
                if [ "${!i}" = "--version" ]; then
                    local next=$((i+1))
                    version="${!next}"
                    break
                fi
            done
            details+=("Index to DELETE: entities_v${version}")
            details+=("⚠️  THIS IS IRREVERSIBLE!")
            confirm_operation "DELETE INDEX" "${details[@]}"
            ;;
        monitor-reindex)
            # Monitor is read-only, but still show confirmation
            details+=("(Read-only operation)")
            confirm_operation "MONITOR REINDEX TASK" "${details[@]}"
            ;;
        list-indices)
            # Read-only operation - skip confirmation
            print_info "Running read-only operation: list-indices"
            ;;
        update-alias)
            local version=""
            # Parse version from args
            for i in "${!@}"; do
                if [ "${!i}" = "--version" ]; then
                    local next=$((i+1))
                    version="${!next}"
                    break
                fi
            done
            details+=("Alias: entities")
            details+=("New Target Index: entities_v${version}")
            details+=("⚠️  This will switch the active index!")
            confirm_operation "UPDATE ALIAS" "${details[@]}"
            ;;
        *)
            # Unknown command - still confirm
            confirm_operation "RUN CLI COMMAND" "${details[@]}"
            ;;
    esac

    print_header "Executing: search-admin $cmd"

    get_opensearch_url

    # Generate a unique pod name
    local pod_name="search-admin-$(date +%s)-$$"

    # Run the command using kubectl run
    kubectl_cmd run "$pod_name" \
        --image="registry.digitalocean.com/geo/search-admin:latest" \
        --restart=Never \
        --rm \
        -i \
        --quiet \
        --env="OPENSEARCH_URL=$OPENSEARCH_URL" \
        --env="INDEX_ALIAS=entities" \
        --env="RUST_LOG=${RUST_LOG:-info}" \
        -n "$NAMESPACE" \
        -- $cmd

    local exit_code=$?

    echo ""
    if [ $exit_code -eq 0 ]; then
        print_success "Command completed successfully"
    else
        print_error "Command failed with exit code $exit_code"
        exit $exit_code
    fi
}

# ═══════════════════════════════════════════════════════════════
# Deployment Orchestration Functions
# ═══════════════════════════════════════════════════════════════

stop_indexer() {
    local details=(
        "Deployment: search-indexer"
        "Action: Scale to 0 replicas"
        "⚠️  This will stop all entity indexing!"
    )

    confirm_operation "STOP SEARCH INDEXER" "${details[@]}"

    print_header "Stopping Search Indexer"

    print_info "Scaling search-indexer deployment to 0 replicas..."
    kubectl_cmd scale deployment/search-indexer --replicas=0 -n "$NAMESPACE"

    print_info "Waiting for pods to terminate..."
    kubectl_cmd wait --for=delete pod -l app=search-indexer -n "$NAMESPACE" --timeout=120s || true

    print_success "Search indexer stopped"
}

start_indexer() {
    local new_version=$1

    local details=(
        "Deployment: search-indexer"
        "Action: Scale to 1 replica"
    )

    if [ -n "$new_version" ]; then
        details+=("ENTITIES_INDEX_VERSION: ${new_version}")
        details+=("Index: entities_v${new_version}")
    else
        details+=("ENTITIES_INDEX_VERSION: (no change)")
    fi

    confirm_operation "START SEARCH INDEXER" "${details[@]}"

    print_header "Starting Search Indexer"

    if [ -n "$new_version" ]; then
        print_info "Updating ENTITIES_INDEX_VERSION to ${new_version}..."
        kubectl_cmd set env deployment/search-indexer ENTITIES_INDEX_VERSION="${new_version}" -n "$NAMESPACE"
        print_success "Updated ENTITIES_INDEX_VERSION to ${new_version}"
    fi

    print_info "Scaling search-indexer deployment to 1 replica..."
    kubectl_cmd scale deployment/search-indexer --replicas=1 -n "$NAMESPACE"

    print_info "Waiting for pod to be ready..."
    kubectl_cmd wait --for=condition=ready pod -l app=search-indexer -n "$NAMESPACE" --timeout=120s

    print_success "Search indexer started"

    # Show recent logs
    print_info "Recent logs:"
    kubectl_cmd logs -n "$NAMESPACE" -l app=search-indexer --tail=20
}

show_status() {
    # Status is read-only, just show info without confirmation
    print_info "Fetching status (read-only operation)..."

    print_header "Search Infrastructure Status"

    print_info "Search Indexer Deployment:"
    kubectl_cmd get deployment search-indexer -n "$NAMESPACE" || print_error "Deployment not found"
    echo ""

    print_info "Current ENTITIES_INDEX_VERSION:"
    kubectl_cmd get deployment search-indexer -n "$NAMESPACE" -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="ENTITIES_INDEX_VERSION")].value}' 2>/dev/null || echo "Not set"
    echo ""
    echo ""

    print_info "Active Pods:"
    kubectl_cmd get pods -n "$NAMESPACE" -l app=search-indexer || echo "No pods found"
    echo ""

    print_info "Recent Jobs:"
    kubectl_cmd get jobs -n "$NAMESPACE" -l app=opensearch-admin --sort-by=.metadata.creationTimestamp 2>/dev/null | tail -5 || echo "No jobs found"
    echo ""

    # Run list-indices via CLI
    print_info "OpenSearch Indices:"
    get_opensearch_url
    local pod_name="search-admin-status-$(date +%s)"
    kubectl_cmd run "$pod_name" \
        --image="registry.digitalocean.com/geo/search-admin:latest" \
        --restart=Never \
        --rm \
        -i \
        --quiet \
        --env="OPENSEARCH_URL=$OPENSEARCH_URL" \
        --env="INDEX_ALIAS=entities" \
        --env="RUST_LOG=warn" \
        -n "$NAMESPACE" \
        -- list-indices 2>/dev/null || print_warning "Could not list indices"

    echo ""
}

full_migration() {
    local source_version=$1
    local target_version=$2

    if [ -z "$source_version" ] || [ -z "$target_version" ]; then
        print_error "Source and target versions are required"
        echo "Usage: $0 full-migration <source_version> <target_version>"
        exit 1
    fi

    # Detailed confirmation for full migration
    local details=(
        "Source Index: entities_v${source_version}"
        "Target Index: entities_v${target_version}"
        ""
        "This will execute 5 steps:"
        "  1. Create new index (entities_v${target_version})"
        "  2. Stop search-indexer deployment"
        "  3. Reindex all data (v${source_version} → v${target_version})"
        "  4. Update alias to point to new index"
        "  5. Start search-indexer with new version"
        ""
        "⚠️  Search indexing will be PAUSED during migration!"
        "⚠️  Old index (entities_v${source_version}) will NOT be auto-deleted"
    )

    confirm_operation "FULL INDEX MIGRATION" "${details[@]}"

    print_header "Full Index Migration: v${source_version} → v${target_version}"

    # Step 1: Create new index (confirmations are handled within each function)
    print_header "Step 1/4: Creating New Index"
    # Override confirmation since we already confirmed at the top level
    echo "  Creating entities_v${target_version}..."
    # Call the underlying kubectl command directly for sub-steps
    get_opensearch_url
    local pod_name="search-admin-create-$(date +%s)"
    kubectl_cmd run "$pod_name" \
        --image="registry.digitalocean.com/geo/search-admin:latest" \
        --restart=Never \
        --rm \
        -i \
        --quiet \
        --env="OPENSEARCH_URL=$OPENSEARCH_URL" \
        --env="INDEX_ALIAS=entities" \
        --env="RUST_LOG=${RUST_LOG:-info}" \
        -n "$NAMESPACE" \
        -- create-index --version "$target_version" --skip-if-exists
    print_success "Index created"
    echo ""

    # Step 2: Stop indexer
    print_header "Step 2/4: Stopping Search Indexer"
    echo "  Scaling down search-indexer..."
    kubectl_cmd scale deployment/search-indexer --replicas=0 -n "$NAMESPACE"
    kubectl_cmd wait --for=delete pod -l app=search-indexer -n "$NAMESPACE" --timeout=120s || true
    print_success "Search indexer stopped"
    echo ""

    # Step 3: Reindex
    print_header "Step 3/4: Reindexing Data"
    print_info "Choose reindex mode:"
    echo "  1. Synchronous (wait for completion - recommended for small indices)"
    echo "  2. Asynchronous (background task - recommended for large indices)"
    echo ""
    read -p "Enter choice (1 or 2): " reindex_choice

    echo "  Starting reindex..."
    local pod_name="search-admin-reindex-$(date +%s)"
    if [ "$reindex_choice" = "1" ]; then
        kubectl_cmd run "$pod_name" \
            --image="registry.digitalocean.com/geo/search-admin:latest" \
            --restart=Never \
            --rm \
            -i \
            --quiet \
            --env="OPENSEARCH_URL=$OPENSEARCH_URL" \
            --env="INDEX_ALIAS=entities" \
            --env="RUST_LOG=${RUST_LOG:-info}" \
            -n "$NAMESPACE" \
            -- reindex \
                --source-version "$source_version" \
                --target-version "$target_version" \
                --wait-for-completion
    else
        kubectl_cmd run "$pod_name" \
            --image="registry.digitalocean.com/geo/search-admin:latest" \
            --restart=Never \
            --rm \
            -i \
            --quiet \
            --env="OPENSEARCH_URL=$OPENSEARCH_URL" \
            --env="INDEX_ALIAS=entities" \
            --env="RUST_LOG=${RUST_LOG:-info}" \
            -n "$NAMESPACE" \
            -- reindex \
                --source-version "$source_version" \
                --target-version "$target_version"

        echo ""
        print_warning "Reindex is running in the background!"
        print_warning "You can monitor it with:"
        echo "  $0 monitor-reindex --task-id <TASK_ID>"
        echo ""
        read -p "Press Enter when reindex is complete to continue..."
    fi
    print_success "Reindex complete"
    echo ""

    # Step 4: Update alias
    print_header "Step 4/5: Updating Alias"
    echo "  Updating entities alias to point to entities_v${target_version}..."
    local pod_name="search-admin-alias-$(date +%s)"
    kubectl_cmd run "$pod_name" \
        --image="registry.digitalocean.com/geo/search-admin:latest" \
        --restart=Never \
        --rm \
        -i \
        --quiet \
        --env="OPENSEARCH_URL=$OPENSEARCH_URL" \
        --env="INDEX_ALIAS=entities" \
        --env="RUST_LOG=${RUST_LOG:-info}" \
        -n "$NAMESPACE" \
        -- update-alias --version "$target_version"
    print_success "Alias updated to entities_v${target_version}"
    echo ""

    # Step 5: Start indexer with new version
    print_header "Step 5/5: Starting Search Indexer"
    echo "  Updating version to ${target_version}..."
    kubectl_cmd set env deployment/search-indexer ENTITIES_INDEX_VERSION="${target_version}" -n "$NAMESPACE"
    echo "  Scaling up search-indexer..."
    kubectl_cmd scale deployment/search-indexer --replicas=1 -n "$NAMESPACE"
    kubectl_cmd wait --for=condition=ready pod -l app=search-indexer -n "$NAMESPACE" --timeout=120s
    print_success "Search indexer started with version ${target_version}"
    echo ""

    print_header "Migration Complete!"
    print_success "Search indexer is now using entities_v${target_version}"
    echo ""
    print_info "Next steps:"
    echo "  1. Monitor the search-indexer logs for any issues:"
    echo "     kubectl logs -n search -l app=search-indexer -f"
    echo ""
    echo "  2. Verify search functionality in your application"
    echo ""
    echo "  3. After a few days of stable operation, delete the old index:"
    echo "     $0 delete-index --version ${source_version} --confirm --yes"
    echo ""
}

# ═══════════════════════════════════════════════════════════════
# Main Script Logic
# ═══════════════════════════════════════════════════════════════

# Parse global flags first
while [[ $# -gt 0 ]]; do
    case $1 in
        --kubeconfig)
            KUBECONFIG_FILE="$2"
            shift 2
            ;;
        --help|-h)
            show_usage
            ;;
        *)
            # Not a global flag, break to process commands
            break
            ;;
    esac
done

# If KUBECONFIG env var is set and KUBECONFIG_FILE is not, use env var
if [ -z "$KUBECONFIG_FILE" ] && [ -n "$KUBECONFIG" ]; then
    KUBECONFIG_FILE="$KUBECONFIG"
fi

check_kubectl

if [ $# -eq 0 ]; then
    show_usage
fi

case "${1}" in
    # Deployment orchestration commands
    stop-indexer)
        stop_indexer
        ;;
    start-indexer)
        start_indexer "$2"
        ;;
    status)
        show_status
        ;;
    full-migration)
        full_migration "$2" "$3"
        ;;

    # CLI commands (passthrough to kubectl)
    create-index|reindex|monitor-reindex|delete-index|list-indices|update-alias|help)
        run_cli_command "$@"
        ;;

    *)
        print_error "Unknown command: $1"
        echo ""
        show_usage
        ;;
esac
