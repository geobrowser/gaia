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

## Merge Strategy

| Target | Method | Why |
|--------|--------|-----|
| `dev` | **Squash merge only** | Clean single commit per feature |
| `main` | **Regular merge** from `dev` | Preserves commit SHAs for rebasing |
| `main` | Squash merge for hotfixes | Hotfixes bypass dev |

**Why this matters:** Squash merging destroys commit identity. If we squash dev→main, feature branches can't cleanly rebase onto dev because Git doesn't recognize "already applied" commits. Regular merge preserves SHAs so rebasing works.

## Drizzle Migrations Across Branches

Drizzle numbers migrations sequentially (`0062_*`, `0063_*`) with one entry per migration in `api/drizzle/meta/_journal.json`. The migrate step runs automatically as an init container on every deploy and applies migrations by a **timestamp high-water mark**: it runs every journal entry whose `when` is newer than the latest `created_at` already in that database's `drizzle.__drizzle_migrations` table.

Because `dev` and `main` are long-lived branches that each auto-migrate their own database, **two migrations must never be generated at the same index on `dev` and `main` in parallel.** If they are, both claim e.g. `0062`, each gets applied to its own environment, and the branches can no longer be merged cleanly — duplicate `0062_*` files, a conflicting `_journal.json`, and a broken snapshot chain, with no trivial resolution.

### Avoid it

- **Land schema changes `dev` → `main`** (the normal flow) so each migration is created once and flows in order.
- **If a migration reaches `main` directly** (a hotfix or release that bypasses dev), **backport `main` → `dev` immediately** — before any new migration is generated on either branch. The longer both branches sit un-synced, the more likely the other branch generates its own migration at the same index. See [Hotfix Workflow](#hotfix-workflow) and [Reset dev After Release](#reset-dev-after-release).

### Fix it (once the conflict exists)

Worked example: [PR #754](https://github.com/geobrowser/gaia/pull/754).

1. **Keep both migrations; renumber the later-merged one** so each index is unique (e.g. `dev`'s stays `0062`, `main`'s becomes `0063`). Which keeps which number is cosmetic — the steps below are what make it correct.
2. **Regenerate, don't hand-rename.** Place the first migration + its snapshot, then run `bun run db:generate` for the second so its `meta/00NN_snapshot.json` stacks on the first's. A hand-rename leaves the snapshot chain (and the next `db:generate` diff) broken.
3. **Make both migrations idempotent** — `CREATE TABLE/INDEX IF NOT EXISTS`, `ADD COLUMN IF NOT EXISTS`, `INSERT … ON CONFLICT DO NOTHING`. Each is already applied on one environment, so after the merge every environment **re-runs the one it already has**; idempotency makes that a safe no-op instead of an `already exists` error.
4. **Bump both `when` timestamps above the newest migration any environment has already applied** (keep them monotonic with the index). Migrate applies by high-water mark, so a migration with an older timestamp than what an environment already ran is **silently skipped** there — the tables/columns would never get created.
5. **Verify:** `bun run db:generate` reports no schema changes, and check each environment's high-water with `SELECT created_at FROM drizzle.__drizzle_migrations ORDER BY created_at DESC LIMIT 1;`.

Net effect: every environment converges — it creates the migration it was missing and no-ops the one it already had.

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

Create a PR from `dev` → `main` and use **regular merge** (not squash):

```bash
# Via GitHub CLI
gh pr create --base main --head dev --title "Release: promote dev to main"
# Then merge with regular merge (not squash) in the GitHub UI
```

Or via command line:
```bash
git checkout main
git pull origin main
git merge dev --no-ff
git push origin main
```

**Important:** Always use regular merge for dev→main to preserve commit identity. See [Merge Strategy](#merge-strategy).

### Reset dev After Release

After promoting dev→main, reset dev to match main:

```bash
git checkout dev
git fetch origin
git reset --hard origin/main
git push --force-with-lease origin dev
```

This keeps dev as a clean staging branch and prevents divergent history that causes rebase conflicts on feature branches.

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
| Promote to prod | `gh pr create --base main --head dev` then **regular merge** |
| Reset dev after release | `git checkout dev && git reset --hard origin/main && git push --force-with-lease` |
| Check deployed image | `kubectl get deploy <name> -n <ns> -o jsonpath='{.spec.template.spec.containers[0].image}'` |
| View logs | `kubectl logs -f deploy/<name> -n <ns>` |
| Restart | `kubectl rollout restart deploy/<name> -n <ns>` |
| Rollback | `kubectl set image deploy/<name> <container>=<registry>/<image>:<old-sha> -n <ns>` |
| Check events | `kubectl get events -n <ns> --sort-by='.lastTimestamp' \| tail -20` |
