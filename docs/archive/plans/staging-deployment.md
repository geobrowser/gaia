# Staging Deployment Strategy

## Status

Implemented (GitFlow)

## Date

2025-01-18 (proposed), 2025-01-19 (implemented)

## Context

We need a staging environment to test changes before deploying to production. Currently, all services deploy directly to production on merge to `main`.

### Current State

- **Services**: api, kg-indexer, hermes-pipeline, hermes-ipfs-cache, search-indexer, scoring-service, atlas, kafka-ui
- **Namespaces**: `api`, `knowledge`, `search`, `scoring`, `kafka`, `hermes-ipfs-cache`
- **Deploy trigger**: Push to `main` with path filters
- **Manifests**: Plain YAML, no templating (except search-indexer uses Kustomize)

### Goals

1. Test changes in staging before production
2. Simple promotion flow
3. Track what's deployed where
4. Support hotfixes without deploying all of staging
5. Allow independent service promotion when needed

## Deployment Model Options

There are two viable approaches, each with trade-offs:

### Option A: GitFlow (Branch-Based Promotion)

```
feature branches → dev → main
                    ↓      ↓
                staging   prod
```

| Event | Action |
|-------|--------|
| Push to `dev` | Auto-deploy changed services to staging |
| Merge `dev` → `main` | Auto-deploy changed services to production |

**Pros:**
- Simple mental model
- One merge = promotion (no manual clicks)
- Same number of workflows as today

**Cons:**
- Must keep `dev` synced with `main` after hotfixes
- Can't easily promote only some services (all of `dev` merges together)
- Cherry-picking is messy

### Option B: Trunk-Based with Manual Promotion

```
feature branches → main → staging (auto) → production (manual trigger)
```

| Event | Action |
|-------|--------|
| Push to `main` | Auto-deploy changed services to staging |
| Manual workflow dispatch | Promote specific service to production |

**Pros:**
- Per-service promotion control
- No branch sync issues
- Easy hotfixes (deploy specific SHA)

**Cons:**
- More workflows (2 per service)
- Must manually trigger each production deploy
- Need "promote all" workflow if deploying together

### Comparison

| Scenario | GitFlow | Trunk + Manual |
|----------|---------|----------------|
| Deploy all changed services | Easy (one merge) | Tedious (N clicks or meta-workflow) |
| Deploy only some services | Hard (must wait or cherry-pick) | Easy (trigger only what you want) |
| Hotfix one service | Branch from main, merge to both | Deploy specific SHA |
| Branch sync overhead | Yes (CI enforced) | None |
| Number of workflows | 9 (same as today) | 18+ (2 per service + optional meta) |

### Recommendation

**If services usually ship together**: GitFlow is simpler.

**If services have different cadences**: Trunk + manual gives more control.

Given our monorepo with independent services, **Trunk-Based with Manual Promotion** is recommended, with an optional "promote all" workflow for convenience.

## Decision

### Deployment Model: GitFlow (Branch-Based Promotion)

```
feature branches → dev → main
                    ↓      ↓
                staging   prod
```

After further consideration, GitFlow was chosen for simplicity:
- One merge = promotion (no manual workflow triggers)
- Services typically ship together in this monorepo
- Simpler mental model for the team

See `docs/runbooks/staging-production.md` for operational details.

### Namespace Strategy

Each service gets a staging namespace alongside its production namespace:

| Service | Production NS | Staging NS |
|---------|--------------|------------|
| api | `api` | `api-staging` |
| kg-indexer | `knowledge` | `knowledge-staging` |
| hermes-pipeline | `knowledge` | `knowledge-staging` |
| hermes-ipfs-cache | `hermes-ipfs-cache` | `hermes-ipfs-cache-staging` |
| search-indexer | `search` | `search-staging` |
| scoring-service | `scoring` | `scoring-staging` |

### Manifest Strategy: Simple Duplication

Given our small number of environments (2) and relatively simple manifests, we'll duplicate rather than use Kustomize overlays:

```
api/k8s/
├── staging/
│   ├── namespace.yaml
│   ├── api.yaml
│   └── secrets.yaml
└── production/
    ├── namespace.yaml
    ├── api.yaml
    └── secrets.yaml
```

**Rationale**: Kustomize adds complexity (patch syntax, mental overhead) for minimal benefit with only 2 environments. Duplication is explicit and easy to review.

### Key Differences: Staging vs Production

| Aspect | Staging | Production |
|--------|---------|------------|
| Namespace | `*-staging` | Current namespaces |
| Replicas | 1 | 2+ |
| Resources | Lower limits | Current limits |
| Ingress hosts | `staging-*.geobrowser.io` | `testnet-*.geobrowser.io` |
| Secrets | `*-staging-secrets` | Current secrets |
| Database | Staging DB | Production DB |
| Kafka consumer groups | `*-staging` suffix | Current groups |

