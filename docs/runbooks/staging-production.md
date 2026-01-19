# Staging & Production Deployment Runbook

## Overview

We use GitFlow for deployments:

```
feature branches → dev → main
                    ↓      ↓
                staging   production
```

- **Push to `dev`**: Auto-deploys changed services to staging
- **Merge `dev` → `main`**: Auto-deploys changed services to production

## Service & Namespace Mapping

| Service | Production NS | Staging NS | Workflow Files |
|---------|--------------|------------|----------------|
| api | `api` | `api-staging` | `api-deploy.yml`, `api-deploy-staging.yml` |
| kg-indexer | `knowledge` | `knowledge-staging` | `kg-indexer-deploy.yml`, `kg-indexer-deploy-staging.yml` |
| hermes-pipeline | `knowledge` | `knowledge-staging` | `hermes-pipeline-deploy.yml`, `hermes-pipeline-deploy-staging.yml` |
| hermes-ipfs-cache | `knowledge` | `knowledge-staging` | `hermes-ipfs-cache-deploy.yml`, `hermes-ipfs-cache-deploy-staging.yml` |
| search-indexer | `search` | `search-staging` | `search-indexer-deploy.yml`, `search-indexer-deploy-staging.yml` |
| scoring-cronjob | `scoring` | `scoring-staging` | `scoring-cronjob-deploy.yml`, `scoring-cronjob-deploy-staging.yml` |
| vote-indexer | `scoring` | `scoring-staging` | `scoring-vote-indexer-deploy.yml`, `scoring-vote-indexer-deploy-staging.yml` |
| atlas | `kafka` | `kafka-staging` | `atlas-deploy.yml`, `atlas-deploy-staging.yml` |
| kafka-ui | `kafka` | `kafka-staging` | `kafka-ui-deploy.yml`, `kafka-ui-deploy-staging.yml` |

## K8s Manifest Structure

Each service has manifests organized by environment:

```
<service>/k8s/
├── staging/
│   ├── namespace.yaml
│   └── <service>.yaml
└── production/
    ├── namespace.yaml
    └── <service>.yaml
```

Exception: `scoring-service` uses `deployment/` instead of `k8s/`:
```
scoring-service/deployment/
├── staging/
└── production/
```

## Common Operations

### Deploy to Staging

Push to the `dev` branch. The workflow triggers automatically for changed paths.

```bash
git checkout dev
git merge feature/my-feature
git push origin dev
```

