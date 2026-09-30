# Deployment Runbook

## Overview

**One trunk: `main`.** There is no `dev`, no `staging` branch, and no auto-deploy.

```
feature branch → PR → main → (manual) build image → (manual) deploy
```

`main` is protected: changes land by pull request. It was called `staging` until 2026-09-17,
when `main` and `dev` — 147 commits behind and untouched since July — were deleted and `staging`
was renamed. The name had come to mean two contradictory things: the branch serving production,
and a tier of k8s environments (`api-staging`, `knowledge-staging`, …) that no longer exist.

**Merging deploys NOTHING.** Every per-service auto-deploy workflow was deleted in #960/#961:
they applied manifests into pre-v2 namespaces (`api`, `knowledge`, `scoring`, `notifications`,
`search`, `kafka`) that no longer exist, so they reported green and changed nothing.

Shipping is two manual dispatches:

```sh
# 1. build — the tag is the 8-char short SHA of the commit on main
gh workflow run build-v2-images.yml --ref main -f service=<svc>

# 2. deploy — cluster geo-testnet-k8s is namespace `gaia`, the live stack
gh workflow run deploy-v2.yml --ref main \
  -f service=<svc> -f tag=<8-char-sha> -f cluster=geo-testnet-k8s
```

Verify by the **running image**, never by the workflow's conclusion:

```sh
kubectl --context=do-nyc2-geo-testnet-k8s -n gaia get deploy <svc> \
  -o jsonpath='{.spec.template.spec.containers[0].image}'
```

Database migrations apply through the api's `migrate` initContainer, so they land when the api
deploys — an image rollback does **not** undo a migration.


## Semantic search (embedding-service, embedding-indexer, slots)

Design and measurements: `docs/tech-designs/semantic-search.md`. Everything lives in namespace
`gaia` and ships with the same two dispatches as any other service.

| Part | Kind | Manifests | Talks to |
|---|---|---|---|
| `embedding-service` | Deployment + HPA (2–6 pods) | `embedding-service/k8s/v2/` | nothing; serves `POST /embed`, `GET /info` |
| `embedding-indexer` | Deployment, 1 pod **per slot** | `embedding-indexer/k8s/v2/` | OpenSearch, embedding-service |
| slot jobs | `search-admin` Jobs | `search-indexer-deploy/k8s/v2/jobs/*-slot*-job.yaml`, `ensure-search-pipeline-job.yaml`, `eval-slot-job.yaml` | OpenSearch, embedding-service |
| api | `EMBEDDING_SERVICE_URL` in `api/k8s/v2/api.yaml` | — | embedding-service |

A **slot** is a model pinned by the hash of its bundle descriptor
(`embedding-service/bundles/<name>/bundle.json`); the id is derived, never typed
(`embedding-service bundle slot <bundle.json>` prints it). The index `_meta`, the service's `/info`
and every semantic response name the slot, and the api uses a slot only when index and service
agree on its descriptor. If the service is down, `mode=semantic|hybrid` answers
`503 SEMANTIC_SEARCH_UNAVAILABLE` and lexical search is untouched.

### First bring-up (once per cluster)

```sh
# 1. the runtime
gh workflow run build-v2-images.yml --ref main -f service=embedding-service
gh workflow run deploy-v2.yml --ref main -f service=embedding-service -f tag=<sha> -f cluster=geo-testnet-k8s
kubectl -n gaia port-forward svc/embedding-service 8080 &  ; curl -s localhost:8080/info | jq '.slots | keys'

# 2. the slot on the index (needs index.knn — an index created before the semantic-search merge
#    lacks it: create the next version and full-migration first) and the hybrid pipeline
kubectl apply -f search-indexer-deploy/k8s/v2/jobs/add-embedding-slot-job.yaml
kubectl -n gaia logs -f job/opensearch-add-embedding-slot
kubectl apply -f search-indexer-deploy/k8s/v2/jobs/ensure-search-pipeline-job.yaml

# 3. the writer — backfills the scope (P1: the Claim type), then follows indexed_at
gh workflow run build-v2-images.yml --ref main -f service=embedding-indexer
gh workflow run deploy-v2.yml --ref main -f service=embedding-indexer -f tag=<sha> -f cluster=geo-testnet-k8s
kubectl apply -f search-indexer-deploy/k8s/v2/jobs/list-slots-job.yaml   # coverage n/m per slot

# 4. the reader — api.yaml already carries EMBEDDING_SERVICE_URL; deploy the api as usual, then
curl -s 'https://<api>/search?q=voter+id+laws&mode=semantic&limit=3' | jq '{embeddingSlot, r: [.results[].name]}'

# 5. the gate
kubectl apply -f search-indexer-deploy/k8s/v2/jobs/eval-slot-job.yaml   # recall@10 ≥ EVAL_MIN_RECALL or the job fails
```

