import {makeExecutableSchema} from "@graphql-tools/schema"
import {file} from "bun"
import {createYoga} from "graphql-yoga"
import type {Resolvers} from "./src/generated/graphql"
import {entities} from "./src/resolvers/root"

const schemaFile = await file("./schema.graphql").text()

const resolvers: Resolvers = {
	Query: {
		entities: async (_, args) => {
			return await entities(args)
		},
	},
}

const schema = makeExecutableSchema({
	typeDefs: schemaFile,
	resolvers,
})

const yoga = createYoga({
	schema,
	batching: true,
})

const server = Bun.serve({
	fetch: yoga,
})

console.info(`Server is running on ${new URL(yoga.graphqlEndpoint, `http://${server.hostname}:${server.port}`)}`)
