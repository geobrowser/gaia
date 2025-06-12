import { GraphQLResolveInfo } from 'graphql';
import { DbEntity } from '../services/storage/schema';
export type Maybe<T> = T | null;
export type InputMaybe<T> = Maybe<T>;
export type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };
export type MakeOptional<T, K extends keyof T> = Omit<T, K> & { [SubKey in K]?: Maybe<T[SubKey]> };
export type MakeMaybe<T, K extends keyof T> = Omit<T, K> & { [SubKey in K]: Maybe<T[SubKey]> };
export type MakeEmpty<T extends { [key: string]: unknown }, K extends keyof T> = { [_ in K]?: never };
export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };
export type Omit<T, K extends keyof T> = Pick<T, Exclude<keyof T, K>>;
export type RequireFields<T, K extends keyof T> = Omit<T, K> & { [P in K]-?: NonNullable<T[P]> };
/** All built-in and custom scalars, mapped to their actual values */
export type Scalars = {
  ID: { input: string; output: string; }
  String: { input: string; output: string; }
  Boolean: { input: boolean; output: boolean; }
  Int: { input: number; output: number; }
  Float: { input: number; output: number; }
};

export type Block = {
  __typename?: 'Block';
  dataSourceType?: Maybe<DataSourceType>;
  entity?: Maybe<Entity>;
  id: Scalars['ID']['output'];
  type: BlockType;
  value?: Maybe<Scalars['String']['output']>;
};

export enum BlockType {
  Data = 'DATA',
  Image = 'IMAGE',
  Text = 'TEXT'
}

export type CheckboxFilter = {
  exists?: InputMaybe<Scalars['Boolean']['input']>;
  is?: InputMaybe<Scalars['Boolean']['input']>;
};

export enum DataSourceType {
  Collection = 'COLLECTION',
  Geo = 'GEO',
  Query = 'QUERY'
}

export enum DataType {
  Checkbox = 'CHECKBOX',
  Number = 'NUMBER',
  Point = 'POINT',
  Relation = 'RELATION',
  Text = 'TEXT',
  Time = 'TIME'
}

export type Entity = {
  __typename?: 'Entity';
  blocks: Array<Maybe<Block>>;
  createdAt: Scalars['String']['output'];
  createdAtBlock: Scalars['String']['output'];
  description?: Maybe<Scalars['String']['output']>;
  id: Scalars['ID']['output'];
  name?: Maybe<Scalars['String']['output']>;
  relations: Array<Maybe<Relation>>;
  spaces: Array<Maybe<Scalars['String']['output']>>;
  types: Array<Maybe<Entity>>;
  updatedAt: Scalars['String']['output'];
  updatedAtBlock: Scalars['String']['output'];
  values: Array<Maybe<Value>>;
};


export type EntityRelationsArgs = {
  filter?: InputMaybe<RelationFilter>;
  spaceId?: InputMaybe<Scalars['String']['input']>;
};


export type EntityValuesArgs = {
  filter?: InputMaybe<ValueFilter>;
  spaceId?: InputMaybe<Scalars['String']['input']>;
};

export type EntityFilter = {
  NOT?: InputMaybe<EntityFilter>;
  OR?: InputMaybe<Array<EntityFilter>>;
  fromRelation?: InputMaybe<RelationFilter>;
  id?: InputMaybe<IdFilter>;
  toRelation?: InputMaybe<RelationFilter>;
  value?: InputMaybe<ValueFilter>;
};

export type IdFilter = {
  in?: InputMaybe<Array<Scalars['String']['input']>>;
};

export type NumberFilter = {
  NOT?: InputMaybe<NumberFilter>;
  exists?: InputMaybe<Scalars['Boolean']['input']>;
  greaterThan?: InputMaybe<Scalars['Float']['input']>;
  greaterThanOrEqual?: InputMaybe<Scalars['Float']['input']>;
  is?: InputMaybe<Scalars['Float']['input']>;
  lessThan?: InputMaybe<Scalars['Float']['input']>;
  lessThanOrEqual?: InputMaybe<Scalars['Float']['input']>;
};

export type PointFilter = {
  exists?: InputMaybe<Scalars['Boolean']['input']>;
  is?: InputMaybe<Array<InputMaybe<Scalars['Float']['input']>>>;
};

