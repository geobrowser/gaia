# Production Kafka Setup Guide

This guide shows how to properly configure Kafka for production use with external access.

## Current Issue

Kafka needs to advertise its external address to clients. The current setup uses the LoadBalancer IP directly, which can change and must be manually updated.

## Production Solution

### Step 1: Reserve a Static IP

Reserve a static IP in your region:

```bash
# Reserve an IP in NYC2
doctl compute reserved-ip create --region nyc2

# Output will show your reserved IP, e.g., 165.227.123.45
```

### Step 2: Update the LoadBalancer Service

Add the reserved IP to the broker service:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: broker
  namespace: kafka
  annotations:
    service.beta.kubernetes.io/do-loadbalancer-reserved-ip: "YOUR-RESERVED-IP"
spec:
  type: LoadBalancer
  # ... rest of config
```

Or apply via kubectl:

```bash
kubectl annotate svc broker -n kafka \
  service.beta.kubernetes.io/do-loadbalancer-reserved-ip="YOUR-RESERVED-IP"
```

### Step 3: (Optional but Recommended) Set up DNS

Add a DNS A record pointing to your reserved IP:

```
kafka.yourdomain.com  →  YOUR-RESERVED-IP
```

### Step 4: Update Kafka Configuration

Update `kafka-broker.yaml` with your static IP or domain:

**Option A: Using Static IP**
```yaml
- name: KAFKA_ADVERTISED_LISTENERS
  value: "PLAINTEXT://kafka-broker-0.broker.kafka.svc.cluster.local:29092,PLAINTEXT_HOST://YOUR-RESERVED-IP:9092"
```

**Option B: Using Domain (Recommended)**
```yaml
- name: KAFKA_ADVERTISED_LISTENERS
  value: "PLAINTEXT://kafka-broker-0.broker.kafka.svc.cluster.local:29092,PLAINTEXT_HOST://kafka.yourdomain.com:9092"
```

### Step 5: Deploy

```bash
kubectl apply -f kafka-broker.yaml
```

## Complete Production Checklist

### Required
- [x] Single broker running with persistent storage
- [ ] Reserved static IP
- [ ] DNS record (optional but recommended)
- [ ] Updated KAFKA_ADVERTISED_LISTENERS

### Recommended
- [ ] **3-broker cluster** for high availability (see `kafka-broker-3node.yaml`)
- [ ] **TLS/SSL encryption** for data in transit
- [ ] **SASL authentication** for security
- [ ] **Monitoring** (Prometheus + Grafana)
- [ ] **Resource limits** properly sized for workload
- [ ] **Backup strategy** for persistent volumes
- [ ] **Network policies** to restrict access

### Production 3-Broker Setup

For true production, you should use the 3-broker setup:

```bash
# Use the 3-node configuration
cp kafka-broker-3node.yaml kafka-broker.yaml

# Fix the KRaft quorum issues (we need to debug this)
# Then deploy
./deploy.sh
```

## Current Workaround

Until you set up a reserved IP, you can:

1. **Use the current LoadBalancer IP**: `138.197.252.214`
2. **Update when it changes**: If you redeploy and get a new IP, update the config
3. **For development**: Use port-forward instead

```bash
kubectl port-forward -n kafka svc/broker 9092:9092
KAFKA_BROKER=localhost:9092 cargo run
```

## Cost Breakdown

**Production Setup:**
- Reserved IP: $4/month
- LoadBalancer: $12/month
- Block Storage (20GB): $2/month
- **Total: $18/month**

**3-Broker Production:**
- Reserved IP: $4/month
- LoadBalancer: $12/month
- Block Storage (60GB): $6/month
- **Total: $22/month**

## Security Best Practices

1. **Enable TLS**: Encrypt data in transit
2. **Enable SASL**: Require authentication
3. **Use Network Policies**: Restrict which pods can access Kafka
4. **Firewall Rules**: Limit source IPs that can access the LoadBalancer
5. **Regular Updates**: Keep Kafka version up to date

## Quick Start (Using Current IP)

For now, to get your producer working:

```bash
# Check the current LoadBalancer IP
kubectl get svc broker -n kafka

# Make sure kafka-broker.yaml has that IP in KAFKA_ADVERTISED_LISTENERS
# Current IP: 138.197.252.214

# If it matches, redeploy
kubectl delete statefulset kafka-broker -n kafka
kubectl delete pvc kafka-data-kafka-broker-0 -n kafka
kubectl apply -f kafka-broker.yaml

# Wait for it to start
kubectl get pods -n kafka -w

# Test the producer
cd ../producer
KAFKA_BROKER=138.197.252.214:9092 cargo run
```
