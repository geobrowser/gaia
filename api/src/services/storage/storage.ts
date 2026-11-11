import {drizzle} from "drizzle-orm/node-postgres"
import {Redacted} from "effect"
import {Pool} from "pg"

import {EnvironmentLive} from "../environment"
import {parseStatementBudgetMs, useStatementBudget} from "../statementBudget"
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

// Server-side budget for every REST / background statement on this pool. Without it this pool
// had no budget of its own: it ran under whatever session-level statement_timeout some other
// client had left on the PgBouncer server connection it landed on (30s or none — the role
// default is `0`). Same 30s as the GraphQL pool: the slowest REST request seen end to end is
// well under that. Work that needs longer opens its own transaction and sets its own
// `SET LOCAL statement_timeout`, which overrides this. See ../statementBudget.ts.
const REST_STATEMENT_TIMEOUT_MS = parseStatementBudgetMs(process.env.PG_REST_STATEMENT_TIMEOUT_MS, 30000)
useStatementBudget(pool, REST_STATEMENT_TIMEOUT_MS, (err) => {
	log.warn("Statement ran without its statement_timeout budget", {
		error: String(err),
		statementTimeoutMs: REST_STATEMENT_TIMEOUT_MS,
	})
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
