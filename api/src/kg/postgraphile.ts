import PgAggregatesPlugin from "@graphile/pg-aggregates"
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
import {
	getGraphqlPressureSnapshot,
	recordGraphqlAcquireTimeout,
	recordGraphqlSlowAcquire,
	type SaturationSnapshot,
	SLOW_ACQUIRE_MS,
	shouldShedPoolTraffic,
} from "../services/dbSaturation"
import {graphqlQueryFingerprint} from "../services/queryFingerprint"
import {log} from "../services/telemetry"
import {useAdmissionControl} from "./admissionControl"
import AggregatesScopePlugin from "./aggregatesScopePlugin"
import {useCostLogger} from "./costLoggerPlugin"
import EntityComputedTextFilterPlugin from "./entityComputedTextFilterPlugin"
import EntityOrderByRankingScorePlugin from "./entityOrderByRankingScorePlugin"
import EntitySpaceFilterPlugin from "./entitySpaceFilterPlugin"
import {shouldUnmaskError} from "./errorMasking"
import HideProceduresPlugin from "./hideProceduresPlugin"
import {useGraphQLInstrumentation} from "./instrumentationPlugin"
import PaginationCapPlugin, {NoFirstAndLastRule} from "./paginationCapPlugin"
import {useResponseBudget} from "./responseBudgetPlugin"
import {hasCacheableData} from "./responseCachePolicy"
import {useSearchInvocationLogger} from "./searchInvocationLogger"
import {createShedEpisodeTracker} from "./shedEpisodeTracker"
import UserVoteLegacyAccessorPlugin from "./userVoteLegacyAccessorPlugin"
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
	// Client-side fuse, deliberately ABOVE the statement_timeout applied on connect
	// below. Postgres must cancel the query first so the server-side backend is
	// freed; this only reclaims the Node side if that fails. If this is ever the
	// tighter of the two it destroys connections for queries the server is still
	// running, and the failure surfaces as an opaque INTERNAL_SERVER_ERROR with no
	// server-side trace. See api/docs/database-configuration.md.
	query_timeout: parseInt(process.env.PG_QUERY_TIMEOUT_MS || "35000", 10),
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

// Rising-edge log tracker: remembers the last logged saturation episode so
// we emit an onset log on the first shed of each new episode and skip the
// rest (avoiding log storms under load).
const shedEpisodeTracker = createShedEpisodeTracker()

function emitShedSignal(args: {
	poolPressure: SaturationSnapshot
	operationName: string
	queryFingerprint: string
	query: string
}) {
	const {poolPressure, operationName, queryFingerprint, query} = args

	Sentry.metrics.count("graphql.pool_shed", 1, {
		attributes: {
			operation: operationName,
			reasons: poolPressure.reasons.length > 0 ? poolPressure.reasons.join(",") : "saturated_no_signal",
		},
	})

	if (shedEpisodeTracker.shouldLogOnset(poolPressure)) {
		log.warn("GraphQL pool pressure shedding started", {
			operationName,
			queryFingerprint,
			query,
			poolStats: getGraphqlPoolStats(),
			poolPressure,
		})
	}
}

// Push pool utilization as Sentry gauges every 10s for dashboards. Independent
// of the request path, so it continues reporting even when traffic is shed.
// `.unref()` allows process exit when nothing else is holding the event loop.
const POOL_METRICS_INTERVAL_MS = 10_000
if (process.env.SENTRY_DSN) {
	setInterval(() => {
		const stats = getGraphqlPoolStats()
		Sentry.metrics.gauge("graphql.pool.total_connections", stats.totalConnections)
		Sentry.metrics.gauge("graphql.pool.idle_connections", stats.idleConnections)
		Sentry.metrics.gauge("graphql.pool.waiting_count", stats.waitingCount)
	}, POOL_METRICS_INTERVAL_MS).unref()
}

// SQLSTATE codes raised intentionally by Postgres functions to signal user input errors.
// These should surface the raised message to API clients instead of being masked.
// - 22023 invalid_parameter_value (RAISE ... USING ERRCODE = '22023' in pg functions)
const USER_INPUT_SQLSTATES = new Set(["22023"])

function findPgUserInputError(error: unknown): {message: string} | null {
	if (!error || typeof error !== "object") return null
	const code = (error as {code?: unknown}).code
	if (typeof code === "string" && USER_INPUT_SQLSTATES.has(code)) {
		return error as {message: string}
	}
	return null
}