### CI/CD Workflow Changes

#### Current (Production Only)
```yaml
# .github/workflows/api-deploy.yml
on:
  push:
    branches: [main]
    paths: ['api/**']
```

#### Implemented (GitFlow)

**Staging Deploy** (auto on push to dev):
```yaml
# .github/workflows/api-deploy-staging.yml
name: Deploy API (Staging)

on:
  push:
    branches: [dev]
    paths:
      - 'api/**'
      - '.github/workflows/api-deploy-staging.yml'

concurrency:
  group: api-staging
  cancel-in-progress: true
```

**Production Deploy** (auto on push to main):
```yaml
# .github/workflows/api-deploy.yml
name: Deploy API (Production)

on:
  push:
    branches: [main]
    paths:
      - 'api/**'
      - '.github/workflows/api-deploy.yml'

concurrency:
  group: api-production
  cancel-in-progress: true
```

Both workflows build and push images, then deploy to the appropriate k8s namespace.

### Deployment Tracking

Use GitHub Deployments API to track what's deployed where:

```bash
# Query via gh CLI
gh api repos/:owner/:repo/deployments --jq '.[] | select(.environment | startswith("api")) | {env: .environment, sha: .sha[0:7], created: .created_at}'
```

Also add labels to Kubernetes deployments:
```yaml
metadata:
  labels:
    app: api
    git-sha: "${GITHUB_SHA}"
```

Query deployed version:
```bash
kubectl get deployment api -n api -o jsonpath='{.metadata.labels.git-sha}'
```

### Hotfix Flow

For urgent production fixes that shouldn't include all staging changes:

1. Create hotfix branch from `main`
2. Make the fix
3. Merge to `main` (auto-deploys to production)
4. Merge `main` back into `dev` to keep branches synced

```bash
# Create hotfix branch from main
git checkout main
git pull
git checkout -b hotfix/critical-fix

# ... make fix, push, create PR to main ...

# After merge to main (auto-deploys to prod), sync dev:
git checkout dev
git merge main
git push
```

### Feature Flags (Future)

For risky changes, consider feature flags:
- Deploy code to production with flag disabled
- Enable in staging for testing
- Enable in production when ready
- Roll back by disabling flag (faster than redeploy)

Recommended tools: LaunchDarkly, Unleash, or simple env var flags.

## Implementation Steps

### Phase 1: Infrastructure Setup
- [x] Create staging namespaces in cluster
- [x] Create staging secrets (separate DB, Kafka consumer groups, etc.)
- [ ] Set up staging ingress hosts (DNS + TLS certs)

### Phase 2: Manifest Duplication (Done: c8f5a0f)
- [x] Create `k8s/staging/` and `k8s/production/` directories
- [x] Move existing manifests to `k8s/production/`
- [x] Copy and modify for `k8s/staging/` (namespace, resources, hosts, secrets)

### Phase 3: Workflow Updates (Done: c42d87a)
- [x] Update existing deploy workflows to use production paths
- [x] Create new `*-deploy-staging.yml` workflows triggering on `dev` branch
- [x] Add concurrency groups to prevent parallel deploys

### Phase 4: Testing
- [ ] Create `dev` branch
- [ ] Push a change to `dev`
- [ ] Verify staging auto-deploys
- [ ] Merge `dev` to `main`
- [ ] Verify production auto-deploys

## Services to Update

| Service | Staging Workflow | Production Workflow |
|---------|-----------------|---------------------|
| api | `api-deploy-staging.yml` | `api-deploy-production.yml` |
| kg-indexer | `kg-indexer-deploy-staging.yml` | `kg-indexer-deploy-production.yml` |
| hermes-pipeline | `hermes-pipeline-deploy-staging.yml` | `hermes-pipeline-deploy-production.yml` |
| hermes-ipfs-cache | `hermes-ipfs-cache-deploy-staging.yml` | `hermes-ipfs-cache-deploy-production.yml` |
| search-indexer | `search-indexer-deploy-staging.yml` | `search-indexer-deploy-production.yml` |
| scoring-service (cronjob) | `scoring-cronjob-deploy-staging.yml` | `scoring-cronjob-deploy-production.yml` |
| scoring-service (vote-indexer) | `scoring-vote-indexer-deploy-staging.yml` | `scoring-vote-indexer-deploy-production.yml` |
| atlas | `atlas-deploy-staging.yml` | `atlas-deploy-production.yml` |
| kafka-ui | `kafka-ui-deploy-staging.yml` | `kafka-ui-deploy-production.yml` |