export type Property = {
  __typename?: 'Property';
  dataType: DataType;
  entity?: Maybe<Entity>;
  id: Scalars['ID']['output'];
  relationValueTypes?: Maybe<Array<Maybe<Type>>>;
  renderableType?: Maybe<RenderableType>;
};

export type PropertyFilter = {
  dataType?: InputMaybe<DataType>;
};

export type Query = {
  __typename?: 'Query';
  entities: Array<Maybe<Entity>>;
  entity?: Maybe<Entity>;
  properties: Array<Maybe<Property>>;
  search: Array<Maybe<Entity>>;
  spaces: Array<Maybe<Space>>;
  types: Array<Maybe<Type>>;
};


export type QueryEntitiesArgs = {
  filter?: InputMaybe<EntityFilter>;
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  spaceId?: InputMaybe<Scalars['String']['input']>;
};


export type QueryEntityArgs = {
  id: Scalars['String']['input'];
  spaceId?: InputMaybe<Scalars['String']['input']>;
};


export type QueryPropertiesArgs = {
  filter?: InputMaybe<PropertyFilter>;
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
};


export type QuerySearchArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  query: Scalars['String']['input'];
  spaceId?: InputMaybe<Scalars['String']['input']>;
  threshold?: InputMaybe<Scalars['Float']['input']>;
};


export type QueryTypesArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  spaceId?: InputMaybe<Scalars['String']['input']>;
};

export type Relation = {
  __typename?: 'Relation';
  entityId: Scalars['ID']['output'];
  from?: Maybe<Entity>;
  fromId: Scalars['String']['output'];
  id: Scalars['ID']['output'];
  position?: Maybe<Scalars['String']['output']>;
  relationEntity?: Maybe<Entity>;
  spaceId: Scalars['String']['output'];
  to?: Maybe<Entity>;
  toId: Scalars['String']['output'];
  toSpaceId?: Maybe<Scalars['String']['output']>;
  type?: Maybe<Property>;
  typeId: Scalars['String']['output'];
  verified?: Maybe<Scalars['Boolean']['output']>;
};

export type RelationFilter = {
  fromEntityId?: InputMaybe<Scalars['String']['input']>;
  toEntityId?: InputMaybe<Scalars['String']['input']>;
  typeId?: InputMaybe<Scalars['String']['input']>;
};

export enum RenderableType {
  Image = 'IMAGE',
  Url = 'URL'
}

export type Space = {
  __typename?: 'Space';
  daoAddress: Scalars['String']['output'];
  id: Scalars['ID']['output'];
  mainVotingAddress?: Maybe<Scalars['String']['output']>;
  membershipAddress?: Maybe<Scalars['String']['output']>;
  personalAddress?: Maybe<Scalars['String']['output']>;
  spaceAddress: Scalars['String']['output'];
  type: SpaceType;
};

export enum SpaceType {
  Personal = 'PERSONAL',
  Public = 'PUBLIC'
}

export type TextFilter = {
  NOT?: InputMaybe<TextFilter>;
  contains?: InputMaybe<Scalars['String']['input']>;
  endsWith?: InputMaybe<Scalars['String']['input']>;
  exists?: InputMaybe<Scalars['Boolean']['input']>;
  is?: InputMaybe<Scalars['String']['input']>;
  startsWith?: InputMaybe<Scalars['String']['input']>;
};

export type Type = {
  __typename?: 'Type';
  description?: Maybe<Scalars['String']['output']>;
  entity?: Maybe<Entity>;
  id: Scalars['ID']['output'];
  name?: Maybe<Scalars['String']['output']>;
  properties?: Maybe<Array<Maybe<Property>>>;
};

export type Value = {
  __typename?: 'Value';
  entity?: Maybe<Entity>;
  entityId: Scalars['String']['output'];
  format?: Maybe<Scalars['String']['output']>;
  id: Scalars['ID']['output'];
  language?: Maybe<Scalars['String']['output']>;
  property?: Maybe<Property>;
  propertyId: Scalars['String']['output'];
  spaceId: Scalars['String']['output'];
  timezone?: Maybe<Scalars['String']['output']>;
  unit?: Maybe<Scalars['String']['output']>;
  value?: Maybe<Scalars['String']['output']>;
};