function toUserInputGraphQLError(error: GraphQLError, pgError: {message: string}): GraphQLError {
	return new GraphQLError(pgError.message, {
		nodes: error.nodes,
		source: error.source,
		positions: error.positions,
		path: error.path,
		extensions: {
			code: "BAD_USER_INPUT",
			http: {status: 400},
		},
	})
}

// Apply the server-side query budget ourselves rather than trusting the database's
// configured statement_timeout. api/docs/database-configuration.md documented 10s,
// but the managed instances actually ship `0` (unlimited) on the v2 cluster and
// `1min` on production — so a client-side fuse tuned "just above 10s" was in fact
// the only, and tighter, limit. Setting it here makes the layering true everywhere.
//
// Fires once per new physical connection, not per request. This must be a plain
// SET rather than a startup `options` parameter: DigitalOcean's PgBouncer rejects
// `options: -c statement_timeout=...` at connect time with
// `FATAL: unsupported startup parameter in options`.
const PG_STATEMENT_TIMEOUT_MS = parseInt(process.env.PG_STATEMENT_TIMEOUT_MS || "30000", 10)
pgPool.on("connect", (client) => {
	client.query(`SET statement_timeout = ${PG_STATEMENT_TIMEOUT_MS}`).catch((err) => {
		log.error("Failed to apply statement_timeout to new pool connection", {
			error: String(err),
			statementTimeoutMs: PG_STATEMENT_TIMEOUT_MS,
		})
	})
})

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
	// - EntityComputedTextFilterPlugin rewrites EntityFilter.name / .description filters
	//   (and any other computed text fields backed by entities_<field>() STABLE functions)
	//   as indexed EXISTS subqueries on values, replacing per-row function calls.
	//   Must run AFTER ConnectionFilterPlugin so its build hook can wrap
	//   `connectionFilterRegisterResolver` before the computed-columns sub-plugin uses it.
	// - PaginationCapPlugin clamps first/last/offset on collection fields
	// - UserVoteLegacyAccessorPlugin re-adds the pre-0071 unique-key accessor name
	//   for user_votes, resolving the curation row. Deprecated shim; drop it with
	//   the votes_count upvotes/downvotes shims.
	// - PgAggregatesPlugin adds `groupedAggregates` / `aggregates` to connections, so a faceted
	//   menu can ask "of the rows matching this filter, how many carry each topic / live in each
	//   space" without the client fetching the corpus to count it itself (GEO-2796). Pinned to the
	//   0.1.x line: 0.2.x targets PostGraphile v5 and will not load here.
	appendPlugins: [
		UndashedUuidPlugin,
		AggregatesScopePlugin,
		PgAggregatesPlugin,
		ValueScalarsPlugin,
		ConnectionFilterPlugin,
		SimplifyInflectionPlugin,
		EntitySpaceFilterPlugin,
		EntityComputedTextFilterPlugin,
		ValueOrderByScorePlugin,
		EntityOrderByRankingScorePlugin,
		PaginationCapPlugin,
		HideProceduresPlugin,
		UserVoteLegacyAccessorPlugin,
	],
	disableDefaultMutations: true,
	simpleCollections: "both" as const,
	graphileBuildOptions: {
		// GEO-2796: pg-aggregates opt-in. Off everywhere; aggregatesScopePlugin re-enables
		// relations + entities. Keeps the expensive-aggregate surface deliberate.
		disableAggregatesByDefault: true,
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

// Create PostGraphile schemas. Exported so cost-estimator / query-shape tests
// can run complexity calculations against the real schema instead of a fake
// minimal one — catches schema-level surprises (NonNull wrapping, Connection
// naming, auto-generated field shapes) that mocks don't.
export const postgraphileSchema = await createPostGraphileSchema(pgPool, ["public"], postgraphileOptions)

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
	// Per-request pgClient tracking. WeakMap keyed by the HTTP Request so the
	// onResponse cleanup below can find and release the client even when
	// onExecuteDone never fires (the leak path: useExecutionCancellation aborts
	// execute() and the cleanup callback chain skips usePgClient).
	// See api/docs/gql-pool-leak-investigation.md.
	const requestPgClients = new WeakMap<Request, PoolClient>()

	return {
		async onExecute({extendContext, args}) {
			const operationName = args.operationName || "anonymous"
			const fullQuery = print(args.document)
			// Truncated for Sentry extras (server-side cap ~8 KB).
			const truncatedQuery = fullQuery.slice(0, 5000)
			const queryFingerprint = graphqlQueryFingerprint(fullQuery)
			const acquireStartMs = Date.now()

			// Check pool pressure before attempting checkout. This only runs on
			// cache misses — cached responses skip usePgClient entirely.
			const poolPressure = getGraphqlPressureSnapshot(getGraphqlPoolStats())
			if (shouldShedPoolTraffic(poolPressure)) {
				emitShedSignal({poolPressure, operationName, queryFingerprint, query: fullQuery})
				throw new GraphQLError("Service temporarily unavailable due to database pressure; please retry.", {
					extensions: {
						code: "SERVICE_UNAVAILABLE",
						http: {
							status: 503,
							headers: {
								"Retry-After": "2",
							},
						},
					},
				})
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
					query: fullQuery,
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
						query: truncatedQuery,
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
			if (acquireWaitMs > SLOW_ACQUIRE_MS) {
				// Feed the saturation FSM, not just the log. This is the signal
				// that moves when the event loop is starved rather than the pool
				// being exhausted — see dbSaturation.ts.
				recordGraphqlSlowAcquire()
				log.warn("PostGraphile pool acquire was slow", {
					operationName,
					acquireWaitMs,
					poolStats: getGraphqlPoolStats(),
					poolPressure: getGraphqlPoolPressure(),
				})
			}

			extendContext({pgClient})

			// Track this checkout against the originating request so the
			// onResponse hook below can release it if execute() is aborted.
			const request = (args.contextValue as {request?: Request})?.request
			if (request) {
				requestPgClients.set(request, pgClient)
			}

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
					if (request) requestPgClients.delete(request)
				},
			}
		},

		// Belt-and-braces cleanup. Yoga's onResponse fires after the HTTP
		// response is produced, regardless of whether execute() completed
		// normally, threw, or was aborted by useExecutionCancellation. If the
		// pgClient is still tracked here, onExecuteDone never ran — release it
		// in destroy mode (release(err)) since the in-flight query may still
		// be running on this connection and the client's state is unknown.
		onResponse({request}) {
			const pgClient = requestPgClients.get(request)
			if (pgClient) {
				pgClient.release(new Error("graphql request ended without normal cleanup"))
				requestPgClients.delete(request)
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
		// Deliberately null: the API has no authenticated session, and the cache
		// key already includes the query document and its variables, so callers
		// filtering by different addresses get different entries.
		session: () => null,
		ttl: DEFAULT_TTL_MS,
		// Replaces the plugin default (`!result.errors?.length`), so the error
		// check has to be repeated here. See hasCacheableData for why empty
		// results must not be cached.
		shouldCacheResult: ({result}) => {
			if (result.errors?.length) return false
			if (hasCacheableData(result.data)) return true
			log.debug("Response cache skipped: empty result", {
				reason: "empty_result_not_cacheable",
			})
			return false
		},
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
// Yoga plugin that registers custom GraphQL validation rules.
// Validation runs before execute, so rules like NoFirstAndLastRule surface
// misuse as a structured BAD_USER_INPUT response (with http.status 400) and
// never reach resolvers — or the instrumentation plugin's Sentry capture.
const customValidationRules: Plugin = {
	onValidate({addValidationRule}) {
		addValidationRule(NoFirstAndLastRule)
	},
}

// Response cache is first so cache hits skip pgClient checkout entirely.
// Search-invocation logger runs per request (AST walk only, no DB) and
// warns on every `search` / `searchConnection` invocation so we can
// observe usage + caller IP without adding a separate metrics pipeline.
const sharedPlugins = [
	...(responseCachePlugin ? [responseCachePlugin] : []),
	customValidationRules,
	useCostLogger(),
	// Must follow useCostLogger — it reads the score that plugin stashes on the
	// request context. Must precede usePgClient so a refused operation never
	// checks out a connection or touches the database.
	useAdmissionControl(),
	// Must follow the response cache and precede instrumentation. The cache
	// injects (and later strips) its own metadata fields, so measuring ahead of
	// it would count bytes the client never receives; instrumentation reuses the
	// measurement this plugin leaves on the request context instead of walking
	// the response a second time. See useResponseBudget for the full ordering
	// argument.
	useResponseBudget(),
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
			if (shouldUnmaskError(error)) {
				return error
			}
			if (error instanceof GraphQLError) {
				const pgError = findPgUserInputError(error.originalError)
				if (pgError) {
					return toUserInputGraphQLError(error, pgError)
				}
			}
			return maskError(error, message, isDev)
		},
	},
	plugins: sharedPlugins,
})
