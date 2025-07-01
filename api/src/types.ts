import type DataLoader from "dataloader"
import {Data} from "effect"
import type {InputMaybe, RelationFilter, ValueFilter} from "./generated/graphql"

export type OmitStrict<T, K extends keyof T> = Pick<T, Exclude<keyof T, K>>

export interface GraphQLContext {
	spaceId?: InputMaybe<string>
	entitiesLoader: DataLoader<
		string,
		{
			id: string
			createdAt: string
			createdAtBlock: string
			updatedAt: string
			updatedAtBlock: string
		} | null
	>
	entityNamesLoader: DataLoader<string, string | null>
	entityDescriptionsLoader: DataLoader<string, string | null>
	entityValuesLoader: DataLoader<
		{
			entityId: string
			spaceId?: string | null
			filter?: ValueFilter | null
		},
		{
			id: string
			propertyId: string
			entityId: string
			spaceId: string
			value: string
			language: string | null
			unit: string | null
		}[],
		string
	>
	entityRelationsLoader: DataLoader<
		{
			entityId: string
			spaceId?: string | null
			filter?: RelationFilter | null
		},
		{
			id: string
			entityId: string
			spaceId: string
			typeId: string
			fromEntityId: string
			fromSpaceId: string | null
			fromVersionId: string | null
			toEntityId: string
			toSpaceId: string | null
			toVersionId: string | null
			position: string | null
			verified: boolean | null
		}[]
	>
	propertiesLoader: DataLoader<
		string,
		{
			id: string
			type: "Text" | "Number" | "Checkbox" | "Time" | "Point" | "Relation"
		} | null,
		string
	>
	entityBacklinksLoader: DataLoader<
		{
			entityId: string
			spaceId?: string | null
			filter?: RelationFilter | null
		},
		{
			id: string
			entityId: string
			spaceId: string
			typeId: string
			fromEntityId: string
			fromSpaceId: string | null
			fromVersionId: string | null
			toEntityId: string
			toSpaceId: string | null
			toVersionId: string | null
			position: string | null
			verified: boolean | null
		}[]
	>
}

export class BatchingError extends Data.TaggedError("BatchingError")<{
	cause?: unknown
	message?: string
}> {}
