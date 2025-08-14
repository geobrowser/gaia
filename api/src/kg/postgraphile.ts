import SimplifyInflectionPlugin from "@graphile-contrib/pg-simplify-inflector"
import {useResponseCache} from "@graphql-yoga/plugin-response-cache"
import {createYoga, useExecutionCancellation} from "graphql-yoga"
import {Pool} from "pg"
import {createPostGraphileSchema, withPostGraphileContext} from "postgraphile"
import ConnectionFilterPlugin from "postgraphile-plugin-connection-filter"

// Create PostgreSQL pool
const pgPool = new Pool({
	connectionString: process.env.DATABASE_URL || "postgres://user:pass@localhost/mydb",
})

// PostGraphile options
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

// Create PostGraphile schema
const postgraphileSchema = await createPostGraphileSchema(pgPool, ["public"], postgraphileOptions)

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
	context: async ({request}) => {
		// Create a promise that will resolve with the PostGraphile context
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
	},
})
