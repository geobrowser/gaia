import {Hono} from "hono"
import {describeRoute} from "hono-openapi"
import {renderQueryCostHistogram} from "./kg/costLoggerPlugin"
import {getGraphqlPoolPressure, getGraphqlPoolStats} from "./kg/postgraphile"
import {db, getPoolStats} from "./services/storage/storage"
import {log} from "./services/telemetry"

const health = new Hono()

const READINESS_DB_TIMEOUT_MS = parseInt(process.env.READINESS_DB_TIMEOUT_MS || "1000", 10)

type PoolStats = {
	totalConnections: number
	idleConnections: number
	waitingCount: number
	maxConnections: number
}

async function isDatabaseReachable(): Promise<boolean> {
	try {
		await Promise.race([
			db.execute("SELECT 1"),
			new Promise((_, reject) =>
				setTimeout(() => reject(new Error("readiness_db_timeout")), READINESS_DB_TIMEOUT_MS),
			),
		])
		return true
	} catch {
		return false
	}
}

function poolUtilizationRatio(poolStats: PoolStats): number {
	if (poolStats.maxConnections <= 0) {
		return 0
	}

	return poolStats.totalConnections / poolStats.maxConnections
}

function renderGaugeMetric(name: string, help: string, value: number): string {
	const normalizedValue = Number.isFinite(value) ? value : 0
	return `# HELP ${name} ${help}\n# TYPE ${name} gauge\n${name} ${normalizedValue}\n`
}

function renderPrometheusMetrics(): string {
	const dbPoolStats = getPoolStats()
	const graphqlPoolStats = getGraphqlPoolStats()
	const graphqlPoolPressure = getGraphqlPoolPressure()

	return [
		renderGaugeMetric(
			"gaia_api_db_pool_total_connections",
			"Total connections currently tracked by the REST/Drizzle PostgreSQL pool.",
			dbPoolStats.totalConnections,
		),
		renderGaugeMetric(
			"gaia_api_db_pool_idle_connections",
			"Idle connections currently available in the REST/Drizzle PostgreSQL pool.",
			dbPoolStats.idleConnections,
		),
		renderGaugeMetric(
			"gaia_api_db_pool_waiting_count",
			"Requests currently waiting for a REST/Drizzle PostgreSQL pool client.",
			dbPoolStats.waitingCount,
		),
		renderGaugeMetric(
			"gaia_api_db_pool_max_connections",
			"Configured maximum size of the REST/Drizzle PostgreSQL pool.",
			dbPoolStats.maxConnections,
		),
		renderGaugeMetric(
			"gaia_api_db_pool_utilization_ratio",
			"Current utilization ratio of the REST/Drizzle PostgreSQL pool.",
			poolUtilizationRatio(dbPoolStats),
		),
		renderGaugeMetric(
			"gaia_api_graphql_pool_total_connections",
			"Total connections currently tracked by the GraphQL/PostGraphile PostgreSQL pool.",
			graphqlPoolStats.totalConnections,
		),
		renderGaugeMetric(
			"gaia_api_graphql_pool_idle_connections",
			"Idle connections currently available in the GraphQL/PostGraphile PostgreSQL pool.",
			graphqlPoolStats.idleConnections,
		),
		renderGaugeMetric(
			"gaia_api_graphql_pool_waiting_count",
			"Requests currently waiting for a GraphQL/PostGraphile PostgreSQL pool client.",
			graphqlPoolStats.waitingCount,
		),
		renderGaugeMetric(
			"gaia_api_graphql_pool_max_connections",
			"Configured maximum size of the GraphQL/PostGraphile PostgreSQL pool.",
			graphqlPoolStats.maxConnections,
		),
		renderGaugeMetric(
			"gaia_api_graphql_pool_utilization_ratio",
			"Current utilization ratio of the GraphQL/PostGraphile PostgreSQL pool.",
			poolUtilizationRatio(graphqlPoolStats),
		),
		renderGaugeMetric(
			"gaia_api_graphql_pool_pressured",
			"Whether the GraphQL/PostGraphile PostgreSQL pool is currently pressured (1=true, 0=false).",
			graphqlPoolPressure.isPressured ? 1 : 0,
		),
		renderGaugeMetric(
			"gaia_api_graphql_pool_saturated",
			"Whether the GraphQL/PostGraphile PostgreSQL pool is currently saturated (1=true, 0=false).",
			graphqlPoolPressure.isSaturated ? 1 : 0,
		),
		renderGaugeMetric(
			"gaia_api_graphql_pool_recent_acquire_timeouts",
			"Recent GraphQL/PostGraphile pool acquire timeouts inside the configured moving window.",
			graphqlPoolPressure.recentAcquireTimeouts,
		),
		// GraphQL query cost histogram (accumulated since process start;
		// Prometheus derives time-windowed views via rate / histogram_quantile).
		renderQueryCostHistogram(),
	].join("\n")
}

