# Index Change Management Guide

## Background

If we want to change the search index, changing field types or the number of primary shards, then a new index must be created and "reindexed" which is the process of transferring entities from the old index to the new. The process to reindex should avoid incorrect data writes to the new index and minimize indexing downtime.

> **💡 Note**
> Reindexing can be resource intensive for OpenSearch at large index sizes

### Reindex rough time estimates

Assuming ~8,000 documents per second, but this depends on hardware availability and usage.

| Number of documents (entities) | On the order of… | Estimated time |
| --- | --- | --- |
| 1K - 100K | ~ seconds | 13 sec |
| 1M - 10M | ~ minutes | 2 - 20 minutes |
| 100M - 800M | ~ hours | 3.5 hours |
| 1B + | ~ days | 1.4 days |

## Most simple process

1. Create the new index
2. Stop the search-indexer
3. Reindex data from the previous version to the new (Main reindex)
4. Update the alias to point to the new index
5. Start the search-indexer
6. Delete the previous index when assured with the new

## Least downtime process

We introduce a "catch-up" reindex so that the search-indexer is stopped for minimal time. The catch-up reindex only has to transfer changes made during the main reindex. For a large index, the main reindex could take on the order of minutes or hours, but the catch-up reindex should take much less time.

1. Create the new index
2. Reindex data from the previous version to the new (Main reindex)
3. Stop the search-indexer
4. Reindex data updated since start of main reindex (catch-up reindex)
5. Update the alias to point to the new index
6. Start the search-indexer
7. Delete the previous index when assured with the new

> **⚠️ Note**
> The catch-up reindex feature is not yet implemented in the current tooling. Use the simple process for now.

## Most simple process - Detailed

This guide walks through migrating from one index version to another using Kubernetes jobs. These jobs can be run from a user's machine with kubectl access or from CI/CD.

### Prerequisites

- `kubectl` configured with access to the cluster
- Appropriate permissions for the target namespace (`search` for production, `search-staging` for staging)
- The job YAML files from the appropriate environment directory:
  - **Production:** `search-indexer-deploy/k8s/production/jobs/`
  - **Staging:** `search-indexer-deploy/k8s/staging/jobs/`
  - **Testnet (gaia-v2):** `search-indexer-deploy/k8s/v2/jobs/`

### Step 0: Choose Your Environment

Before running any jobs, decide which environment you're migrating:

| Environment | Directory | Namespace | Index Prefix |
|-------------|-----------|-----------|--------------|
| Production | `k8s/production/jobs/` | `search` | (none) |
| Staging | `k8s/staging/jobs/` | `search-staging` | `staging_` |
| Testnet (gaia-v2) | `k8s/v2/jobs/` | `gaia-v2` | `testnet_` |

```bash
# For production
cd search-indexer-deploy/k8s/production/jobs

# For staging
cd search-indexer-deploy/k8s/staging/jobs

# For testnet (gaia-v2)
cd search-indexer-deploy/k8s/v2/jobs
```

All commands below assume you've navigated to the appropriate jobs directory.

### Full Automation (Recommended)

For a streamlined migration, use the `full-migration-job.yaml` which automates steps 1-5:

#### Step 1: Edit the job YAML to set versions

Edit `full-migration-job.yaml` and update these environment variables:

```yaml
- name: SOURCE_VERSION
  value: "2"  # Change to your source version
- name: TARGET_VERSION
  value: "3"  # Change to your target version
```

#### Step 2: Run the migration

**Note:** Replace `-n search` with `-n search-staging` if running in staging.

```bash
# Clean up any previous migration job
kubectl delete job opensearch-full-migration -n search 2>/dev/null || true

# Apply the job
kubectl apply -f full-migration-job.yaml

# Wait for the pod to be ready
kubectl wait --for=condition=ready pod -l job-name=opensearch-full-migration -n search --timeout=300s

# Follow the logs
kubectl logs -n search -f job/opensearch-full-migration
```

This job will:
1. Create the new index (`entities_v3`)
2. Stop the search-indexer
3. Reindex the data (v2 → v3)
4. Update the alias to point to v3
5. Start the search-indexer with version 3

Expected Output:

