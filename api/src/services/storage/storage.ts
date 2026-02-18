import {drizzle} from "drizzle-orm/node-postgres"
import {Redacted} from "effect"
import {Pool} from "pg"

import {classifyDbFailure} from "../dbFailures"
import {withDbRetry} from "../dbRetry"
import {EnvironmentLive} from "../environment"
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

function parseEnvInt(name: string, fallback: number): number {
	const raw = process.env[name]
	if (!raw) {
		return fallback
	}

	const parsed = Number.parseInt(raw, 10)
	if (!Number.isFinite(parsed) || parsed <= 0) {
		return fallback
	}

	return parsed
}

const drizzlePoolMax = parseEnvInt("PG_DRIZZLE_POOL_MAX", 18)
const poolConnectionTimeoutMs = parseEnvInt("PG_CONNECTION_TIMEOUT_MS", 10000)
const poolIdleTimeoutMs = parseEnvInt("PG_IDLE_TIMEOUT_MS", 30000)

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

const originalPoolConnect = pool.connect.bind(pool)
pool.connect = ((...args: unknown[]) => {
	if (args.length > 0) {
		return (originalPoolConnect as (...inner: unknown[]) => unknown)(...args)
	}

	return withDbRetry(() => (originalPoolConnect as () => Promise<import("pg").PoolClient>)(), {
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
	})
}) as unknown as typeof pool.connect

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
	client: pool,
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
