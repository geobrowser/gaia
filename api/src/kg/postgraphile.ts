import SimplifyInflectionPlugin from "@graphile-contrib/pg-simplify-inflector";
import { useResponseCache } from "@graphql-yoga/plugin-response-cache";
import { createYoga, useExecutionCancellation } from "graphql-yoga";
import { Pool } from "pg";
import {
	createPostGraphileSchema,
	withPostGraphileContext,
} from "postgraphile";
import ConnectionFilterPlugin from "postgraphile-plugin-connection-filter";
import EntitySpaceFilterPlugin from "./entitySpaceFilterPlugin";
import { useGraphQLInstrumentation } from "./instrumentationPlugin";
import UndashedUuidPlugin from "./uuidScalarPlugin";
import ValueScalarsPlugin from "./valueScalarsPlugin";

// Server context passed from HTTP middleware
export type GraphQLServerContext = {
	traceContext?: {
		traceId: string;
		spanId: string;
		traceFlags: number;
	};
};

// Create PostgreSQL pool with explicit configuration to prevent connection exhaustion
// Note: Without PgBouncer, each pool connection = 1 Postgres connection.
// Ensure max * num_replicas < Postgres max_connections (leaving room for admin/migrations).
const pgPool = new Pool({
	connectionString:
		process.env.DATABASE_URL || "postgres://user:pass@localhost/mydb",
	// Pool size - PgBouncer handles multiplexing, so we can be generous here.
	// The real PostgreSQL connection limit is managed by PgBouncer's pool_size.
	// With 2 replicas at 50 each = 100 connections, well under PgBouncer's 200 max_client_conn.
	max: parseInt(process.env.PG_POOL_MAX || "50", 10),
	// Fail fast if no connection available (default is 0 = wait forever, causing hangs)
	connectionTimeoutMillis: parseInt(
		process.env.PG_CONNECTION_TIMEOUT_MS || "3000",
		10,
	),
	// Close idle connections after 30 seconds to free up PgBouncer slots
	idleTimeoutMillis: parseInt(process.env.PG_IDLE_TIMEOUT_MS || "30000", 10),
	// Allow process to exit cleanly when pool is idle (for graceful shutdown)
	allowExitOnIdle: true,
});

// Base PostGraphile options (without uuidScalarPlugin)
const postgraphileOptions = {
	watchPg: true,
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
};

// Create PostGraphile schemas
const postgraphileSchema = await createPostGraphileSchema(
	pgPool,
	["public"],
	postgraphileOptions,
);

// Helper to create context
const createContext = async ({ request }: { request: Request }) => {
	const contextPromise = new Promise((resolve, reject) => {
		withPostGraphileContext(
			{
				pgPool,
			},
			async (postgraphileContext) => {
				resolve({
					request,
					...postgraphileContext,
				});

				// Return a dummy result since withPostGraphileContext expects a result
				// The actual result will be handled by GraphQL execution
				return { data: null };
			},
		).catch(reject); // Propagate connection errors (e.g., pool timeout)
	});

	return await contextPromise;
};

// Shared plugins for GraphQL server
const sharedPlugins = [
	useExecutionCancellation(),
	useResponseCache({
		session: () => null,
		ttl: 10_000, // 10 seconds
	}),
	useGraphQLInstrumentation(),
];

// GraphQL server without uuidScalarPlugin
export const graphqlServer = createYoga<GraphQLServerContext>({
	schema: postgraphileSchema,
	graphiql: {
		title: "Geo API",
	},
	plugins: sharedPlugins,
	context: createContext,
});