## Consequences

### Positive
- Clear separation between staging and production
- Simple promotion: merge `dev` → `main`
- No manual workflow triggers needed
- Services ship together naturally (monorepo pattern)

### Negative
- Manifest duplication (must update both staging and production)
- More workflows to maintain (18 total, 2 per service)
- Staging infrastructure costs (additional pods, separate DB)
- Must keep `dev` synced with `main` after hotfixes

### Risks
- Staging and production manifests drift apart
- Staging DB/data may not reflect production issues
- `dev` and `main` branches diverging if not kept in sync

## Feature Flags

Regardless of deployment model, **feature flags are recommended** for managing risk:

### Why Feature Flags

- **Decouple deployment from release**: Code can be in production but inactive
- **Instant rollback**: Disable a flag vs. redeploy
- **Gradual rollout**: Enable for 10% of users, then 50%, then 100%
- **Test in production**: Enable only for internal users first
- **Reduce staging/prod drift**: Same code runs everywhere, behavior differs by flag

### When to Use Flags

- New features that might need quick rollback
- Risky changes to critical paths
- A/B testing or gradual rollouts
- Features that depend on external systems (can disable if system is down)

### Simple Implementation

For services without a flag system, use environment variables:

```rust
// Rust
let use_new_parser = std::env::var("FF_NEW_PARSER").unwrap_or_default() == "true";

if use_new_parser {
    new_parser::parse(input)
} else {
    old_parser::parse(input)
}
```

```typescript
// TypeScript
const useNewParser = process.env.FF_NEW_PARSER === 'true';

if (useNewParser) {
  newParser.parse(input);
} else {
  oldParser.parse(input);
}
```

Set differently per environment:
```yaml
# staging deployment
env:
  - name: FF_NEW_PARSER
    value: "true"

# production deployment
env:
  - name: FF_NEW_PARSER
    value: "false"
```

### Feature Flag Services (Future)

For more sophisticated needs:
- **LaunchDarkly**: Full-featured, paid
- **Unleash**: Open source, self-hosted
- **Flipt**: Open source, simple
- **ConfigCat**: Simple, free tier

## Future Considerations

1. **Kustomize migration**: If we add more environments (per-PR previews, regional), reconsider Kustomize
2. **ArgoCD**: Use for GitOps-style deploys and drift detection
3. **Automated promotion**: Auto-promote to production after N hours in staging with no errors
4. **Preview environments**: Per-PR ephemeral environments for testing
5. **Feature flag service**: Evaluate if env var flags become unwieldy

## Appendix: GitFlow Alternative

If the team later prefers GitFlow over trunk-based, here's the setup:

### Branch Structure

```
feature branches → dev → main
                    ↓      ↓
                staging   prod
```

### Workflow Changes

```yaml
# api-deploy-staging.yml
on:
  push:
    branches: [dev]  # Changed from main
    paths: ['api/**']

# api-deploy-production.yml
on:
  push:
    branches: [main]
    paths: ['api/**']
```

### Enforcing Branch Sync

CI check to fail if `dev` is behind `main`:

```yaml
# .github/workflows/dev-sync-check.yml
name: Check dev is synced with main

on:
  push:
    branches: [dev]
  pull_request:
    branches: [dev]

jobs:
  check-sync:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      
      - name: Check if dev contains all main commits
        run: |
          git fetch origin main
          BEHIND=$(git rev-list --count HEAD..origin/main)
          if [ "$BEHIND" -gt 0 ]; then
            echo "::error::dev is $BEHIND commits behind main. Merge main into dev first."
            git log --oneline HEAD..origin/main
            exit 1
          fi
          echo "dev is up to date with main"
```

### GitFlow Hotfix Process

1. Branch from `main`: `git checkout -b hotfix/fix-bug main`
2. Make fix, push, create PR to `main`
3. After merge to `main`, immediately merge `main` → `dev`
4. The CI check enforces this—PRs to `dev` will fail until synced

### GitFlow Selective Promotion Problem

The main downside: you can't easily promote only some services. Options:
- Wait until all services on `dev` are ready
- Cherry-pick specific commits (messy, error-prone)
- Use feature flags to disable unready code in production

This is why trunk-based with manual promotion is recommended for independent services.

## References

- Current deploy workflows: `.github/workflows/*-deploy.yml`
- Existing k8s manifests: `*/k8s/*.yaml`
- GitHub Deployments API: https://docs.github.com/en/rest/deployments
- GitHub Environments: https://docs.github.com/en/actions/deployment/targeting-different-environments
