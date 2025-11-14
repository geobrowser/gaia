# ArgoCD Setup for Hermes Visualization

ArgoCD is used for **visualization and monitoring** of the Hermes deployment. GitHub Actions handles the actual automated deployment.

## Why This Approach?

- **GitHub Actions** handles deployment (builds images, applies k8s manifests)
- **ArgoCD** provides a beautiful UI to visualize and monitor your services
- **No Git writes** - ArgoCD doesn't auto-sync, avoiding branch protection issues
- **Simple flow** - Merge PR → GitHub Actions deploys → View in ArgoCD UI

## Initial Setup

### 1. Install ArgoCD

```bash
# Create argocd namespace
kubectl create namespace argocd

# Install ArgoCD
kubectl apply -n argocd -f https://raw.githubusercontent.com/argoproj/argo-cd/stable/manifests/install.yaml

# Wait for ArgoCD to be ready
kubectl wait --for=condition=available --timeout=300s deployment/argocd-server -n argocd
```

### 2. Access ArgoCD UI

#### Option A: Port Forward (for testing)
```bash
kubectl port-forward svc/argocd-server -n argocd 8080:443
```
Then open https://localhost:8080

#### Option B: LoadBalancer (recommended for production)
```bash
kubectl patch svc argocd-server -n argocd -p '{"spec": {"type": "LoadBalancer"}}'
kubectl get svc argocd-server -n argocd
```

### 3. Get Initial Admin Password

```bash
kubectl -n argocd get secret argocd-initial-admin-secret -o jsonpath="{.data.password}" | base64 -d; echo
```

Login with:
- Username: `admin`
- Password: (output from above command)

**IMPORTANT**: Change the password after first login!

### 4. Deploy the Hermes Application

```bash
kubectl apply -f hermes/argocd/hermes-application.yaml
```

This creates an ArgoCD Application that:
- Watches the `main` branch of your repository
- Monitors `hermes/overlays/digitalocean/`
- **Does NOT auto-sync** - used for visualization only
- Shows the status of all your Hermes resources

## How It Works

### Deployment Flow

1. You merge a PR to `main`
2. **GitHub Actions** automatically:
   - Builds Docker image with `:latest` tag
   - Pushes to DigitalOcean registry
   - Applies k8s manifests using `kubectl apply`
   - Restarts the hermes-producer Job
3. **ArgoCD UI** shows the updated deployment status

### What ArgoCD Shows

In the UI, you'll see:
- **Kafka broker** (StatefulSet) - running status, pod health
- **Kafka UI** - deployment status, service endpoint
- **Hermes Producer** (Job) - completion status, pod logs
- **ConfigMaps** - protobuf schema versions
- **All resource relationships** - visual graph of how everything connects

## Using ArgoCD for Visualization

### View Application Status

**In the UI:**
1. Open ArgoCD (https://localhost:8080 or your LoadBalancer IP)
2. Click on the "hermes" application
3. See a visual graph of all resources
4. Click on any resource to view details, logs, events

**Via CLI:**
```bash
kubectl get applications -n argocd
kubectl describe application hermes -n argocd
```

### View Logs

In the ArgoCD UI:
1. Click on the "hermes" application
2. Click on a pod
3. Click the "Logs" tab

### Check Sync Status

The UI will show if the cluster state matches Git:
- **Synced** - Cluster matches Git ✅
- **OutOfSync** - Someone manually changed something in the cluster ⚠️

Since GitHub Actions deploys directly, you might see "OutOfSync" status. This is normal! It means GitHub Actions deployed something that ArgoCD hasn't refreshed yet.

### Manually Sync (Optional)

If you want to deploy directly from ArgoCD:
1. Click "Sync" in the UI
2. Click "Synchronize"

This applies the manifests from Git to the cluster.

## GitHub Actions Deployment

The workflow at `.github/workflows/hermes-deploy.yml` automatically deploys when you merge to main.

### Required GitHub Secrets

- `DIGITALOCEAN_ACCESS_TOKEN` - Your DigitalOcean API token
- `DIGITALOCEAN_CLUSTER_NAME` - Your k8s cluster name

### What Gets Deployed

When changes are detected in:
- `hermes-producer/**` - Rebuilds and redeploys producer
- `hermes-schema/**` - Rebuilds producer with new schemas
- `wire/**` - Rebuilds producer with dependency changes
- `hermes/**` - Applies k8s manifest changes (kafka-broker, kafka-ui, etc.)

### View Deployment Logs

Check GitHub Actions:
1. Go to your repo → Actions tab
2. Click on the latest "Deploy Hermes Producer" workflow
3. View logs for each step

## Troubleshooting

### ArgoCD shows "OutOfSync"

This is normal! GitHub Actions deploys directly, so ArgoCD's view might be slightly behind. Click "Refresh" in the UI to update.

### Want to deploy manually from ArgoCD?

Click "Sync" in the UI. But normally GitHub Actions handles this automatically.

### ArgoCD not showing resources

1. Check the Application is created:
   ```bash
   kubectl get application hermes -n argocd
   ```

2. Check for errors:
   ```bash
   kubectl describe application hermes -n argocd
   ```

3. View ArgoCD logs:
   ```bash
   kubectl logs -n argocd deployment/argocd-application-controller
   ```

## Alternative Visualization Tools

### Lens (Recommended for Development)
- Download: https://k8slens.dev
- Desktop app with amazing UX
- No cluster installation needed
- Multi-cluster support

### k9s (Terminal UI)
```bash
brew install k9s
k9s
```

### Kubernetes Dashboard
```bash
kubectl apply -f https://raw.githubusercontent.com/kubernetes/dashboard/v2.7.0/aio/deploy/recommended.yaml
kubectl proxy
```

## ArgoCD CLI (Optional)

Install for command-line access:

```bash
# macOS
brew install argocd

# Login
argocd login localhost:8080

# View apps
argocd app list

# Get app details
argocd app get hermes

# View logs
argocd app logs hermes
```

## Configuration Files

- `hermes-application.yaml` - ArgoCD Application manifest
- `install.yaml` - ArgoCD installation reference

## Summary

**Deployment:** GitHub Actions (automated on merge to main)
**Visualization:** ArgoCD UI (shows status, logs, resource graph)
**No Git writes:** Everything stays simple and predictable

This gives you the best of both worlds:
- ✅ Beautiful visualization
- ✅ Simple deployment flow
- ✅ No branch protection issues
- ✅ Full control
