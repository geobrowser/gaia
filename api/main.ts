import {makeExecutableSchema} from "@graphql-tools/schema"
import {file} from "bun"
import {createYoga} from "graphql-yoga"
import type {Resolvers as GeneratedResolvers} from "./src/generated/graphql"
import * as Resolvers from "./src/resolvers/root"

const schemaFile = await file("./schema.graphql").text()

const resolvers: GeneratedResolvers = {
	Query: {
		entities: async (_, args) => {
			return await Resolvers.entities(args)
		},
		entity: async (_, args) => {
			return await Resolvers.entity(args)
		},
	},
	Entity: {
		// @TODO: Properties
		// @TODO: Relations
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