The search-admin jobs run `search-admin:latest` (built from `main` by `search-admin-build.yml`).
Before the semantic-search merge that image has no slot commands: dispatch that workflow on the
feature branch and pin the commit-sha tag in the job. Always check the running image carries the
command — the CLI's own usage error in the job log is the symptom when it does not.

**After a large backfill, force-merge once.** A fresh backfill leaves the k-NN graph across many
segments and a semantic query then scans them all: measured 440 ms p50 → 22 ms after
`POST /<index>/_forcemerge?max_num_segments=1` (through a port-forward to OpenSearch, with the
credentials from the `opensearch-credentials` secret). The merge itself took ~30 min on 321k
documents; the `curl` may time out while the merge completes server-side.

A backfill is embedding-bound: ~300 texts/s per service pod on 2 CPUs. The HPA adds pods as the
indexer saturates one; raise `maxReplicas` in `embedding-service/k8s/v2/hpa.yaml` for a large
scope rather than waiting.

### Model rotation (design D9)

No step reindexes or overwrites a vector, and at every step every response says which slot
answered.

1. Add bundle B under `embedding-service/bundles/`, build and deploy the service (both slots
   loaded; `/info` lists both).
2. `add-embedding-slot-job.yaml` with `EMBEDDING_SLOT=<B>` (A keeps serving).
3. Copy `embedding-indexer.yaml` to a second Deployment (`embedding-indexer-<B>`,
   `EMBEDDING_SLOT=<B>`); it backfills B in parallel with A's follow loop.
4. `eval-slot-job.yaml` for A and for B; compare recall, MRR and the contrastive rows.
5. `set-default-slot-job.yaml` with `EMBEDDING_SLOT=<B>`. Callers that pin `slot=A` keep working.
6. Grace period, then: delete A's indexer Deployment, `retire-embedding-slot-job.yaml` for A,
   drop A's bundle from the image. A's fields stay until the next index version.

### Failure modes

| Symptom | Where to look | Usual cause |
|---|---|---|
| semantic/hybrid 503, lexical fine | api logs "embedding slots refresh failed"; `list-slots` job | service down, or `HASH MISMATCH` / `NOT LOADED`: image and index disagree on the slot |
| coverage stops climbing | indexer logs (cycle stats), `/health/ready` | service unreachable (indexer backs off, no data loss), or OpenSearch write block (`read_only_allow_delete`: disk) |
| hybrid 503 but semantic works | `ensure-search-pipeline` job | pipeline missing for this alias |
| slow semantic queries after a backfill | `_cat/segments` | not force-merged (see above) |
| indexer CrashLoop at start | its first log lines | slot not on the index, or descriptor hash differs from the service's |

### Locally

`docker compose --profile infra --profile semantic up -d --build` starts both services next to
the compose OpenSearch (the first build compiles the ONNX runtime and fetches the model). The
header comment of that block in `docker-compose.yml` has the slot-registration step; on Apple
M4 see `docs/gotchas.md` for the JVM flag OpenSearch needs.

---

> **Everything below describes the retired GitFlow** (`dev` → staging, `main` → production,
> auto-deploy on push). It is kept for historical context; none of those branches or workflows
> exist. Treat it as a record of how things used to work, not as instructions.

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
