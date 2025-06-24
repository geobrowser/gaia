import {Effect, Layer} from "effect"
import {Hono} from "hono"
import {Environment, make as makeEnvironment} from "./services/environment"
import {make as makeStorage, Storage} from "./services/storage/storage"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

const health = new Hono()

// Simple health check - returns 200 if database is accessible
health.get("/", async (c) => {
	try {
		const healthCheck = await Effect.runPromise(
			Effect.gen(function* () {
				const storage = yield* Storage

				// Try a simple query to test connectivity
				const result = yield* storage.use(async (client) => {
					await client.execute("SELECT 1")
					return true
				})

				return result
			}).pipe(provideDeps),
		)

		if (healthCheck) {
			return c.json({
				status: "healthy",
				timestamp: new Date().toISOString(),
			})
		} else {
			return c.json(
				{
					status: "unhealthy",
					timestamp: new Date().toISOString(),
				},
				503,
			)
		}
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
})

// Detailed health check with pool statistics
health.get("/detailed", async (c) => {
	try {
		const healthData = await Effect.runPromise(
			Effect.gen(function* () {
				const storage = yield* Storage

				// Get pool statistics
				const poolStats = yield* storage.getPoolStats()

				// Test database connectivity
				const dbConnected = yield* storage.use(async (client) => {
					const result = await client.execute("SELECT 1 as test, NOW() as timestamp")
					return {
						connected: true,
						testResult: result,
					}
				})

				const utilizationPercent = Math.round((poolStats.totalConnections / poolStats.maxConnections) * 100)

				const isHealthy = dbConnected.connected && utilizationPercent < 90 && poolStats.waitingCount === 0

				return {
					status: isHealthy ? "healthy" : "degraded",
					database: {
						connected: dbConnected.connected,
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
					recommendations: getHealthRecommendations(poolStats, utilizationPercent),
					timestamp: new Date().toISOString(),
				}
			}).pipe(provideDeps),
		)

		const statusCode = healthData.status === "healthy" ? 200 : healthData.status === "degraded" ? 206 : 503

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
})

// Pool-specific metrics endpoint
health.get("/pool", async (c) => {
	try {
		const poolData = await Effect.runPromise(
			Effect.gen(function* () {
				const storage = yield* Storage
				const poolStats = yield* storage.getPoolStats()

				const utilizationPercent = Math.round((poolStats.totalConnections / poolStats.maxConnections) * 100)

				return {
					...poolStats,
					activeConnections: poolStats.totalConnections - poolStats.idleConnections,
					utilizationPercent,
					status: utilizationPercent > 85 ? "critical" : utilizationPercent > 70 ? "warning" : "ok",
					timestamp: new Date().toISOString(),
				}
			}).pipe(provideDeps),
		)

		return c.json(poolData)
	} catch (error) {
		return c.json(
			{
				error: String(error),
				timestamp: new Date().toISOString(),
			},
			500,
		)
	}
})

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
