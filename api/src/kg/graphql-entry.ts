import {makeExecutableSchema} from "@graphql-tools/schema"
import {file} from "bun"
import {Effect, Layer} from "effect"
import {createYoga} from "graphql-yoga"
import type {
	DataType,
	EntityRelationsArgs,
	EntityValuesArgs,
	Resolvers as GeneratedResolvers,
	InputMaybe,
	QuerySearchArgs,
	QuerySpaceArgs,
	QuerySpacesArgs,
} from "../generated/graphql"
import {Environment, make as makeEnvironment} from "../services/environment"
import {Batching, make as makeBatching} from "../services/storage/dataloaders"
import {make as makeStorage, Storage} from "../services/storage/storage"
import * as MembershipResolvers from "./resolvers/membership"
import * as Resolvers from "./resolvers/root"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const BatchingLayer = Layer.effect(Batching, makeBatching).pipe(Layer.provide(StorageLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer, BatchingLayer)
const provideDeps = Effect.provide(layers)

interface GraphQLContext {
	spaceId?: InputMaybe<string>
}

const schemaFile = await file("./schema.graphql").text()

const resolvers: GeneratedResolvers = {
	Query: {
		meta: async () => {
			return Resolvers.meta()
		},
		entities: async (_, args, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return Resolvers.entities(args)
		},
		entity: async (_, args, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return Resolvers.entity(args)
		},
		types: async (_, args, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return Resolvers.types(args)
		},
		search: (_, args: QuerySearchArgs, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return Resolvers.search(args)
		},
		properties: (_, args) => {
			return Resolvers.properties(args)
		},
		property: (_, args) => {
			return Resolvers.property({id: args.id})
		},
		spaces: (_, args: QuerySpacesArgs) => {
			return Resolvers.spaces(args)
		},
		space: (_, args: QuerySpaceArgs) => {
			return Resolvers.space(args.id)
		},
		relation: (_, args) => {
			return Resolvers.relation({id: args.id})
		},
		relations: (_, args) => {
			return Resolvers.relations(args)
		},
	},
	Entity: {
		name: (parent: {id: string}) => {
			return Resolvers.entityName({id: parent.id})
		},
		description: (parent: {id: string}) => {
			return Resolvers.entityDescription({id: parent.id})
		},
		blocks: (parent: {id: string}) => {
			return Resolvers.blocks({id: parent.id})
		},
		types: (parent: {id: string}) => {
			return Resolvers.entityTypes({id: parent.id})
		},
		spaces: (parent: {id: string}) => {
			return Resolvers.entitySpaces({id: parent.id})
		},
		values: (parent: {id: string}, args: EntityValuesArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.values({id: parent.id, spaceId})
		},
		relations: (parent: {id: string}, args: EntityRelationsArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.entityRelations({
				id: parent.id,
				spaceId,
			})
		},
		backlinks: (parent: {id: string}, args: EntityRelationsArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.entityBacklinks({
				id: parent.id,
				spaceId: spaceId,
			})
		},
	},
	Type: {
		name: (parent: {id: string}) => {
			return Resolvers.entityName({id: parent.id})
		},
		description: (parent: {id: string}) => {
			return Resolvers.entityDescription({id: parent.id})
		},
		entity: (parent: {id: string}) => {
			return Resolvers.entity({id: parent.id})
		},
		properties: (parent: {id: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.propertiesForType(parent.id, {
				spaceId: context.spaceId,
			})
		},
	},
	Value: {
		entity: (parent: {entityId: string}) => {
			return Resolvers.entity({id: parent.entityId})
		},
		property: (parent: {propertyId: string}) => {
			return Resolvers.property({
				id: parent.propertyId,
			})
		},
	},
	Property: {
		entity: (parent: {id: string}) => {
			return Resolvers.entity({id: parent.id})
		},
		relationValueTypes: (parent: {id: string; dataType: DataType}) => {
			return Resolvers.propertyRelationValueTypes({
				id: parent.id,
				dataType: parent.dataType,
			})
		},
		renderableType: (parent: {id: string}) => {
			return Resolvers.propertyRenderableType({id: parent.id})
		},
	},
	Relation: {
		from: (parent: {fromId: string}) => {
			return Resolvers.entity({id: parent.fromId})
		},
		to: (parent: {toId: string}) => {
			return Resolvers.entity({id: parent.toId})
		},
		type: (parent: {typeId: string}) => {
			return Resolvers.property({id: parent.typeId})
		},
		relationEntity: async (parent: {entityId: string}) => {
			return Resolvers.entity({id: parent.entityId})
		},
	},
	Space: {
		entity: (parent: {id: string}) => {
			return Resolvers.spaceEntity(parent.id)
		},
		editors: (parent: {id: string}) => {
			return Effect.runPromise(MembershipResolvers.getEditors({spaceId: parent.id}).pipe(provideDeps))
		},
		members: (parent: {id: string}) => {
			return Effect.runPromise(MembershipResolvers.getMembers({spaceId: parent.id}).pipe(provideDeps))
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
	graphqlEndpoint: "/graphql",
	fetchAPI: {Response, Request},
})
