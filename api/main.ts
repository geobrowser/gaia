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
		name: async (parent: {id: string}) => {
			return Resolvers.entityName({id: parent.id})
		},
		properties: async (parent: {id: string}) => {
			return Resolvers.properties({id: parent.id})
		},
		relations: async (parent: {id: string}) => {
			return Resolvers.relations({id: parent.id})
		},
	},
	Property: {
		entity: async (parent: {entityId: string}) => {
			return Resolvers.entity({id: parent.entityId})
		},

		attribute: async (parent: {attributeId: string}) => {
			return Resolvers.entity({id: parent.attributeId})
		},
	},
	Relation: {
		from: async (parent: {fromId: string}) => {
			return Resolvers.entity({id: parent.fromId})
		},
		to: async (parent: {toId: string}) => {
			return Resolvers.entity({id: parent.toId})
		},
		type: async (parent: {typeId: string}) => {
			return Resolvers.entity({id: parent.typeId})
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
