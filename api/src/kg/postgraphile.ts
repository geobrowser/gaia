import SimplifyInflectionPlugin from "@graphile-contrib/pg-simplify-inflector"
import {useResponseCache} from "@graphql-yoga/plugin-response-cache"
import {createYoga, type Plugin, useExecutionCancellation} from "graphql-yoga"
import {LRUCache} from "lru-cache"
import type {PoolClient} from "pg"
import {Pool} from "pg"
import {createPostGraphileSchema} from "postgraphile"
import ConnectionFilterPlugin from "postgraphile-plugin-connection-filter"
import EntitySpaceFilterPlugin from "./entitySpaceFilterPlugin"
import {useGraphQLInstrumentation} from "./instrumentationPlugin"
import UndashedUuidPlugin from "./uuidScalarPlugin"
import ValueScalarsPlugin from "./valueScalarsPlugin"

// Server context passed from HTTP middleware
export type GraphQLServerContext = {
	traceContext?: {
		traceId: string
		spanId: string
		traceFlags: number
	}
}

// Create PostgreSQL pool with explicit configuration to prevent connection exhaustion
// Note: Without PgBouncer, each pool connection = 1 Postgres connection.
// Ensure max * num_replicas < Postgres max_connections (leaving room for admin/migrations).
if (!process.env.DATABASE_URL) {
	throw new Error("DATABASE_URL environment variable is required")
}

const pgPool = new Pool({
	connectionString: process.env.DATABASE_URL,
	// Pool size - PgBouncer handles multiplexing, so we can be generous here.
	// The real PostgreSQL connection limit is managed by PgBouncer's pool_size.
	// With 2 replicas at 50 each = 100 connections, well under PgBouncer's 200 max_client_conn.
	max: parseInt(process.env.PG_POOL_MAX || "50", 10),
	// Fail fast if no connection available (default is 0 = wait forever, causing hangs)
	connectionTimeoutMillis: parseInt(process.env.PG_CONNECTION_TIMEOUT_MS || "3000", 10),
	// Close idle connections after 30 seconds to free up PgBouncer slots
	idleTimeoutMillis: parseInt(process.env.PG_IDLE_TIMEOUT_MS || "30000", 10),
	// Allow process to exit cleanly when pool is idle (for graceful shutdown)
	allowExitOnIdle: true,
})

// Base PostGraphile options (without uuidScalarPlugin)
const postgraphileOptions = {
	watchPg: false,
	graphiql: true,
	enhanceGraphiql: true,
	dynamicJson: true,
	setofFunctionsContainNulls: false,
	ignoreRBAC: false,
	// NOTE: Plugin order is intentional:
	// - UndashedUuidPlugin patches the UUID scalar first so that all subsequent
	//   plugins (including ConnectionFilterPlugin) build their types against the
	//   undashed UUID behavior. This has been verified to work with both dashed
	//   and undashed UUID inputs in filters.
	// - ValueScalarsPlugin registers custom scalars (GeoPoint, GeoRect, Date, etc.)
	//   and remaps Value fields to use them for self-documenting schema
	// - EntitySpaceFilterPlugin adds efficient spaceId filter using EXISTS instead of computed column
	appendPlugins: [
		UndashedUuidPlugin,
		ValueScalarsPlugin,
		ConnectionFilterPlugin,
		SimplifyInflectionPlugin,
		EntitySpaceFilterPlugin,
	],
	disableDefaultMutations: true,
	simpleCollections: "both" as const,
	graphileBuildOptions: {
		connectionFilterRelations: true,
		connectionFilterComputedColumns: true,
		connectionFilterAllowNullInput: true, // default: false
		connectionFilterAllowEmptyObjectInput: true, // default: false
		connectionFilterOperatorNames: {
			equalTo: "is",
			equalToInsensitive: "isInsensitive",
			notEqualTo: "isNot",
			notEqualToInsensitive: "isNotInsensitive",
			contains: "in",
		},
		pgOmitListSuffix: true,
	},
}

// Create PostGraphile schemas
const postgraphileSchema = await createPostGraphileSchema(pgPool, ["public"], postgraphileOptions)

/**
 * Yoga plugin that manages the pgClient lifecycle for PostGraphile resolvers.
 *
 * PostGraphile's generated resolvers expect `context.pgClient` — a pg.Client
 * checked out from the pool. This plugin checks out a client before execution
 * and releases it back to the pool after execution completes.
 *
 * The checkout happens in onExecute (not in the context factory) so that
 * cache hits from useResponseCache never check out a client at all — the
 * response cache short-circuits in onParams before onExecute fires.
 *
 * We don't use PostGraphile's `withPostGraphileContext` because we don't need
 * its transaction wrapper or JWT/role features (mutations are disabled, all
 * queries are reads). Checking out the client directly is simpler and avoids
 * the unnecessary BEGIN/COMMIT overhead on every request.
 */
function usePgClient(pool: Pool): Plugin<{pgClient: PoolClient}> {
	return {
		async onExecute({extendContext}) {
			const pgClient = await pool.connect()
			extendContext({pgClient})

			return {
				onExecuteDone() {
					pgClient.release()
				},
			}
		},
	}
}

/**
 * Simple TTL-based response cache backed by lru-cache.
 *
 * The default createInMemoryCache from @envelop/response-cache maintains
 * unbounded side Maps (entityToResponseIds, responseIdToEntityIds) for
 * entity-based cache invalidation. A bug in its dispose callback (receives
 * the cached value instead of the cache key due to lru-cache v10's
 * dispose(value, key, reason) signature) means those Maps are never cleaned
 * up on eviction, causing a memory leak proportional to query diversity.
 *
 * Since we don't use entity-based invalidation (no external writes trigger
 * cache purges — mutations are disabled), we replace it with a plain LRU
 * that only does TTL-based expiry.
 */
function createSimpleResponseCache(max: number) {
	const cache = new LRUCache<string, any>({max, allowStale: false})
	return {
		set(id: string, data: unknown, _entities: Iterable<unknown>, ttl: number) {
			cache.set(id, data, {ttl})
		},
		get(id: string) {
			return cache.get(id) ?? null
		},
		invalidate(_entities: Iterable<unknown>) {
			// No-op: we don't use entity-based invalidation.
			// All entries expire via TTL or LRU eviction.
		},
	}
}

// Shared plugins for GraphQL server
const sharedPlugins = [
	usePgClient(pgPool),
	useExecutionCancellation(),
	useResponseCache({
		session: () => null,
		ttl: 10_000, // 10 seconds
		cache: createSimpleResponseCache(1024),
	}),
	useGraphQLInstrumentation(),
]

// GraphQL server without uuidScalarPlugin
export const graphqlServer = createYoga<GraphQLServerContext>({
	schema: postgraphileSchema,
	graphiql: {
		title: "Geo API",
	},
	plugins: sharedPlugins,
})
