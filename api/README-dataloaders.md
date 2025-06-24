# DataLoader Implementation for Pool Connection Management

## Overview

This API implements a custom batching system (similar to Facebook's DataLoader) to solve PostgreSQL connection pool exhaustion issues. Instead of making individual database queries for each GraphQL field resolver, the system batches multiple requests together and executes them in a single database query.

## The Problem

When GraphQL resolvers make individual database calls for each entity, you can quickly exhaust your connection pool:

```typescript
// BAD: This creates N+1 queries and uses N connections
const entities = await getEntities({ limit: 50 });
for (const entity of entities) {
  entity.name = await getEntityName(entity.id);        // 1 connection
  entity.description = await getEntityDescription(entity.id); // 1 connection
  entity.values = await getEntityValues(entity.id);    // 1 connection
}
// Total: 50 * 3 = 150 concurrent database connections!
```

## The Solution

Our batching system collects requests over a short time window (5ms) and executes them as a single batched query:

```typescript
// GOOD: This batches requests and uses 1 connection per batch type
const entities = await getEntities({ limit: 50 });
for (const entity of entities) {
  entity.name = await batching.loadEntityName(entity.id);        // Batched
  entity.description = await batching.loadEntityDescription(entity.id); // Batched
  entity.values = await batching.loadEntityValues(entity.id);    // Batched
}
// Total: 3 database connections (one per batch type)
```

## How It Works

### 1. SimpleBatcher Class

The core batching logic collects requests for 5ms, then executes them together:

```typescript
class SimpleBatcher<K, V> {
  // Collects requests for a short time window
  load(key: K): Promise<V>

  // Executes all pending requests as a single batch
  private async executeBatch()
}
```

### 2. Batch Functions

Each data type has a corresponding batch function:

```typescript
// Batches entity loading by ID
async (ids: string[]) => {
  const entities = await client.query.entities.findMany({
    where: (entities, { inArray }) => inArray(entities.id, ids),
  });
  // Return results in same order as input IDs
}
```

### 3. Effect Integration

The batching service integrates with your existing Effect-based architecture:

```typescript
export const make = Effect.gen(function* () {
  const storage = yield* Storage;

  const entitiesBatcher = new SimpleBatcher(/* ... */);

  return Batching.of({
    loadEntity: (id: string) => Effect.tryPromise({
      try: () => entitiesBatcher.load(id),
      catch: (error) => new BatchingError({ /* ... */ }),
    }),
  });
});
```

## Available Batching Methods

The `Batching` service provides these methods:

```typescript
interface BatchingShape {
  loadEntity(id: string): Effect<Entity | null, BatchingError, never>;
  loadEntityName(id: string): Effect<string | null, BatchingError, never>;
  loadEntityDescription(id: string): Effect<string | null, BatchingError, never>;
  loadEntityValues(entityId: string, spaceId?: string | null): Effect<any[], BatchingError, never>;
  loadEntityRelations(entityId: string, spaceId?: string | null): Effect<any[], BatchingError, never>;
  loadEntityBacklinks(entityId: string, spaceId?: string | null): Effect<any[], BatchingError, never>;
  loadProperty(propertyId: string): Effect<Property | null, BatchingError, never>;
}
```

## Usage in Resolvers

### Before (Direct Database Calls)

```typescript
export function getEntityName(id: string) {
  return Effect.gen(function* () {
    const db = yield* Storage;

    const nameProperty = yield* db.use(async (client) => {
      const result = await client.query.values.findFirst({
        where: (values, {eq, and}) =>
          and(eq(values.propertyId, SystemIds.NAME_PROPERTY), eq(values.entityId, id)),
      });
      return result;
    });

    return nameProperty?.value ?? null;
  });
}
```

### After (With Batching)

```typescript
export function getEntityName(id: string) {
  return Effect.gen(function* () {
    const batching = yield* Batching;
    const name = yield* batching.loadEntityName(id);
    return name;
  });
}
```

## GraphQL Integration

Add the batching service to your GraphQL layers:

```typescript
const EnvironmentLayer = Layer.effect(Environment, makeEnvironment);
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer));
const BatchingLayer = Layer.effect(Batching, makeBatching).pipe(Layer.provide(StorageLayer));
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer, BatchingLayer);
```

## Configuration

Batch settings can be tuned in the `SimpleBatcher` constructor:

```typescript
const entitiesBatcher = new SimpleBatcher(
  batchFunction,
  keyFunction,
  50,  // maxBatchSize: Maximum items per batch
  5    // batchDelayMs: How long to wait before executing batch
);
```

### Recommended Settings

- **Entities/Properties**: `maxBatchSize: 50`, `batchDelayMs: 5`
- **Values/Relations**: `maxBatchSize: 30`, `batchDelayMs: 5`
- **Complex queries**: `maxBatchSize: 20`, `batchDelayMs: 10`

## Performance Benefits

### Before Batching
```
50 entities × 3 queries each = 150 concurrent connections
Pool exhaustion at ~15-20 concurrent GraphQL requests
```

### After Batching
```
50 entities = 3 batched queries = 3 connections
Can handle 100+ concurrent GraphQL requests
```

## Connection Pool Monitoring

Check your pool health with the new health endpoints:

```bash
# Basic health check
curl http://localhost:3000/health

# Detailed pool statistics
curl http://localhost:3000/health/detailed

# Pool-specific metrics
curl http://localhost:3000/health/pool
```

Sample healthy response:
```json
{
  "status": "healthy",
  "connectionPool": {
    "totalConnections": 3,
    "idleConnections": 2,
    "activeConnections": 1,
    "waitingCount": 0,
    "maxConnections": 15,
    "utilizationPercent": 20,
    "status": "low"
  }
}
```

## Best Practices

### 1. Always Use Batching for Field Resolvers
```typescript
// ❌ Don't make direct database calls in field resolvers
Entity: {
  name: async (parent) => {
    const storage = yield* Storage;
    return storage.use(/* individual query */);
  }
}

// ✅ Use batching instead
Entity: {
  name: async (parent) => {
    const batching = yield* Batching;
    return batching.loadEntityName(parent.id);
  }
}
```

### 2. Batch Related Data Together
```typescript
// ✅ Load related data in parallel
const [entity, values, relations] = yield* Effect.all([
  batching.loadEntity(id),
  batching.loadEntityValues(id, spaceId),
  batching.loadEntityRelations(id, spaceId),
]);
```

### 3. Handle Errors Gracefully
```typescript
const entity = yield* batching.loadEntity(id).pipe(
  Effect.catchAll((error) => {
    console.error(`Failed to load entity ${id}:`, error);
    return Effect.succeed(null); // Return default value
  })
);
```

## Migration Guide

To migrate existing resolvers:

1. **Add Batching to your service layers**
2. **Replace direct Storage calls with Batching calls**
3. **Test with monitoring endpoints**
4. **Tune batch sizes based on usage patterns**

### Migration Checklist

- [ ] Add `BatchingLayer` to GraphQL layers
- [ ] Update entity resolvers to use `batching.loadEntity()`
- [ ] Update property resolvers to use `batching.loadProperty()`
- [ ] Replace value queries with `batching.loadEntityValues()`
- [ ] Replace relation queries with `batching.loadEntityRelations()`
- [ ] Monitor pool utilization after deployment
- [ ] Tune batch sizes if needed

## Troubleshooting

### High Pool Utilization

If you still see high pool utilization:

1. Check that all resolvers use batching
2. Increase batch delays for better batching
3. Reduce batch sizes to use fewer connections per batch

### Slow Query Performance

If batched queries are slow:

1. Add database indexes for `IN` queries
2. Reduce batch sizes
3. Consider splitting complex batch functions

### Memory Usage

If you see high memory usage:

1. Reduce batch sizes
2. Implement batch result streaming for large datasets
3. Add batch result limits

## Monitoring

Monitor these metrics in production:

- Pool utilization percentage
- Waiting client count
- Batch sizes and frequencies
- Query execution times
- Error rates per batch type

The health endpoints provide real-time metrics for monitoring and alerting.
