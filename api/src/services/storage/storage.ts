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
	// Allow process to exit cleanly when pool is idle (for graceful shutdown).
	allowExitOnIdle: true,
})

pool.on("error", (err) => {
	log.error("PostgreSQL pool error", {error: String(err)})
})

// Dedicated pool for POST /v2/versioned/review — the only endpoint that runs heavy,
// fan-out diff computation on untrusted client input. Isolated from the shared pool
// (so a burst of expensive reviews can't starve the other REST routes' 18 slots) and
// capped at a 10s statement_timeout per query so one pathological edit can't hold a
// connection indefinitely. Small + interactive use → a low max is plenty.
const reviewPool = new Pool({
	connectionString: Redacted.value(EnvironmentLive.databaseUrl),
	max: 6,
	idleTimeoutMillis: 30000,
	connectionTimeoutMillis: 3000,
	allowExitOnIdle: true,
	statement_timeout: 10_000,
})

reviewPool.on("error", (err) => {
	log.error("PostgreSQL review pool error", {error: String(err)})
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

/** Drizzle handle over the isolated, statement_timeout-bounded review pool. */
export const reviewDb = drizzle<DbSchema>({
	casing: "snake_case",
	client: reviewPool,
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
