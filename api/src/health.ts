import { Effect } from "effect";
import { Hono } from "hono";
import { describeRoute } from "hono-openapi";
import { runtime } from "./services/runtime";
import { Storage } from "./services/storage/storage";

const health = new Hono();

// Liveness probe — proves the event loop is responsive, no DB or external dependencies.
// This must never block on I/O so Kubernetes doesn't kill healthy-but-busy pods.
health.get(
  "/liveness",
  describeRoute({
    tags: ["Health"],
    summary: "Liveness probe",
    description:
      "Returns 200 if the process is alive. No external dependency checks.",
    responses: {
      200: {
        description: "Process is alive",
        content: {
          "application/json": {
            schema: {
              type: "object",
              properties: {
                status: { type: "string", enum: ["ok"] },
              },
              required: ["status"],
            },
          },
        },
      },
    },
  }),
  (c) => c.json({ status: "ok" }),
);

// Simple health check - returns 200 if database is accessible
health.get(
  "/",
  describeRoute({
    tags: ["Health"],
    summary: "Basic health check",
    description: "Returns 200 if the database is accessible",
    responses: {
      200: {
        description: "Service is healthy",
        content: {
          "application/json": {
            schema: {
              type: "object",
              properties: {
                status: { type: "string", enum: ["healthy"] },
                timestamp: { type: "string", format: "date-time" },
              },
              required: ["status", "timestamp"],
            },
          },
        },
      },
      503: {
        description: "Service is unhealthy",
        content: {
          "application/json": {
            schema: {
              type: "object",
              properties: {
                status: { type: "string", enum: ["unhealthy"] },
                error: { type: "string" },
                timestamp: { type: "string", format: "date-time" },
              },
              required: ["status", "timestamp"],
            },
          },
        },
      },
    },
  }),
  async (c) => {
    try {
      const healthCheck = await runtime.runPromise(
        Effect.gen(function* () {
          const storage = yield* Storage;

          // Try a simple query to test connectivity
          const result = yield* storage.use(async (client) => {
            await client.execute("SELECT 1");
            return true;
          });

          return result;
        }),
      );

      if (healthCheck) {
        return c.json({
          status: "healthy",
          timestamp: new Date().toISOString(),
        });
      } else {
        return c.json(
          {
            status: "unhealthy",
            timestamp: new Date().toISOString(),
          },
          503,
        );
      }
    } catch (error) {
      return c.json(
        {
          status: "unhealthy",
          error: String(error),
          timestamp: new Date().toISOString(),
        },
        503,
      );
    }
  },
);

// Detailed health check with pool statistics
health.get(
  "/detailed",
  describeRoute({
    tags: ["Health"],
    summary: "Detailed health check",
    description:
      "Returns detailed health information including database connectivity and connection pool statistics",
    responses: {
      200: {
        description: "Service is healthy",
        content: {
          "application/json": {
            schema: {
              type: "object",
              properties: {
                status: { type: "string", enum: ["healthy"] },
                database: {
                  type: "object",
                  properties: {
                    connected: { type: "boolean" },
                    testQuery: { type: "string" },
                  },
                },
                connectionPool: {
                  type: "object",
                  properties: {
                    totalConnections: { type: "integer" },
                    idleConnections: { type: "integer" },
                    activeConnections: { type: "integer" },
                    waitingCount: { type: "integer" },
                    maxConnections: { type: "integer" },
                    utilizationPercent: { type: "integer" },
                    status: { type: "string", enum: ["low", "medium", "high"] },
                  },
                },
                recommendations: {
                  type: "array",
                  items: { type: "string" },
                },
                timestamp: { type: "string", format: "date-time" },
              },
              required: [
                "status",
                "database",
                "connectionPool",
                "recommendations",
                "timestamp",
              ],
            },
          },
        },
      },
      206: {
        description: "Service is degraded",
        content: {
          "application/json": {
            schema: {
              type: "object",
              properties: {
                status: { type: "string", enum: ["degraded"] },
                database: { type: "object" },
                connectionPool: { type: "object" },
                recommendations: { type: "array", items: { type: "string" } },
                timestamp: { type: "string", format: "date-time" },
              },
            },
          },
        },
      },
      503: {
        description: "Service is unhealthy",
        content: {
          "application/json": {
            schema: {
              type: "object",
              properties: {
                status: { type: "string", enum: ["unhealthy"] },
                error: { type: "string" },
                timestamp: { type: "string", format: "date-time" },
              },
              required: ["status", "timestamp"],
            },
          },
        },
      },
    },
  }),
  async (c) => {
    try {
      const healthData = await runtime.runPromise(
        Effect.gen(function* () {
          const storage = yield* Storage;

          // Get pool statistics
          const poolStats = yield* storage.getPoolStats();

          // Test database connectivity
          const dbConnected = yield* storage.use(async (client) => {
            const result = await client.execute(
              "SELECT 1 as test, NOW() as timestamp",
            );
            return {
              connected: true,
              testResult: result,
            };
          });

          const utilizationPercent = Math.round(
            (poolStats.totalConnections / poolStats.maxConnections) * 100,
          );

          const isHealthy =
            dbConnected.connected &&
            utilizationPercent < 90 &&
            poolStats.waitingCount === 0;

          return {
            status: isHealthy ? "healthy" : "degraded",
            database: {
              connected: dbConnected.connected,
              testQuery: "SELECT 1",
            },
            connectionPool: {
              totalConnections: poolStats.totalConnections,
              idleConnections: poolStats.idleConnections,
              activeConnections:
                poolStats.totalConnections - poolStats.idleConnections,
              waitingCount: poolStats.waitingCount,
              maxConnections: poolStats.maxConnections,
              utilizationPercent,
              status:
                utilizationPercent > 85
                  ? "high"
                  : utilizationPercent > 70
                    ? "medium"
                    : "low",
            },
            recommendations: getHealthRecommendations(
              poolStats,
              utilizationPercent,
            ),
            timestamp: new Date().toISOString(),
          };
        }),
      );

      const statusCode =
        healthData.status === "healthy"
          ? 200
          : healthData.status === "degraded"
            ? 206
            : 503;

      return c.json(healthData, statusCode);
    } catch (error) {
      return c.json(
        {
          status: "unhealthy",
          error: String(error),
          timestamp: new Date().toISOString(),
        },
        503,
      );
    }
  },
);