export type ValueFilter = {
  checkbox?: InputMaybe<CheckboxFilter>;
  number?: InputMaybe<NumberFilter>;
  point?: InputMaybe<PointFilter>;
  property: Scalars['String']['input'];
  text?: InputMaybe<TextFilter>;
};

export type WithIndex<TObject> = TObject & Record<string, any>;
export type ResolversObject<TObject> = WithIndex<TObject>;

export type ResolverTypeWrapper<T> = Promise<T> | T;


export type ResolverWithResolve<TResult, TParent, TContext, TArgs> = {
  resolve: ResolverFn<TResult, TParent, TContext, TArgs>;
};
export type Resolver<TResult, TParent = {}, TContext = {}, TArgs = {}> = ResolverFn<TResult, TParent, TContext, TArgs> | ResolverWithResolve<TResult, TParent, TContext, TArgs>;

export type ResolverFn<TResult, TParent, TContext, TArgs> = (
  parent: TParent,
  args: TArgs,
  context: TContext,
  info: GraphQLResolveInfo
) => Promise<TResult> | TResult;

export type SubscriptionSubscribeFn<TResult, TParent, TContext, TArgs> = (
  parent: TParent,
  args: TArgs,
  context: TContext,
  info: GraphQLResolveInfo
) => AsyncIterable<TResult> | Promise<AsyncIterable<TResult>>;

export type SubscriptionResolveFn<TResult, TParent, TContext, TArgs> = (
  parent: TParent,
  args: TArgs,
  context: TContext,
  info: GraphQLResolveInfo
) => TResult | Promise<TResult>;

export interface SubscriptionSubscriberObject<TResult, TKey extends string, TParent, TContext, TArgs> {
  subscribe: SubscriptionSubscribeFn<{ [key in TKey]: TResult }, TParent, TContext, TArgs>;
  resolve?: SubscriptionResolveFn<TResult, { [key in TKey]: TResult }, TContext, TArgs>;
}

export interface SubscriptionResolverObject<TResult, TParent, TContext, TArgs> {
  subscribe: SubscriptionSubscribeFn<any, TParent, TContext, TArgs>;
  resolve: SubscriptionResolveFn<TResult, any, TContext, TArgs>;
}

export type SubscriptionObject<TResult, TKey extends string, TParent, TContext, TArgs> =
  | SubscriptionSubscriberObject<TResult, TKey, TParent, TContext, TArgs>
  | SubscriptionResolverObject<TResult, TParent, TContext, TArgs>;

export type SubscriptionResolver<TResult, TKey extends string, TParent = {}, TContext = {}, TArgs = {}> =
  | ((...args: any[]) => SubscriptionObject<TResult, TKey, TParent, TContext, TArgs>)
  | SubscriptionObject<TResult, TKey, TParent, TContext, TArgs>;

export type TypeResolveFn<TTypes, TParent = {}, TContext = {}> = (
  parent: TParent,
  context: TContext,
  info: GraphQLResolveInfo
) => Maybe<TTypes> | Promise<Maybe<TTypes>>;

export type IsTypeOfResolverFn<T = {}, TContext = {}> = (obj: T, context: TContext, info: GraphQLResolveInfo) => boolean | Promise<boolean>;

export type NextResolverFn<T> = () => Promise<T>;

export type DirectiveResolverFn<TResult = {}, TParent = {}, TContext = {}, TArgs = {}> = (
  next: NextResolverFn<TResult>,
  parent: TParent,
  args: TArgs,
  context: TContext,
  info: GraphQLResolveInfo
) => TResult | Promise<TResult>;



