import {makeExecutableSchema} from "@graphql-tools/schema"
import {file} from "bun"
import {createYoga} from "graphql-yoga"
import type {QueryTypesArgs, Resolvers as GeneratedResolvers} from "./src/generated/graphql"
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
		types: async (_, args) => {
			const result = await Resolvers.types(args)
			return result.map((type) => ({
				...type,
				__spaceId: args.spaceId,
			}))
		},
	},
	Entity: {
		name: async (parent: {id: string}) => {
			return Resolvers.entityName({id: parent.id})
		},
		description: async (parent: {id: string}) => {
			return Resolvers.entityDescription({id: parent.id})
		},
		types: async (parent: {id: string}) => {
			return Resolvers.entityTypes({id: parent.id})
		},
		spaces: async (parent: {id: string}) => {
			return Resolvers.spaces({id: parent.id})
		},
		values: async (parent: {id: string}) => {
			return Resolvers.values({id: parent.id})
		},
		relations: async (parent: {id: string}) => {
			return Resolvers.relations({id: parent.id})
		},
	},
	Type: {
		name: async (parent: {id: string}) => {
			return Resolvers.entityName({id: parent.id})
		},
		description: async (parent: {id: string}) => {
			return Resolvers.entityDescription({id: parent.id})
		},
		entity: async (parent: {id: string}) => {
			return Resolvers.entity({id: parent.id})
		},
		properties: async (parent: {id: string}) => {
			// @ts-expect-error type jankiness. Overwriting for now
			const spaceId = (parent.__spaceId as QueryTypesArgs["spaceId"]) ?? null
			return Resolvers.properties(parent.id, {spaceId: spaceId})
		},
	},
	Value: {
		entity: async (parent: {entityId: string}) => {
			return Resolvers.entity({id: parent.entityId})
		},
		property: async (parent: {propertyId: string}) => {
			return Resolvers.property({id: parent.propertyId})
		},
	},
	Property: {
		entity: async (parent: {id: string}) => {
			return Resolvers.entity({id: parent.id})
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