// Pool-specific metrics endpoint
health.get(
  "/pool",
  describeRoute({
    tags: ["Health"],
    summary: "Connection pool metrics",
    description: "Returns connection pool statistics and status",
    responses: {
      200: {
        description: "Pool statistics",
        content: {
          "application/json": {
            schema: {
              type: "object",
              properties: {
                totalConnections: { type: "integer" },
                idleConnections: { type: "integer" },
                activeConnections: { type: "integer" },
                waitingCount: { type: "integer" },
                maxConnections: { type: "integer" },
                utilizationPercent: { type: "integer" },
                status: { type: "string", enum: ["ok", "warning", "critical"] },
                timestamp: { type: "string", format: "date-time" },
              },
              required: [
                "totalConnections",
                "idleConnections",
                "activeConnections",
                "waitingCount",
                "maxConnections",
                "utilizationPercent",
                "status",
                "timestamp",
              ],
            },
          },
        },
      },
      500: {
        description: "Failed to retrieve pool statistics",
        content: {
          "application/json": {
            schema: {
              type: "object",
              properties: {
                error: { type: "string" },
                timestamp: { type: "string", format: "date-time" },
              },
              required: ["error", "timestamp"],
            },
          },
        },
      },
    },
  }),
  async (c) => {
    try {
      const poolData = await runtime.runPromise(
        Effect.gen(function* () {
          const storage = yield* Storage;
          const poolStats = yield* storage.getPoolStats();

          const utilizationPercent = Math.round(
            (poolStats.totalConnections / poolStats.maxConnections) * 100,
          );

          return {
            ...poolStats,
            activeConnections:
              poolStats.totalConnections - poolStats.idleConnections,
            utilizationPercent,
            status:
              utilizationPercent > 85
                ? "critical"
                : utilizationPercent > 70
                  ? "warning"
                  : "ok",
            timestamp: new Date().toISOString(),
          };
        }),
      );

      return c.json(poolData);
    } catch (error) {
      return c.json(
        {
          error: String(error),
          timestamp: new Date().toISOString(),
        },
        500,
      );
    }
  },
);

// Helper function to provide health recommendations
function getHealthRecommendations(
  poolStats: {
    totalConnections: number;
    idleConnections: number;
    waitingCount: number;
    maxConnections: number;
  },
  utilizationPercent: number,
): string[] {
  const recommendations: string[] = [];

  if (utilizationPercent > 85) {
    recommendations.push(
      "High pool utilization detected. Consider implementing DataLoaders to batch queries.",
    );
    recommendations.push(
      "Consider increasing max pool connections if server resources allow.",
    );
  }

  if (poolStats.waitingCount > 0) {
    recommendations.push(
      `${poolStats.waitingCount} clients waiting for connections. Implement query batching.`,
    );
  }

  if (
    poolStats.idleConnections === 0 &&
    poolStats.totalConnections === poolStats.maxConnections
  ) {
    recommendations.push(
      "Pool is fully utilized with no idle connections. Consider optimizing query performance.",
    );
  }

  if (poolStats.totalConnections < poolStats.maxConnections * 0.1) {
    recommendations.push(
      "Very low pool utilization. Consider reducing max connections to free resources.",
    );
  }

  if (recommendations.length === 0) {
    recommendations.push("Pool health is optimal.");
  }

  return recommendations;
}

export { health };