// Liveness probe — proves the event loop is responsive, no DB or external dependencies.
// This must never block on I/O so Kubernetes doesn't kill healthy-but-busy pods.
health.get(
	"/liveness",
	describeRoute({
		tags: ["Health"],
		summary: "Liveness probe",
		description: "Returns 200 if the process is alive. No external dependency checks.",
		responses: {
			200: {
				description: "Process is alive",
				content: {
					"application/json": {
						schema: {
							type: "object",
							properties: {
								status: {type: "string", enum: ["ok"]},
							},
							required: ["status"],
						},
					},
				},
			},
		},
	}),
	(c) => c.json({status: "ok"}),
)

// Readiness probe — fail only when the pod cannot reach its core dependency.
// Local query saturation is handled by request shedding in /graphql so we do not
// drop every pod from service endpoints during a shared overload event.
health.get(
	"/readiness",
	describeRoute({
		tags: ["Health"],
		summary: "Readiness probe",
		description: "Returns 200 when pod is ready to receive traffic, 503 when the database is unavailable.",
		responses: {
			200: {
				description: "Pod is ready",
				content: {
					"application/json": {
						schema: {
							type: "object",
							properties: {
								status: {type: "string", enum: ["ready"]},
								timestamp: {type: "string", format: "date-time"},
							},
							required: ["status", "timestamp"],
						},
					},
				},
			},
			503: {
				description: "Pod is temporarily not ready because the database is unavailable",
				content: {
					"application/json": {
						schema: {
							type: "object",
							properties: {
								status: {type: "string", enum: ["not_ready"]},
								reason: {type: "string"},
								timestamp: {type: "string", format: "date-time"},
							},
							required: ["status", "reason", "timestamp"],
						},
					},
				},
			},
		},
	}),
	async (c) => {
		const databaseReachable = await isDatabaseReachable()
		const timestamp = new Date().toISOString()

		if (!databaseReachable) {
			log.warn("Readiness probe failed: database unreachable", {
				path: c.req.path,
				readinessDbTimeoutMs: READINESS_DB_TIMEOUT_MS,
			})

			return c.json(
				{
					status: "not_ready",
					reason: "database_unreachable",
					timestamp,
				},
				503,
			)
		}

		return c.json({
			status: "ready",
			timestamp,
		})
	},
)

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
								status: {type: "string", enum: ["healthy"]},
								timestamp: {type: "string", format: "date-time"},
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
								status: {type: "string", enum: ["unhealthy"]},
								error: {type: "string"},
								timestamp: {type: "string", format: "date-time"},
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
			await db.execute("SELECT 1")
			return c.json({
				status: "healthy",
				timestamp: new Date().toISOString(),
			})
		} catch (error) {
			return c.json(
				{
					status: "unhealthy",
					error: String(error),
					timestamp: new Date().toISOString(),
				},
				503,
			)
		}
	},
)

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
								status: {type: "string", enum: ["healthy"]},
								database: {
									type: "object",
									properties: {
										connected: {type: "boolean"},
										testQuery: {type: "string"},
									},
								},
								connectionPool: {
									type: "object",
									properties: {
										totalConnections: {type: "integer"},
										idleConnections: {type: "integer"},
										activeConnections: {type: "integer"},
										waitingCount: {type: "integer"},
										maxConnections: {type: "integer"},
										utilizationPercent: {type: "integer"},
										status: {type: "string", enum: ["low", "medium", "high"]},
									},
								},
								recommendations: {
									type: "array",
									items: {type: "string"},
								},
								timestamp: {type: "string", format: "date-time"},
							},
							required: ["status", "database", "connectionPool", "recommendations", "timestamp"],
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
								status: {type: "string", enum: ["degraded"]},
								database: {type: "object"},
								connectionPool: {type: "object"},
								recommendations: {type: "array", items: {type: "string"}},
								timestamp: {type: "string", format: "date-time"},
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
								status: {type: "string", enum: ["unhealthy"]},
								error: {type: "string"},
								timestamp: {type: "string", format: "date-time"},
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
			await db.execute("SELECT 1")

			const poolStats = getPoolStats()
			const graphqlPoolPressure = getGraphqlPoolPressure()
			const utilizationPercent = Math.round((poolStats.totalConnections / poolStats.maxConnections) * 100)
			const isHealthy =
				utilizationPercent < 90 && poolStats.waitingCount === 0 && !graphqlPoolPressure.isSaturated

			const healthData = {
				status: isHealthy ? "healthy" : "degraded",
				database: {
					connected: true,
					testQuery: "SELECT 1",
				},
				connectionPool: {
					totalConnections: poolStats.totalConnections,
					idleConnections: poolStats.idleConnections,
					activeConnections: poolStats.totalConnections - poolStats.idleConnections,
					waitingCount: poolStats.waitingCount,
					maxConnections: poolStats.maxConnections,
					utilizationPercent,
					status: utilizationPercent > 85 ? "high" : utilizationPercent > 70 ? "medium" : "low",
				},
				graphqlPoolPressure,
				recommendations: getHealthRecommendations(poolStats, utilizationPercent),
				timestamp: new Date().toISOString(),
			}

			const statusCode = healthData.status === "healthy" ? 200 : 206
			return c.json(healthData, statusCode)
		} catch (error) {
			return c.json(
				{
					status: "unhealthy",
					error: String(error),
					timestamp: new Date().toISOString(),
				},
				503,
			)
		}
	},
)