Monitor: [GitHub Actions](https://github.com/geo-web-project/gaia/actions)

### Promote to Production

Merge `dev` into `main`:

```bash
git checkout main
git merge dev
git push origin main
```

Or create a PR from `dev` → `main` for review.

### Check What's Deployed

**Via kubectl:**
```bash
# Production
kubectl get deployment <name> -n <namespace> -o jsonpath='{.spec.template.spec.containers[0].image}'

# Staging
kubectl get deployment <name> -n <namespace>-staging -o jsonpath='{.spec.template.spec.containers[0].image}'
```

**Examples:**
```bash
# API production
kubectl get deployment api -n api -o jsonpath='{.spec.template.spec.containers[0].image}'

# KG Indexer staging
kubectl get deployment kg-indexer -n knowledge-staging -o jsonpath='{.spec.template.spec.containers[0].image}'
```

**Via GitHub Actions:**
Check the most recent workflow run for the service.

### View Logs

```bash
# Production
kubectl logs -f deployment/<name> -n <namespace>

# Staging
kubectl logs -f deployment/<name> -n <namespace>-staging
```

**Examples:**
```bash
# API production logs
kubectl logs -f deployment/api -n api

# KG Indexer staging logs
kubectl logs -f deployment/kg-indexer -n knowledge-staging

# Hermes pipeline (job, not deployment)
kubectl logs -f job/hermes-pipeline -n knowledge
kubectl logs -f job/hermes-pipeline -n knowledge-staging
```

### Rollback

**Option 1: Revert commit and push**
```bash
git checkout main  # or dev for staging
git revert <bad-commit>
git push
```

**Option 2: Manually set image to previous SHA**
```bash
kubectl set image deployment/<name> <container>=registry.digitalocean.com/geo/<image>:<previous-sha> -n <namespace>
```

### Restart a Deployment

```bash
kubectl rollout restart deployment/<name> -n <namespace>
```

### Scale a Deployment

```bash
# Scale down (e.g., for maintenance)
kubectl scale deployment/<name> -n <namespace> --replicas=0

# Scale up
kubectl scale deployment/<name> -n <namespace> --replicas=2
```

## Hotfix Workflow

For urgent production fixes when staging has untested changes:

1. **Create hotfix branch from main:**
   ```bash
   git checkout main
   git pull
   git checkout -b hotfix/critical-fix
   ```

2. **Make the fix, push, and merge to main:**
   ```bash
   # ... make changes ...
   git commit -m "fix: critical issue"
   git push -u origin hotfix/critical-fix
   # Create PR to main, get review, merge
   ```

3. **Backport to dev:**
   ```bash
   git checkout dev
   git merge main  # Brings the hotfix into dev
   git push
   ```

## Environment Differences

| Aspect | Staging | Production |
|--------|---------|------------|
| Namespace suffix | `-staging` | (none) |
| Image tag | `:staging` or `:sha` | `:latest` or `:sha` |
| Replicas | Usually 1 | 2+ |
| Resources | Lower limits | Full limits |
| Database | Staging DB | Production DB |
| Kafka consumer groups | `*-staging` | Standard names |

## Debugging

### Deployment Not Starting

```bash
# Check events
kubectl get events -n <namespace> --sort-by='.lastTimestamp' | tail -20

# Describe deployment
kubectl describe deployment/<name> -n <namespace>

# Check pod status
kubectl get pods -n <namespace> -l app=<name>
kubectl describe pod <pod-name> -n <namespace>
```

### Pod CrashLooping

```bash
# Get logs from crashed pod
kubectl logs <pod-name> -n <namespace> --previous

# Check resource limits
kubectl describe pod <pod-name> -n <namespace> | grep -A5 "Limits:"
```

### Image Pull Errors

```bash
# Verify image exists in registry
doctl registry login
docker manifest inspect registry.digitalocean.com/geo/<image>:<tag>
```

### Workflow Not Triggering

Check that your changes match the path filter in the workflow file:
- Workflows only trigger when files in specified paths change
- The workflow file itself is also in the path filter

## Service-Specific Notes

### hermes-pipeline, atlas

These are **Jobs**, not Deployments. Jobs are immutable, so the workflow deletes the existing job before creating a new one.

```bash
# Check job status
kubectl get jobs -n knowledge
kubectl get jobs -n kafka

# View job logs
kubectl logs job/hermes-pipeline -n knowledge
kubectl logs job/atlas -n kafka
```

### kafka-ui

Deploys a ConfigMap with protobuf schemas in addition to the deployment. If proto files change, the ConfigMap is updated.

### scoring-cronjob

This is a CronJob, not a Deployment. Check scheduled runs:

```bash
kubectl get cronjob -n scoring
kubectl get jobs -n scoring --sort-by='.metadata.creationTimestamp' | tail -5
```

## Keeping Manifests in Sync

When updating k8s manifests, remember to update **both** staging and production if the change applies to both environments.

```bash
# Example: updating resource limits for kg-indexer
# Edit both files:
# - kg-indexer/k8s/staging/kg-indexer.yaml
# - kg-indexer/k8s/production/kg-indexer.yaml
```

Use diff to check for unintended drift:
```bash
diff <service>/k8s/staging/<file>.yaml <service>/k8s/production/<file>.yaml
```

## Quick Reference

| Task | Command |
|------|---------|
| Deploy to staging | `git push origin dev` |
| Promote to prod | `git checkout main && git merge dev && git push` |
| Check deployed image | `kubectl get deploy <name> -n <ns> -o jsonpath='{.spec.template.spec.containers[0].image}'` |
| View logs | `kubectl logs -f deploy/<name> -n <ns>` |
| Restart | `kubectl rollout restart deploy/<name> -n <ns>` |
| Rollback | `kubectl set image deploy/<name> <container>=<registry>/<image>:<old-sha> -n <ns>` |
| Check events | `kubectl get events -n <ns> --sort-by='.lastTimestamp' \| tail -20` |