```
════════════════════════════════════════════════
Full Index Migration: v2 → v3
════════════════════════════════════════════════

Source Index: entities_v2
Target Index: entities_v3

Verifying source index exists...
✓ Source index exists

────────────────────────────────────────────────
Step 1/5: Creating New Index
────────────────────────────────────────────────
✓ Index entities_v3 created successfully

────────────────────────────────────────────────
Step 2/5: Stopping Search Indexer
────────────────────────────────────────────────
✓ Scaled down search-indexer to 0 replicas
  Waiting for pods to terminate...
✓ Search indexer stopped

────────────────────────────────────────────────
Step 3/5: Reindexing Data
────────────────────────────────────────────────
Source document count: 1000000

Task ID: abc123xyz:12345
⏳ Waiting for reindex to complete...
  Progress: 100.0% (1000000/1000000)
✓ Reindex completed successfully!
  Task ID: abc123xyz:12345

Reindex statistics:
  Total: 1000000
  Created: 1000000
  Updated: 0

Target document count: 1000000
✓ Document counts match!

────────────────────────────────────────────────
Step 4/5: Updating Alias
────────────────────────────────────────────────
✓ Alias 'entities' now points to entities_v3

────────────────────────────────────────────────
Step 5/5: Starting Search Indexer
────────────────────────────────────────────────
✓ Updated ENTITIES_INDEX_VERSION to 3
✓ Scaled up search-indexer to 1 replica
⏳ Waiting for pod to be ready...
✓ Search indexer started

════════════════════════════════════════════════
✓ Migration Complete!
════════════════════════════════════════════════
Search indexer is now using entities_v3

Next steps:
  1. Monitor the search-indexer logs for any issues:
     kubectl logs -n search -l app=search-indexer -f

  2. Verify search functionality in your application

  3. After a few days of stable operation, delete the old index:
     Edit delete-index-job.yaml (set INDEX_VERSION=2, CONFIRM_DELETE=true)
     kubectl delete job opensearch-delete-index -n search 2>/dev/null || true
     kubectl apply -f delete-index-job.yaml
     kubectl logs -n search -f job/opensearch-delete-index
```

#### Step 3: Verify the migration

```bash
# Check the deployment version
kubectl get deployment search-indexer -n search -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="ENTITIES_INDEX_VERSION")].value}'

# List indices and aliases
kubectl delete job opensearch-list-indices -n search 2>/dev/null || true
kubectl apply -f list-indices-job.yaml
kubectl logs -n search -f job/opensearch-list-indices
```

#### Step 4: Update deployment configuration and merge

Update the `ENTITIES_INDEX_VERSION` in the deployment YAML file so that future CI/CD deployments use the correct index version.

> **💡 Important**
> If you skip this step, the next CI/CD deployment will start the search-indexer with old index version (and it will fail to start)

1. Edit the appropriate deployment file:
   - **Production:** `search-indexer-deploy/k8s/production/search-indexer.yaml`
   - **Staging:** `search-indexer-deploy/k8s/staging/search-indexer.yaml`

```yaml
# Find the ENTITIES_INDEX_VERSION environment variable
- name: ENTITIES_INDEX_VERSION
  value: "3"  # Update from "2" to "3"
```

2. Commit and merge the change:

```bash
# For production
git add search-indexer-deploy/k8s/production/search-indexer.yaml
# For staging
git add search-indexer-deploy/k8s/staging/search-indexer.yaml

git commit -m "chore(search): update ENTITIES_INDEX_VERSION to 3"
git push
# Create PR and merge to main
```

3. Verify the change is merged before proceeding to delete the old index.

#### Step 5: Delete the old index (after 3-7 days)

After 3-7 days of stable operation, or when you are confident that the old index is not needed, delete it to free up storage space.

> **⚠️ WARNING**
> Be very careful here! Make sure you are deleting the **OLD** index version, not the current one.
> Double-check that `INDEX_VERSION` matches your **source** version from Step 1.

Edit `delete-index-job.yaml` and set:

```yaml
- name: INDEX_VERSION
  value: "2"  # IMPORTANT: This should be your OLD version (SOURCE_VERSION from Step 1)
- name: CONFIRM_DELETE
  value: "true"  # Required safety flag
```

Then run:

```bash
# Clean up any previous delete job
kubectl delete job opensearch-delete-index -n search 2>/dev/null || true

# Apply the delete job
kubectl apply -f delete-index-job.yaml

# Follow the logs
kubectl logs -n search -f job/opensearch-delete-index
```

Expected Output:

```
════════════════════════════════════════════════
Delete Index
════════════════════════════════════════════════
Index to Delete: entities_v2
⚠️  WARNING: This operation is IRREVERSIBLE!
Deleting index...
✓ Index deleted successfully
```

---

## Manual Step-by-Step Process

For more control, debugging, or for correcitons in the case of a failed migration, you can run each step individually using separate job files.

### Step 1: Create the New Index

Create the new versioned index with the updated mappings/settings.

