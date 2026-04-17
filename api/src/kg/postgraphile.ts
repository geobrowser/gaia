import SimplifyInflectionPlugin from "@graphile-contrib/pg-simplify-inflector"
import {useResponseCache} from "@graphql-yoga/plugin-response-cache"
import * as Sentry from "@sentry/node"
import {GraphQLError, print} from "graphql"
import {createYoga, maskError, type Plugin, useExecutionCancellation} from "graphql-yoga"
import type {PoolClient} from "pg"
import {Pool} from "pg"
import {createPostGraphileSchema} from "postgraphile"
import ConnectionFilterPlugin from "postgraphile-plugin-connection-filter"
import {classifyDbFailure} from "../services/dbFailures"
import {getGraphqlPressureSnapshot, recordGraphqlAcquireTimeout, shouldShedPoolTraffic} from "../services/dbSaturation"
import {graphqlQueryFingerprint} from "../services/queryFingerprint"
import {log} from "../services/telemetry"
import EntitySpaceFilterPlugin from "./entitySpaceFilterPlugin"
import {useGraphQLInstrumentation} from "./instrumentationPlugin"
import PaginationCapPlugin from "./paginationCapPlugin"
import {useSearchInvocationLogger} from "./searchInvocationLogger"
import UndashedUuidPlugin from "./uuidScalarPlugin"
import {createValkeyCache} from "./valkeyCache"
import ValueOrderByScorePlugin from "./valueOrderByScorePlugin"
import ValueScalarsPlugin from "./valueScalarsPlugin"

// Server context passed from HTTP middleware
export type GraphQLServerContext = {
	traceContext?: {
		traceId: string
		spanId: string
		traceFlags: number
	}
	requestId?: string
	setGraphqlOperationName?: (operationName: string) => void
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
	// With 2 replicas at 50 each = 100 GraphQL connections, well under PgBouncer's 900 max_client_conn.
	max: parseInt(process.env.PG_POOL_MAX || "50", 10),
	// Fail fast if no connection available (default is 0 = wait forever, causing hangs)
	connectionTimeoutMillis: parseInt(process.env.PG_CONNECTION_TIMEOUT_MS || "3000", 10),
	// Close idle connections after 30 seconds to free up PgBouncer slots
	idleTimeoutMillis: parseInt(process.env.PG_IDLE_TIMEOUT_MS || "30000", 10),
	// Allow process to exit cleanly when pool is idle (for graceful shutdown)
	allowExitOnIdle: true,
})

export function getGraphqlPoolStats() {
	return {
		totalConnections: pgPool.totalCount,
		idleConnections: pgPool.idleCount,
		waitingCount: pgPool.waitingCount,
		maxConnections: pgPool.options.max ?? 0,
	}
}

export function getGraphqlPoolPressure() {
	return getGraphqlPressureSnapshot(getGraphqlPoolStats())
}

function isUserInputGraphQLError(error: unknown): error is GraphQLError {
	if (!(error instanceof GraphQLError)) {
		return false
	}

	if (error.extensions?.code === "BAD_USER_INPUT") {
		return true
	}

	return error.originalError instanceof GraphQLError && error.originalError.extensions?.code === "BAD_USER_INPUT"
}

