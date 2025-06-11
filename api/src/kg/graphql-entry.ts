import {makeExecutableSchema} from "@graphql-tools/schema"
import {file} from "bun"
import {createYoga} from "graphql-yoga"
import type {
	EntityRelationsArgs,
	EntityValuesArgs,
	Resolvers as GeneratedResolvers,
	InputMaybe,
	QuerySearchArgs,
} from "../generated/graphql"
import * as Resolvers from "./resolvers/root"

interface GraphQLContext {
	spaceId?: InputMaybe<string>
}

const schemaFile = await file("./schema.graphql").text()

const resolvers: GeneratedResolvers = {
	Query: {
		entities: async (_, args, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return await Resolvers.entities(args)
		},
		entity: async (_, args, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return await Resolvers.entity(args)
		},
		types: async (_, args, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return await Resolvers.types(args)
		},
		search: async (_, args: QuerySearchArgs, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return await Resolvers.search(args)
		},
		properties: async (_, args) => {
			return await Resolvers.properties(args)
		},
	},
	Entity: {
		name: async (parent: {id: string}) => {
			return Resolvers.entityName({id: parent.id})
		},
		description: async (parent: {id: string}) => {
			return Resolvers.entityDescription({id: parent.id})
		},
		blocks: async (parent: {id: string}) => {
			return Resolvers.blocks({id: parent.id})
		},
		types: async (parent: {id: string}) => {
			return Resolvers.entityTypes({id: parent.id})
		},
		spaces: async (parent: {id: string}) => {
			return Resolvers.spaces({id: parent.id})
		},
		values: async (parent: {id: string}, args: EntityValuesArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.values({id: parent.id, spaceId})
		},
		relations: async (parent: {id: string}, args: EntityRelationsArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.relations({id: parent.id, spaceId})
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
		properties: async (parent: {id: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.propertiesForType(parent.id, {spaceId: context.spaceId})
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
		relationValueTypes: async (parent: {id: string}) => {
			return Resolvers.propertyRelationValueTypes({id: parent.id})
		},
		renderableType: async (parent: {id: string}) => {
			return Resolvers.propertyRenderableType({id: parent.id})
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
			return Resolvers.property({id: parent.typeId})
		},
		relationEntity: async (parent: {entityId: string}) => {
			return Resolvers.entity({id: parent.entityId})
		},
	},
}

const schema = makeExecutableSchema({
	typeDefs: schemaFile,
	resolvers,
})

export const graphqlServer = createYoga({
	schema,
	batching: true,
	context: (): GraphQLContext => ({}),
	graphqlEndpoint: "/graphql",
	fetchAPI: {Response, Request},
})
