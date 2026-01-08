import SimplifyInflectionPlugin from "@graphile-contrib/pg-simplify-inflector"
import { useResponseCache } from "@graphql-yoga/plugin-response-cache"
import { createYoga, useExecutionCancellation } from "graphql-yoga"
import { Pool } from "pg"
import { createPostGraphileSchema, withPostGraphileContext } from "postgraphile"
import ConnectionFilterPlugin from "postgraphile-plugin-connection-filter"
import UndashedUuidPlugin from "./uuidScalarPlugin"

// Create PostgreSQL pool
const pgPool = new Pool({
	connectionString: process.env.DATABASE_URL || "postgres://user:pass@localhost/mydb",
})

// Base PostGraphile options (without uuidScalarPlugin)
const postgraphileOptions = {
	watchPg: true,
	graphiql: true,
	enhanceGraphiql: true,
	dynamicJson: true,
	setofFunctionsContainNulls: false,
	ignoreRBAC: false,
	appendPlugins: [ConnectionFilterPlugin, SimplifyInflectionPlugin],
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

// PostGraphile options with uuidScalarPlugin for v2
const postgraphileOptionsV2 = {
	...postgraphileOptions,
	// NOTE: Plugin order is intentional:
	// - UndashedUuidPlugin patches the UUID scalar first so that all subsequent
	//   plugins (including ConnectionFilterPlugin) build their types against the
	//   undashed UUID behavior. This has been verified to work with both dashed
	//   and undashed UUID inputs in filters.
	appendPlugins: [UndashedUuidPlugin, ConnectionFilterPlugin, SimplifyInflectionPlugin],
}

// Create PostGraphile schemas
const postgraphileSchema = await createPostGraphileSchema(pgPool, ["public"], postgraphileOptions)
const postgraphileSchemaV2 = await createPostGraphileSchema(pgPool, ["public"], postgraphileOptionsV2)

// Helper to create context
const createContext = async ({request}: {request: Request}) => {
	const contextPromise = new Promise((resolve) => {
		withPostGraphileContext(
			{
				pgPool,
			},
			async (postgraphileContext) => {
				resolve({
					request,
					...postgraphileContext,
				})

				// Return a dummy result since withPostGraphileContext expects a result
				// The actual result will be handled by GraphQL execution
				return {data: null}
			},
		)
	})

	return await contextPromise
}

// GraphQL server without uuidScalarPlugin
export const graphqlServer = createYoga({
	schema: postgraphileSchema,
	graphiql: {
		title: "Geo API",
	},
	plugins: [
		useExecutionCancellation(),
		useResponseCache({
			session: () => null,
			ttl: 10_000, // 10 seconds
		}),
	],
	context: createContext,
})

// GraphQL server with uuidScalarPlugin (v2)
export const graphqlServerV2 = createYoga({
	schema: postgraphileSchemaV2,
	graphiql: {
		title: "Geo API v2",
	},
	plugins: [
		useExecutionCancellation(),
		useResponseCache({
			session: () => null,
			ttl: 10_000, // 10 seconds
		}),
	],
	context: createContext,
})
