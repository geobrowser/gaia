import {makeExecutableSchema} from "@graphql-tools/schema"
import {file} from "bun"
import {createYoga} from "graphql-yoga"
import {entities} from "./src/resolvers/root"

const schemaFile = await file("./schema.graphql").text()

const resolvers = {
	Query: {
		entities,
	},
}
// Create executable schema
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