/** Mapping between all available schema types and the resolvers types */
export type ResolversTypes = ResolversObject<{
  Block: ResolverTypeWrapper<Omit<Block, 'entity'> & { entity?: Maybe<ResolversTypes['Entity']> }>;
  BlockType: BlockType;
  Boolean: ResolverTypeWrapper<Scalars['Boolean']['output']>;
  CheckboxFilter: CheckboxFilter;
  DataSourceType: DataSourceType;
  DataType: DataType;
  Entity: ResolverTypeWrapper<DbEntity>;
  EntityFilter: EntityFilter;
  Float: ResolverTypeWrapper<Scalars['Float']['output']>;
  ID: ResolverTypeWrapper<Scalars['ID']['output']>;
  IdFilter: IdFilter;
  Int: ResolverTypeWrapper<Scalars['Int']['output']>;
  NumberFilter: NumberFilter;
  PointFilter: PointFilter;
  Property: ResolverTypeWrapper<Omit<Property, 'entity' | 'relationValueTypes'> & { entity?: Maybe<ResolversTypes['Entity']>, relationValueTypes?: Maybe<Array<Maybe<ResolversTypes['Type']>>> }>;
  PropertyFilter: PropertyFilter;
  Query: ResolverTypeWrapper<{}>;
  Relation: ResolverTypeWrapper<Omit<Relation, 'from' | 'relationEntity' | 'to' | 'type'> & { from?: Maybe<ResolversTypes['Entity']>, relationEntity?: Maybe<ResolversTypes['Entity']>, to?: Maybe<ResolversTypes['Entity']>, type?: Maybe<ResolversTypes['Property']> }>;
  RelationFilter: RelationFilter;
  RenderableType: RenderableType;
  Space: ResolverTypeWrapper<Space>;
  SpaceType: SpaceType;
  String: ResolverTypeWrapper<Scalars['String']['output']>;
  TextFilter: TextFilter;
  Type: ResolverTypeWrapper<Omit<Type, 'entity' | 'properties'> & { entity?: Maybe<ResolversTypes['Entity']>, properties?: Maybe<Array<Maybe<ResolversTypes['Property']>>> }>;
  Value: ResolverTypeWrapper<Omit<Value, 'entity' | 'property'> & { entity?: Maybe<ResolversTypes['Entity']>, property?: Maybe<ResolversTypes['Property']> }>;
  ValueFilter: ValueFilter;
}>;

/** Mapping between all available schema types and the resolvers parents */
export type ResolversParentTypes = ResolversObject<{
  Block: Omit<Block, 'entity'> & { entity?: Maybe<ResolversParentTypes['Entity']> };
  Boolean: Scalars['Boolean']['output'];
  CheckboxFilter: CheckboxFilter;
  Entity: DbEntity;
  EntityFilter: EntityFilter;
  Float: Scalars['Float']['output'];
  ID: Scalars['ID']['output'];
  IdFilter: IdFilter;
  Int: Scalars['Int']['output'];
  NumberFilter: NumberFilter;
  PointFilter: PointFilter;
  Property: Omit<Property, 'entity' | 'relationValueTypes'> & { entity?: Maybe<ResolversParentTypes['Entity']>, relationValueTypes?: Maybe<Array<Maybe<ResolversParentTypes['Type']>>> };
  PropertyFilter: PropertyFilter;
  Query: {};
  Relation: Omit<Relation, 'from' | 'relationEntity' | 'to' | 'type'> & { from?: Maybe<ResolversParentTypes['Entity']>, relationEntity?: Maybe<ResolversParentTypes['Entity']>, to?: Maybe<ResolversParentTypes['Entity']>, type?: Maybe<ResolversParentTypes['Property']> };
  RelationFilter: RelationFilter;
  Space: Space;
  String: Scalars['String']['output'];
  TextFilter: TextFilter;
  Type: Omit<Type, 'entity' | 'properties'> & { entity?: Maybe<ResolversParentTypes['Entity']>, properties?: Maybe<Array<Maybe<ResolversParentTypes['Property']>>> };
  Value: Omit<Value, 'entity' | 'property'> & { entity?: Maybe<ResolversParentTypes['Entity']>, property?: Maybe<ResolversParentTypes['Property']> };
  ValueFilter: ValueFilter;
}>;

