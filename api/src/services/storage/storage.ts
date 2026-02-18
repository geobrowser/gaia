import {drizzle} from "drizzle-orm/node-postgres"
import {Redacted} from "effect"
import {Pool} from "pg"

import {classifyDbFailure} from "../dbFailures"
import {EnvironmentLive} from "../environment"
import {parsePositiveIntEnv} from "../numberEnv"
import {createPoolWithRetryConnect} from "../poolWithRetry"
import {log} from "../telemetry"
import {
	editors,
	editorsRelations,
	entities,
	entityForeignValues,
	ipfsCache,
	members,
	membersRelations,
	meta,
	propertiesEntityRelations,
	relations,
	relationsEntityRelations,
	spaces,
	spacesRelations,
	values,
} from "./schema"

const drizzlePoolMax = parsePositiveIntEnv("PG_DRIZZLE_POOL_MAX", 18)
const poolConnectionTimeoutMs = parsePositiveIntEnv("PG_CONNECTION_TIMEOUT_MS", 10000)
const poolIdleTimeoutMs = parsePositiveIntEnv("PG_IDLE_TIMEOUT_MS", 30000)

const pool = new Pool({
	connectionString: Redacted.value(EnvironmentLive.databaseUrl),
	// REST routes (/versioned, /proposals, /profile) do sequential db.execute() calls
	// that check out a connection for ~milliseconds each. 18 is generous for this pattern.
	// With 2 replicas: (50 PostGraphile + 18 Drizzle) × 2 = 136, under PgBouncer's 200 max_client_conn.
	max: drizzlePoolMax,
	// Close idle connections after 30s to free PgBouncer slots.
	idleTimeoutMillis: poolIdleTimeoutMs,
	// Keep checkout/connect timeout aligned with GraphQL pool. Retries are handled
	// in the shared DB retry helper.
	connectionTimeoutMillis: poolConnectionTimeoutMs,
	// Allow process to exit cleanly when pool is idle (for graceful shutdown).
	allowExitOnIdle: true,
})

pool.on("error", (err) => {
	log.error("PostgreSQL pool error", {
		error: String(err),
		failureClass: classifyDbFailure(err),
		poolStats: getPoolStats(),
	})
})

const drizzlePool = createPoolWithRetryConnect({
	pool,
	operationName: "drizzle.pool.connect",
	onRetry: ({attempt, delayMs, elapsedMs, failureClass, error}) => {
		log.warn("Retrying Drizzle pool connect", {
			attempt,
			delayMs,
			elapsedMs,
			failureClass,
			error: error instanceof Error ? error.message : String(error),
			poolStats: getPoolStats(),
		})
	},
	onGiveUp: ({attempts, elapsedMs, failureClass, error, reason}) => {
		log.error("Drizzle pool connect retry exhausted", {
			attempts,
			elapsedMs,
			failureClass,
			reason,
			error: error instanceof Error ? error.message : String(error),
			poolStats: getPoolStats(),
		})
	},
})

const schemaDefinition = {
	ipfsCache,
	entities,
	values: values,
	relations: relations,
	spaces,
	members,
	editors,
	meta,

	entityForeignProperties: entityForeignValues,
	propertiesEntityRelations,
	relationsEntityRelations,
	membersRelations,
	editorsRelations,
	spacesRelations,
} as const

type DbSchema = typeof schemaDefinition

export const db = drizzle<DbSchema>({
	casing: "snake_case",
	client: drizzlePool,
	schema: schemaDefinition,
})

export function getPoolStats() {
	return {
		totalConnections: pool.totalCount,
		idleConnections: pool.idleCount,
		waitingCount: pool.waitingCount,
		maxConnections: pool.options.max ?? drizzlePoolMax,
	}
}