Edit `create-index-job.yaml` to set `INDEX_VERSION`:

```yaml
- name: INDEX_VERSION
  value: "3"
```

Then run:

```bash
kubectl delete job opensearch-create-index -n search 2>/dev/null || true
kubectl apply -f create-index-job.yaml
kubectl logs -n search -f job/opensearch-create-index
```

Expected Output:

```
════════════════════════════════════════════════
Create Index
════════════════════════════════════════════════
Index Name: entities_v3
Version:    3
✓ Index created successfully
```

Verification:

```bash
kubectl delete job opensearch-list-indices -n search 2>/dev/null || true
kubectl apply -f list-indices-job.yaml
kubectl logs -n search -f job/opensearch-list-indices
```

You should see both `entities_v2` (old) and `entities_v3` (new) indices.

---

### Step 2: Stop the Search Indexer

Stop the `search-indexer` deployment to prevent new data from being written during migration.

```bash
kubectl scale deployment/search-indexer --replicas=0 -n search
kubectl wait --for=delete pod -l app=search-indexer -n search --timeout=120s
```

Expected Output:

```
deployment.apps/search-indexer scaled
```

Verification:

```bash
kubectl get deployment search-indexer -n search
```

You should see READY: 0/0.

---

### Step 3: Reindex the Data

Copy all documents from the old index to the new index.

Edit `reindex-job.yaml` to set source and target versions:

```yaml
- name: SOURCE_VERSION
  value: "2"
- name: TARGET_VERSION
  value: "3"
- name: WAIT_FOR_COMPLETION
  value: "true"  # Set to true for synchronous reindex
```

Then run:

```bash
kubectl delete job opensearch-reindex -n search 2>/dev/null || true
kubectl apply -f reindex-job.yaml
kubectl logs -n search -f job/opensearch-reindex
```

Expected Output:

```
════════════════════════════════════════════════
Reindex
════════════════════════════════════════════════
Source Index: entities_v2
Target Index:  entities_v3
Source document count: 1000000

Task ID: abc123xyz:12345
⏳ Waiting for reindex to complete...
  Progress: 45.2% (452000/1000000)
  Progress: 87.5% (875000/1000000)
✓ Reindex completed successfully!
  Task ID: abc123xyz:12345

Reindex statistics:
  Total: 1000000
  Created: 1000000
  Updated: 0

Target document count: 1000000
✓ Document counts match!
```

### Estimated Reindex Times:

| Document Count | Estimated Time (at 8,000 docs/sec) |
|----------------|-------------------------------------|
| 100K           | ~13 seconds                         |
| 1M             | ~2 minutes                          |
| 10M            | ~21 minutes                         |
| 100M           | ~3.5 hours                          |

---

### Step 4: Update the Alias

Update the alias to point to the new index version.

Edit `update-alias-job.yaml` to set the target version:

```yaml
- name: TARGET_VERSION
  value: "3"
```

Then run:

```bash
kubectl delete job opensearch-update-alias -n search 2>/dev/null || true
kubectl apply -f update-alias-job.yaml
kubectl logs -n search -f job/opensearch-update-alias
```

Expected Output:

```
════════════════════════════════════════════════
Update Index Alias
════════════════════════════════════════════════
Alias:        entities
New Index:    entities_v3
✓ Target index exists
Current alias mapping: entities -> entities_v2
Updating alias...
════════════════════════════════════════════════
✓ Alias Updated Successfully
════════════════════════════════════════════════
Alias 'entities' now points to 'entities_v3'
```

Verification:

```bash
kubectl delete job opensearch-list-indices -n search 2>/dev/null || true
kubectl apply -f list-indices-job.yaml
kubectl logs -n search -f job/opensearch-list-indices
```

You should see the alias now points to `entities_v3`.

---

### Step 5: Start the Search Indexer

Start the `search-indexer` deployment with the new index version.

```bash
# Update the index version
kubectl set env deployment/search-indexer ENTITIES_INDEX_VERSION=3 -n search

# Scale up to 1 replica
kubectl scale deployment/search-indexer --replicas=1 -n search

# Wait for pod to be ready
kubectl wait --for=condition=ready pod -l app=search-indexer -n search --timeout=120s
```

Expected Output:

```
deployment.apps/search-indexer env updated
deployment.apps/search-indexer scaled
pod/search-indexer-xxxx condition met
```

Verification:

```bash
kubectl get pods -n search -l app=search-indexer
```

You should see the pod in Running state with READY: 1/1.

---

### Step 6: Monitor and Verify

Monitor the `search-indexer` logs to ensure it's working correctly with the new index.

