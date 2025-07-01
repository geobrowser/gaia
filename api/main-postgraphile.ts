import { SystemIds } from "@graphprotocol/grc-20";
import express from "express";
import { gql, makeExtendSchemaPlugin } from "graphile-utils";
import type { GraphQLResolveInfo } from "graphql";
import type { Client } from "pg";
import { postgraphile } from "postgraphile";

const EntityNameResolverPlugin = makeExtendSchemaPlugin({
	typeDefs: gql`
		extend type Entity {
			name_v2: String
		}
	`,
	resolvers: {
		Entity: {
			name: async (
				entity: any,
				args: any,
				context: { pgClient: Client },
				info: GraphQLResolveInfo,
			) => {
				const { pgClient } = context;

				// Query for the name value using SystemIds.NAME_PROPERTY
				const result = await pgClient.query(
					`SELECT value FROM values WHERE entity_id = $1 AND property_id = $2 LIMIT 1`,
					[entity.id, SystemIds.NAME_PROPERTY],
				);

				return result.rows[0]?.value || null;
			},
		},
	},
});

const middleware = postgraphile(process.env.DATABASE_URL!, "public", {
	appendPlugins: [EntityNameResolverPlugin],
	graphiql: true,
	enhanceGraphiql: true,
});

const app = express();

app.use(middleware);

const server = app.listen(5678, () => {
	const address = server.address();
	if (typeof address !== "string") {
		const href = `http://localhost:${address?.port}/graphiql`;
		console.log(`PostGraphiQL available at ${href} 🚀`);
	} else {
		console.log(`PostGraphile listening on ${address} 🚀`);
	}
});
