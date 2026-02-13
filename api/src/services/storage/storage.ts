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
	max: 18,
	idleTimeoutMillis: 30000,
	// Fail fast when pool is saturated. Matches PostGraphile pool. See: 5d88b96.
	connectionTimeoutMillis: 3000,
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
		maxConnections: pool.options.max ?? 10,
	}
}
