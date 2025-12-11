# Kubernetes Secrets Isolation

## Current State

All services in the `kafka` namespace share a single `kafka-credentials` secret:

| Service | Secret Keys Used |
|---------|-----------------|
| kafka-ui | KAFKA_BROKER, KAFKA_USERNAME, KAFKA_PASSWORD, KAFKA_SSL_CA_PEM |
| hermes-processor | KAFKA_BROKER, KAFKA_USERNAME, KAFKA_PASSWORD, KAFKA_SSL_CA_PEM |
| atlas | KAFKA_BROKER, KAFKA_USERNAME, KAFKA_PASSWORD, KAFKA_SSL_CA_PEM |

This is acceptable for now since all services need identical Kafka credentials. However, as we add database connections and other service-specific secrets, we should isolate secrets per service.

## Proposed Structure

```
kafka-credentials          # Shared Kafka access (read-only, all services)
kafka-ui-secrets           # kafka-ui specific (currently none needed)
hermes-processor-secrets   # hermes-processor specific (future: DATABASE_URL)
atlas-secrets              # atlas specific (future: DATABASE_URL, NEO4J_URL)
```

## When to Isolate

Keep `kafka-credentials` shared since all services need identical Kafka access. Create service-specific secrets when adding:

- Database connection strings
- API keys for external services
- Service-specific tokens or certificates

## Implementation

### 1. Create service-specific secrets (when needed)

```bash
# Example: Adding a database to atlas
kubectl create secret generic atlas-secrets \
  --namespace=kafka \
  --from-literal=DATABASE_URL="postgres://user:pass@host:5432/atlas"
```

### 2. Update deployment to reference new secret

```yaml
# atlas.yaml
env:
  # Kafka credentials (shared)
  - name: KAFKA_BROKER
    valueFrom:
      secretKeyRef:
        name: kafka-credentials
        key: KAFKA_BROKER
  # ... other kafka vars ...
  
  # Atlas-specific credentials
  - name: DATABASE_URL
    valueFrom:
      secretKeyRef:
        name: atlas-secrets
        key: DATABASE_URL
```

### 3. Document secret creation in kustomization.yaml

Update the comments in `hermes/k8s/kustomization.yaml` to document required secrets:

```yaml
# Required secrets:
#
# 1. kafka-credentials (shared by all services):
#    kubectl create secret generic kafka-credentials \
#      --namespace=kafka \
#      --from-literal=KAFKA_BROKER=<broker> \
#      --from-literal=KAFKA_USERNAME=<username> \
#      --from-literal=KAFKA_PASSWORD=<password> \
#      --from-literal=KAFKA_SSL_CA_PEM="$(doctl databases get-ca <id> --format Certificate --no-header)"
#
# 2. atlas-secrets (atlas only):
#    kubectl create secret generic atlas-secrets \
#      --namespace=kafka \
#      --from-literal=DATABASE_URL=<connection-string>
```

## Future: RBAC Restrictions

For stricter isolation, add RBAC policies so each service account can only read its own secrets:

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: atlas-secrets-reader
  namespace: kafka
rules:
  - apiGroups: [""]
    resources: ["secrets"]
    resourceNames: ["kafka-credentials", "atlas-secrets"]
    verbs: ["get"]
```

This is optional and adds operational complexity. Only implement if security requirements demand it.

## Naming Convention

```
<service>-secrets    # Service-specific secrets
<shared>-credentials # Shared infrastructure credentials (kafka, redis, etc.)
```