```bash
kubectl logs -n search -l app=search-indexer -f
```

Check for:

- Successful connection to OpenSearch
- Documents being indexed without errors
- No unexpected warnings or errors

Test search functionality:

- Test your application's search features to ensure they're working correctly with the new index.

---

### Step 7: Update deployment configuration and merge

Update the `ENTITIES_INDEX_VERSION` in the deployment YAML file so that future CI/CD deployments use the correct index version.

> **💡 Important**
> If you skip this step, the next CI/CD deployment will revert the search-indexer back to the old index version!

1. Edit the appropriate deployment file:
   - **Production:** `search-indexer-deploy/k8s/production/search-indexer.yaml`
   - **Staging:** `search-indexer-deploy/k8s/staging/search-indexer.yaml`

```yaml
# Find the ENTITIES_INDEX_VERSION environment variable
- name: ENTITIES_INDEX_VERSION
  value: "3"  # Update from "2" to "3"
```

2. Commit and merge the change:

```bash
# For production
git add search-indexer-deploy/k8s/production/search-indexer.yaml
# For staging
git add search-indexer-deploy/k8s/staging/search-indexer.yaml

git commit -m "chore(search): update ENTITIES_INDEX_VERSION to 3"
git push
# Create PR and merge to main
```

3. Verify the change is merged before proceeding to delete the old index.

---

### Step 8: Delete the Old Index

After a confidence period (recommended: 3-7 days), delete the old index to free up resources.

> **⚠️ WARNING**
> This operation is irreversible. Ensure the new index is working correctly before proceeding.

Edit `delete-index-job.yaml` and set:

```yaml
- name: INDEX_VERSION
  value: "2"  # IMPORTANT: This should be your OLD version
- name: CONFIRM_DELETE
  value: "true"  # Required safety flag
```

Then run:

```bash
kubectl delete job opensearch-delete-index -n search 2>/dev/null || true
kubectl apply -f delete-index-job.yaml
kubectl logs -n search -f job/opensearch-delete-index
```

Expected Output:

```
════════════════════════════════════════════════
Delete Index
════════════════════════════════════════════════
Index to Delete: entities_v2
⚠️  WARNING: This operation is IRREVERSIBLE!
Deleting index...
✓ Index deleted successfully
```

Verification:

```bash
kubectl delete job opensearch-list-indices -n search 2>/dev/null || true
kubectl apply -f list-indices-job.yaml
kubectl logs -n search -f job/opensearch-list-indices
```

You should only see `entities_v3` now.

---

## Troubleshooting

### Job Fails to Start

Check the job status:

```bash
kubectl get jobs -n search
kubectl describe job <job-name> -n search
```

Common issues:
- Image pull errors - verify the `search-admin` image exists in the registry
- Secret not found - ensure `opensearch-credentials` secret exists
- Permission errors - verify the ServiceAccount has proper RBAC permissions

### Reindex Takes Longer Than Expected

Monitor OpenSearch resource usage:

```bash
# Check OpenSearch pod status
kubectl get pods -n search -l app=opensearch

# Check OpenSearch logs
kubectl logs -n search -l app=opensearch --tail=100
```

Consider:
- OpenSearch may be under heavy load from other operations
- Hardware resources (CPU, memory, disk I/O) may be constrained
- Network bandwidth between nodes may be limited

### Document Counts Don't Match

If the source and target document counts differ:

1. Check for indexing errors in the reindex job logs
2. Verify no writes occurred to the source index during reindex (indexer should be stopped)
3. Check OpenSearch cluster health:

```bash
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -n search -- \
  curl "http://opensearch:9200/_cluster/health?pretty"
```

### Search Indexer Won't Start

Check deployment and pod status:

```bash
kubectl get deployment search-indexer -n search
kubectl get pods -n search -l app=search-indexer
kubectl describe pod <pod-name> -n search
kubectl logs -n search -l app=search-indexer --tail=100
```

Common issues:
- Image pull errors
- OpenSearch connectivity issues
- Invalid `ENTITIES_INDEX_VERSION` environment variable
- Missing required secrets or config

---

## Additional Resources

- **Job YAML Files**:
  - Production: `search-indexer-deploy/k8s/production/jobs/`
  - Staging: `search-indexer-deploy/k8s/staging/jobs/`
- **Jobs Documentation**: `search-indexer-deploy/k8s/jobs/README.md`
- **Search Admin Tool README**: `search-admin/README.md`
- **CI/CD Pipeline**: `.github/workflows/search-admin-build.yml`
- **Index Configuration**: `search-indexer-repository/src/opensearch/index_config.rs`