export type BlockResolvers<ContextType = any, ParentType extends ResolversParentTypes['Block'] = ResolversParentTypes['Block']> = ResolversObject<{
  dataSourceType?: Resolver<Maybe<ResolversTypes['DataSourceType']>, ParentType, ContextType>;
  entity?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  type?: Resolver<ResolversTypes['BlockType'], ParentType, ContextType>;
  value?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type EntityResolvers<ContextType = any, ParentType extends ResolversParentTypes['Entity'] = ResolversParentTypes['Entity']> = ResolversObject<{
  blocks?: Resolver<Array<Maybe<ResolversTypes['Block']>>, ParentType, ContextType>;
  createdAt?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  createdAtBlock?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  description?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  name?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  relations?: Resolver<Array<Maybe<ResolversTypes['Relation']>>, ParentType, ContextType, Partial<EntityRelationsArgs>>;
  spaces?: Resolver<Array<Maybe<ResolversTypes['String']>>, ParentType, ContextType>;
  types?: Resolver<Array<Maybe<ResolversTypes['Entity']>>, ParentType, ContextType>;
  updatedAt?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  updatedAtBlock?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  values?: Resolver<Array<Maybe<ResolversTypes['Value']>>, ParentType, ContextType, Partial<EntityValuesArgs>>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type PropertyResolvers<ContextType = any, ParentType extends ResolversParentTypes['Property'] = ResolversParentTypes['Property']> = ResolversObject<{
  dataType?: Resolver<ResolversTypes['DataType'], ParentType, ContextType>;
  entity?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  relationValueTypes?: Resolver<Maybe<Array<Maybe<ResolversTypes['Type']>>>, ParentType, ContextType>;
  renderableType?: Resolver<Maybe<ResolversTypes['RenderableType']>, ParentType, ContextType>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type QueryResolvers<ContextType = any, ParentType extends ResolversParentTypes['Query'] = ResolversParentTypes['Query']> = ResolversObject<{
  entities?: Resolver<Array<Maybe<ResolversTypes['Entity']>>, ParentType, ContextType, RequireFields<QueryEntitiesArgs, 'limit' | 'offset'>>;
  entity?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType, RequireFields<QueryEntityArgs, 'id'>>;
  properties?: Resolver<Array<Maybe<ResolversTypes['Property']>>, ParentType, ContextType, RequireFields<QueryPropertiesArgs, 'limit' | 'offset'>>;
  search?: Resolver<Array<Maybe<ResolversTypes['Entity']>>, ParentType, ContextType, RequireFields<QuerySearchArgs, 'limit' | 'offset' | 'query' | 'threshold'>>;
  spaces?: Resolver<Array<Maybe<ResolversTypes['Space']>>, ParentType, ContextType>;
  types?: Resolver<Array<Maybe<ResolversTypes['Type']>>, ParentType, ContextType, RequireFields<QueryTypesArgs, 'limit' | 'offset'>>;
}>;

export type RelationResolvers<ContextType = any, ParentType extends ResolversParentTypes['Relation'] = ResolversParentTypes['Relation']> = ResolversObject<{
  entityId?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  from?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  fromId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  position?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  relationEntity?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  spaceId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  to?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  toId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  toSpaceId?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  type?: Resolver<Maybe<ResolversTypes['Property']>, ParentType, ContextType>;
  typeId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  verified?: Resolver<Maybe<ResolversTypes['Boolean']>, ParentType, ContextType>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type SpaceResolvers<ContextType = any, ParentType extends ResolversParentTypes['Space'] = ResolversParentTypes['Space']> = ResolversObject<{
  daoAddress?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  mainVotingAddress?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  membershipAddress?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  personalAddress?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  spaceAddress?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  type?: Resolver<ResolversTypes['SpaceType'], ParentType, ContextType>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type TypeResolvers<ContextType = any, ParentType extends ResolversParentTypes['Type'] = ResolversParentTypes['Type']> = ResolversObject<{
  description?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  entity?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  name?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  properties?: Resolver<Maybe<Array<Maybe<ResolversTypes['Property']>>>, ParentType, ContextType>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type ValueResolvers<ContextType = any, ParentType extends ResolversParentTypes['Value'] = ResolversParentTypes['Value']> = ResolversObject<{
  entity?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  entityId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  format?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  language?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  property?: Resolver<Maybe<ResolversTypes['Property']>, ParentType, ContextType>;
  propertyId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  spaceId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  timezone?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  unit?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  value?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type Resolvers<ContextType = any> = ResolversObject<{
  Block?: BlockResolvers<ContextType>;
  Entity?: EntityResolvers<ContextType>;
  Property?: PropertyResolvers<ContextType>;
  Query?: QueryResolvers<ContextType>;
  Relation?: RelationResolvers<ContextType>;
  Space?: SpaceResolvers<ContextType>;
  Type?: TypeResolvers<ContextType>;
  Value?: ValueResolvers<ContextType>;
}>;

