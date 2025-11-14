# ArgoCD Setup for Hermes

This directory contains ArgoCD configuration for automated GitOps deployment of Hermes to production.

## What is ArgoCD?

ArgoCD is a GitOps continuous delivery tool for Kubernetes. It automatically syncs your k8s cluster with your Git repository, providing:
- Automatic deployment when you merge to `main`
- Visual UI showing deployment status
- Automatic drift detection and self-healing
- Easy rollbacks
- Deployment history

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
- Automatically syncs changes to the cluster
- Self-heals if someone manually changes resources in the cluster

## How It Works

### Automated Deployment Flow

1. You make changes to files in `hermes/`
2. You merge a PR to `main`
3. ArgoCD detects the change (within ~3 minutes)
4. ArgoCD automatically applies the changes to the cluster
5. You can watch the deployment in the ArgoCD UI

### What Gets Deployed

ArgoCD will deploy all resources in `hermes/overlays/digitalocean/`:
- Kafka broker (StatefulSet)
- Kafka UI
- Hermes Producer (Job)
- ConfigMaps (protobuf schemas)

## Using ArgoCD

### View Deployment Status

```bash
# CLI
kubectl get applications -n argocd

# Or use the ArgoCD UI (much better!)
```

### Manually Trigger Sync

```bash
kubectl patch application hermes -n argocd --type merge -p '{"metadata": {"annotations": {"argocd.argoproj.io/refresh": "normal"}}}'
```

Or click "Sync" in the UI.

### View Application Details

```bash
# Get app status
kubectl get application hermes -n argocd -o yaml

# View sync status
kubectl describe application hermes -n argocd
```

### Rollback to Previous Version

In the ArgoCD UI:
1. Go to the Hermes application
2. Click "History and Rollback"
3. Select the version you want to rollback to
4. Click "Rollback"

## Docker Image Updates

For the hermes-producer Docker image, you still need to:

1. Build and push the image to the registry
2. Update the image tag in `hermes/base/hermes-producer.yaml` if you want to use specific versions

Or keep using `:latest` tag and ArgoCD will restart the Job when the manifest changes.

### Recommended Workflow with Image Tags

Update the GitHub Actions workflow to:
1. Build image with tag `v1.2.3` (or commit SHA)
2. Push to registry
3. Update `hermes/base/hermes-producer.yaml` with the new tag
4. Commit and push the tag update
5. ArgoCD automatically deploys the new version

## ArgoCD CLI (Optional)

Install the ArgoCD CLI for more control:

```bash
# macOS
brew install argocd

# Login
argocd login localhost:8080

# View apps
argocd app list

# Sync app
argocd app sync hermes

# View logs
argocd app logs hermes
```

## Troubleshooting

### Application not syncing

Check the application status:
```bash
kubectl get application hermes -n argocd -o jsonpath='{.status.sync.status}'
```

View sync errors:
```bash
kubectl describe application hermes -n argocd
```

### Force refresh

```bash
argocd app get hermes --refresh
```

### View ArgoCD logs

```bash
kubectl logs -n argocd deployment/argocd-application-controller
```

## Security Notes

1. **Private Repositories**: If your repo is private, you need to configure ArgoCD with access:
   ```bash
   argocd repo add https://github.com/geobrowser/gaia.git --username <username> --password <token>
   ```

2. **RBAC**: ArgoCD has built-in RBAC. Configure users/permissions as needed.

3. **Secrets**: Don't store secrets in Git. Use sealed-secrets or external-secrets with ArgoCD.

## Next Steps

1. ✅ Install ArgoCD in your cluster
2. ✅ Deploy the Hermes Application
3. Update the GitHub Actions workflow to build Docker images only (remove k8s apply)
4. Consider using image tags instead of `:latest` for better version tracking
5. Set up notifications (Slack, etc.) for deployment status