// Pool-specific metrics endpoint
health.get(
	"/graphql-pool",
	describeRoute({
		tags: ["Health"],
		summary: "GraphQL connection pool metrics",
		description: "Returns PostGraphile connection pool statistics and status",
		responses: {
			200: {
				description: "GraphQL pool statistics",
				content: {
					"application/json": {
						schema: {
							type: "object",
							properties: {
								totalConnections: {type: "integer"},
								idleConnections: {type: "integer"},
								activeConnections: {type: "integer"},
								waitingCount: {type: "integer"},
								maxConnections: {type: "integer"},
								utilizationPercent: {type: "integer"},
								recentAcquireTimeouts: {type: "integer"},
								poolPressure: {type: "object"},
								status: {type: "string", enum: ["ok", "warning", "critical"]},
								timestamp: {type: "string", format: "date-time"},
							},
							required: [
								"totalConnections",
								"idleConnections",
								"activeConnections",
								"waitingCount",
								"maxConnections",
								"utilizationPercent",
								"recentAcquireTimeouts",
								"poolPressure",
								"status",
								"timestamp",
							],
						},
					},
				},
			},
		},
	}),
	(c) => {
		const poolStats = getGraphqlPoolStats()
		const poolPressure = getGraphqlPoolPressure()
		const utilizationPercent = Math.round((poolStats.totalConnections / poolStats.maxConnections) * 100)

		return c.json({
			...poolStats,
			activeConnections: poolStats.totalConnections - poolStats.idleConnections,
			utilizationPercent,
			recentAcquireTimeouts: poolPressure.recentAcquireTimeouts,
			poolPressure,
			status: utilizationPercent > 85 ? "critical" : utilizationPercent > 70 ? "warning" : "ok",
			timestamp: new Date().toISOString(),
		})
	},
)

health.get(
	"/metrics",
	describeRoute({
		tags: ["Health"],
		summary: "Prometheus metrics",
		description: "Returns Prometheus-formatted PostgreSQL pool metrics for autoscaling and alerting.",
		responses: {
			200: {
				description: "Prometheus metrics payload",
				content: {
					"text/plain": {
						schema: {
							type: "string",
						},
					},
				},
			},
		},
	}),
	(c) =>
		c.body(renderPrometheusMetrics(), 200, {
			"content-type": "text/plain; version=0.0.4; charset=utf-8",
			"cache-control": "no-store",
		}),
)

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
								totalConnections: {type: "integer"},
								idleConnections: {type: "integer"},
								activeConnections: {type: "integer"},
								waitingCount: {type: "integer"},
								maxConnections: {type: "integer"},
								utilizationPercent: {type: "integer"},
								status: {type: "string", enum: ["ok", "warning", "critical"]},
								timestamp: {type: "string", format: "date-time"},
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
								error: {type: "string"},
								timestamp: {type: "string", format: "date-time"},
							},
							required: ["error", "timestamp"],
						},
					},
				},
			},
		},
	}),
	(c) => {
		const poolStats = getPoolStats()
		const utilizationPercent = Math.round((poolStats.totalConnections / poolStats.maxConnections) * 100)

		return c.json({
			...poolStats,
			activeConnections: poolStats.totalConnections - poolStats.idleConnections,
			utilizationPercent,
			status: utilizationPercent > 85 ? "critical" : utilizationPercent > 70 ? "warning" : "ok",
			timestamp: new Date().toISOString(),
		})
	},
)

// Helper function to provide health recommendations
function getHealthRecommendations(
	poolStats: {
		totalConnections: number
		idleConnections: number
		waitingCount: number
		maxConnections: number
	},
	utilizationPercent: number,
): string[] {
	const recommendations: string[] = []

	if (utilizationPercent > 85) {
		recommendations.push("High pool utilization detected. Consider implementing DataLoaders to batch queries.")
		recommendations.push("Consider increasing max pool connections if server resources allow.")
	}

	if (poolStats.waitingCount > 0) {
		recommendations.push(`${poolStats.waitingCount} clients waiting for connections. Implement query batching.`)
	}

	if (poolStats.idleConnections === 0 && poolStats.totalConnections === poolStats.maxConnections) {
		recommendations.push("Pool is fully utilized with no idle connections. Consider optimizing query performance.")
	}

	if (poolStats.totalConnections < poolStats.maxConnections * 0.1) {
		recommendations.push("Very low pool utilization. Consider reducing max connections to free resources.")
	}

	if (recommendations.length === 0) {
		recommendations.push("Pool health is optimal.")
	}

	return recommendations
}

export {health}