// Without this handler, background connection errors (PgBouncer closing idle connections,
// network blips) become unhandled events. The pool doesn't remove the dead connection,
// so subsequent connect() calls can check out a stale client that fails during query execution.
pgPool.on("error", (err) => {
	log.error("PostGraphile pool error", {
		error: String(err),
		failureClass: classifyDbFailure(err),
		poolStats: getGraphqlPoolStats(),
	})
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
	// - PaginationCapPlugin clamps first/last/offset on collection fields
	appendPlugins: [
		UndashedUuidPlugin,
		ValueScalarsPlugin,
		ConnectionFilterPlugin,
		SimplifyInflectionPlugin,
		EntitySpaceFilterPlugin,
		ValueOrderByScorePlugin,
		PaginationCapPlugin,
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
 * We don't use PostGraphile's `withPostGraphileContext` because we don't need
 * its transaction wrapper or JWT/role features (mutations are disabled, all
 * queries are reads). Checking out the client directly is simpler and avoids
 * the unnecessary BEGIN/COMMIT overhead on every request.
 *
 * On completion, we check the result for errors. release(err) tells pg.Pool to
 * destroy the connection instead of returning it to the pool — this prevents
 * stale connections (e.g. from PgBouncer closing them) from being reused.
 */
function usePgClient(pool: Pool): Plugin<{pgClient: PoolClient}> {
	return {
		async onExecute({extendContext, args}) {
			const operationName = args.operationName || "anonymous"
			const fullQuery = print(args.document)
			const query = fullQuery.slice(0, 2000)
			const queryFingerprint = graphqlQueryFingerprint(fullQuery)
			const acquireStartMs = Date.now()

			// Check pool pressure before attempting checkout. This only runs on
			// cache misses — cached responses skip usePgClient entirely.
			const poolPressure = getGraphqlPressureSnapshot(getGraphqlPoolStats())
			if (shouldShedPoolTraffic(poolPressure)) {
				throw new Error("pool_pressure_shed")
			}

			let pgClient: PoolClient
			try {
				pgClient = await pool.connect()
			} catch (error) {
				const err = error instanceof Error ? error : new Error(String(error))
				const poolStats = getGraphqlPoolStats()
				const poolPressure = getGraphqlPressureSnapshot(poolStats)
				const failureClass = classifyDbFailure(err)
				if (failureClass === "pool_connect_timeout") {
					recordGraphqlAcquireTimeout()
				}
				const acquireWaitMs = Date.now() - acquireStartMs
				log.error("PostGraphile pool connect error", {
					error: err.message,
					failureClass,
					operationName,
					queryFingerprint,
					query,
					acquireWaitMs,
					poolStats,
					poolPressure,
				})

				Sentry.captureException(err, {
					tags: {
						"graphql.operation_name": operationName,
						"graphql.query_fingerprint": queryFingerprint,
						"db.failure_class": failureClass,
					},
					extra: {
						queryFingerprint,
						query,
						variables: args.variableValues,
						acquireWaitMs,
						poolStats,
						poolPressure,
						failureClass,
					},
				})

				throw error
			}

			const acquireWaitMs = Date.now() - acquireStartMs
			if (acquireWaitMs > 250) {
				log.warn("PostGraphile pool acquire was slow", {
					operationName,
					acquireWaitMs,
					poolStats: getGraphqlPoolStats(),
					poolPressure: getGraphqlPoolPressure(),
				})
			}

			extendContext({pgClient})

			return {
				onExecuteDone({result}) {
					// If the result has GraphQL errors, the underlying connection may be
					// broken (e.g. PgBouncer closed it). release(err) destroys it so the
					// pool doesn't hand out a dead connection on the next request.
					const errors = "errors" in result ? result.errors : undefined
					if (errors?.length) {
						pgClient.release(errors[0])
					} else {
						pgClient.release()
					}
				},
			}
		},
	}
}

// Response cache — shared across all API pods via Valkey (Redis-compatible).
// Disabled gracefully if VALKEY_URL is not set (no cache, direct DB queries).
// Valkey maxmemory + allkeys-lru eviction prevents unbounded memory growth.
const DEFAULT_TTL_MS = 10_000
// Longer TTL for expensive, rarely-changing queries identified in production logs.
// These queries are identical across all users and produce large responses (2-15 MB).
const LONG_TTL_MS = 60_000
const responseCachePlugin = (() => {
	const valkeyUrl = process.env.VALKEY_URL
	if (!valkeyUrl) {
		log.info("Response cache disabled (VALKEY_URL not set)")
		return null
	}
	log.info("Response cache enabled", {valkeyUrl: valkeyUrl.replace(/\/\/.*@/, "//<redacted>@")})
	return useResponseCache({
		session: () => null,
		ttl: DEFAULT_TTL_MS,
		ttlPerSchemaCoordinate: {
			// All DAO spaces list — 12 MB, called on 5+ pages, near-static
			"Query.spaces": LONG_TTL_MS,
			"Query.spacesConnection": LONG_TTL_MS,
			// Bounties/entity lists — 2-15 MB, same result for all users
			"Query.entities": LONG_TTL_MS,
			"Query.entitiesConnection": LONG_TTL_MS,
			"Query.entitiesOrderedByProperty": LONG_TTL_MS,
		},
		cache: createValkeyCache(valkeyUrl),
	})
})()

// Shared plugins for GraphQL server.
// Response cache is first so cache hits skip pgClient checkout entirely.
// Search-invocation logger runs per request (AST walk only, no DB) and
// warns on every `search` / `searchConnection` invocation so we can
// observe usage + caller IP without adding a separate metrics pipeline.
const sharedPlugins = [
	...(responseCachePlugin ? [responseCachePlugin] : []),
	useGraphQLInstrumentation(),
	useSearchInvocationLogger(),
	usePgClient(pgPool),
	useExecutionCancellation(),
]

// GraphQL server without uuidScalarPlugin
export const graphqlServer = createYoga<GraphQLServerContext>({
	schema: postgraphileSchema,
	graphiql: {
		title: "Geo API",
	},
	maskedErrors: {
		maskError(error, message, isDev) {
			if (isUserInputGraphQLError(error)) {
				return error
			}
			return maskError(error, message, isDev)
		},
	},
	plugins: sharedPlugins,
})
