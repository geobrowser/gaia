import {makeExecutableSchema} from "@graphql-tools/schema"
import {file} from "bun"
import {Effect, Layer} from "effect"
import {createYoga} from "graphql-yoga"
import type {
	EntityRelationsArgs,
	EntityValuesArgs,
	Resolvers as GeneratedResolvers,
	InputMaybe,
	QuerySearchArgs,
	QuerySpaceArgs,
	QuerySpacesArgs,
} from "../generated/graphql"
import {Environment, make as makeEnvironment} from "../services/environment"
import {make as makeStorage, Storage} from "../services/storage/storage"
import * as MembershipResolvers from "./resolvers/membership"
import * as Resolvers from "./resolvers/root"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

interface GraphQLContext {
	spaceId?: InputMaybe<string>
}

const schemaFile = await file("./schema.graphql").text()

const resolvers: GeneratedResolvers = {
	Query: {
		meta: async () => {
			return await Resolvers.meta()
		},
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
		property: async (_, args) => {
			return await Resolvers.property({id: args.id})
		},
		spaces: async (_, args: QuerySpacesArgs) => {
			return await Resolvers.spaces(args)
		},
		space: async (_, args: QuerySpaceArgs) => {
			return await Resolvers.space(args.id)
		},
		relation: async (_, args) => {
			return await Resolvers.relation({id: args.id})
		},
		relations: async (_, args) => {
			return await Resolvers.relations(args)
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
			return Resolvers.entitySpaces({id: parent.id})
		},
		values: async (parent: {id: string}, args: EntityValuesArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.values({id: parent.id, spaceId})
		},
		relations: async (parent: {id: string}, args: EntityRelationsArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.entityRelations({id: parent.id, spaceId})
		},
		backlinks: async (parent: {id: string}, args: EntityRelationsArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.entityBacklinks({id: parent.id, spaceId})
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
			return Resolvers.propertiesForType(parent.id, {
				spaceId: context.spaceId,
			})
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
	Space: {
		entity: async (parent: {id: string}) => {
			return Resolvers.spaceEntity(parent.id)
		},
		editors: async (parent: {id: string}) => {
			return await Effect.runPromise(MembershipResolvers.getEditors({spaceId: parent.id}).pipe(provideDeps))
		},
		members: async (parent: {id: string}) => {
			return await Effect.runPromise(MembershipResolvers.getMembers({spaceId: parent.id}).pipe(provideDeps))
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
