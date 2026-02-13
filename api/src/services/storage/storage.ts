import {drizzle} from "drizzle-orm/node-postgres"
import {Redacted} from "effect"
import {Pool} from "pg"

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

const pool = new Pool({
	connectionString: Redacted.value(EnvironmentLive.databaseUrl),
	// REST routes (/versioned, /proposals, /profile) do sequential db.execute() calls
	// that check out a connection for ~milliseconds each. 18 is generous for this pattern.
	// With 2 replicas: (50 PostGraphile + 18 Drizzle) × 2 = 136, under PgBouncer's 200 max_client_conn.
	max: 18,
	// Close idle connections after 30s to free PgBouncer slots.
	idleTimeoutMillis: 30000,
	// Fail fast when pool is saturated — 3s means all 18 connections are busy,
	// indicating DB trouble, not normal load. See: 5d88b96.
	connectionTimeoutMillis: 3000,
	// Safety net: cancel any query that runs longer than 30s. Normal queries complete
	// in <100ms; 30s is generous enough to never trigger under normal conditions but
	// catches pathological cases (missing index, lock contention, bad query plan).
	statement_timeout: 30000,
	// Allow process to exit cleanly when pool is idle (for graceful shutdown).
	allowExitOnIdle: true,
})

pool.on("error", (err) => {
	log.error("PostgreSQL pool error", {error: String(err)})
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
	client: pool,
	schema: schemaDefinition,
})

export function getPoolStats() {
	return {
		totalConnections: pool.totalCount,
		idleConnections: pool.idleCount,
		waitingCount: pool.waitingCount,
		maxConnections: pool.options.max!, // Set to 18 in constructor above
	}
}
