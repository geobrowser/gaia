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

export type CheckboxFilter = {
  exists?: InputMaybe<Scalars['Boolean']['input']>;
  is?: InputMaybe<Scalars['Boolean']['input']>;
};

export type Entity = {
  __typename?: 'Entity';
  blocks: Array<Maybe<Entity>>;
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

export type EntityFilter = {
  NOT?: InputMaybe<EntityFilter>;
  OR?: InputMaybe<Array<EntityFilter>>;
  value?: InputMaybe<ValueFilter>;
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
  entity?: Maybe<Entity>;
  id: Scalars['ID']['output'];
  valueType: Scalars['String']['output'];
};

export type Query = {
  __typename?: 'Query';
  entities: Array<Maybe<Entity>>;
  entity?: Maybe<Entity>;
  types: Array<Maybe<Type>>;
};


export type QueryEntitiesArgs = {
  filter?: InputMaybe<EntityFilter>;
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
};


export type QueryEntityArgs = {
  id: Scalars['String']['input'];
};


export type QueryTypesArgs = {
  filter?: InputMaybe<TypeFilter>;
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
};

export type Relation = {
  __typename?: 'Relation';
  from?: Maybe<Entity>;
  fromId: Scalars['String']['output'];
  id: Scalars['ID']['output'];
  position?: Maybe<Scalars['String']['output']>;
  spaceId: Scalars['String']['output'];
  to?: Maybe<Entity>;
  toId: Scalars['String']['output'];
  toSpaceId?: Maybe<Scalars['String']['output']>;
  type?: Maybe<Entity>;
  typeId: Scalars['String']['output'];
};

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

export type TypeFilter = {
  spaceId?: InputMaybe<Scalars['String']['input']>;
};

export type Value = {
  __typename?: 'Value';
  entity?: Maybe<Entity>;
  entityId: Scalars['String']['output'];
  format?: Maybe<Scalars['String']['output']>;
  hasDate?: Maybe<Scalars['Boolean']['output']>;
  hasTime?: Maybe<Scalars['Boolean']['output']>;
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
  Boolean: ResolverTypeWrapper<Scalars['Boolean']['output']>;
  CheckboxFilter: CheckboxFilter;
  Entity: ResolverTypeWrapper<DbEntity>;
  EntityFilter: EntityFilter;
  Float: ResolverTypeWrapper<Scalars['Float']['output']>;
  ID: ResolverTypeWrapper<Scalars['ID']['output']>;
  Int: ResolverTypeWrapper<Scalars['Int']['output']>;
  NumberFilter: NumberFilter;
  PointFilter: PointFilter;
  Property: ResolverTypeWrapper<Omit<Property, 'entity'> & { entity?: Maybe<ResolversTypes['Entity']> }>;
  Query: ResolverTypeWrapper<{}>;
  Relation: ResolverTypeWrapper<Omit<Relation, 'from' | 'to' | 'type'> & { from?: Maybe<ResolversTypes['Entity']>, to?: Maybe<ResolversTypes['Entity']>, type?: Maybe<ResolversTypes['Entity']> }>;
  String: ResolverTypeWrapper<Scalars['String']['output']>;
  TextFilter: TextFilter;
  Type: ResolverTypeWrapper<Omit<Type, 'entity' | 'properties'> & { entity?: Maybe<ResolversTypes['Entity']>, properties?: Maybe<Array<Maybe<ResolversTypes['Property']>>> }>;
  TypeFilter: TypeFilter;
  Value: ResolverTypeWrapper<Omit<Value, 'entity' | 'property'> & { entity?: Maybe<ResolversTypes['Entity']>, property?: Maybe<ResolversTypes['Property']> }>;
  ValueFilter: ValueFilter;
}>;

/** Mapping between all available schema types and the resolvers parents */
export type ResolversParentTypes = ResolversObject<{
  Boolean: Scalars['Boolean']['output'];
  CheckboxFilter: CheckboxFilter;
  Entity: DbEntity;
  EntityFilter: EntityFilter;
  Float: Scalars['Float']['output'];
  ID: Scalars['ID']['output'];
  Int: Scalars['Int']['output'];
  NumberFilter: NumberFilter;
  PointFilter: PointFilter;
  Property: Omit<Property, 'entity'> & { entity?: Maybe<ResolversParentTypes['Entity']> };
  Query: {};
  Relation: Omit<Relation, 'from' | 'to' | 'type'> & { from?: Maybe<ResolversParentTypes['Entity']>, to?: Maybe<ResolversParentTypes['Entity']>, type?: Maybe<ResolversParentTypes['Entity']> };
  String: Scalars['String']['output'];
  TextFilter: TextFilter;
  Type: Omit<Type, 'entity' | 'properties'> & { entity?: Maybe<ResolversParentTypes['Entity']>, properties?: Maybe<Array<Maybe<ResolversParentTypes['Property']>>> };
  TypeFilter: TypeFilter;
  Value: Omit<Value, 'entity' | 'property'> & { entity?: Maybe<ResolversParentTypes['Entity']>, property?: Maybe<ResolversParentTypes['Property']> };
  ValueFilter: ValueFilter;
}>;

export type EntityResolvers<ContextType = any, ParentType extends ResolversParentTypes['Entity'] = ResolversParentTypes['Entity']> = ResolversObject<{
  blocks?: Resolver<Array<Maybe<ResolversTypes['Entity']>>, ParentType, ContextType>;
  createdAt?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  createdAtBlock?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  description?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  name?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  relations?: Resolver<Array<Maybe<ResolversTypes['Relation']>>, ParentType, ContextType>;
  spaces?: Resolver<Array<Maybe<ResolversTypes['String']>>, ParentType, ContextType>;
  types?: Resolver<Array<Maybe<ResolversTypes['Entity']>>, ParentType, ContextType>;
  updatedAt?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  updatedAtBlock?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  values?: Resolver<Array<Maybe<ResolversTypes['Value']>>, ParentType, ContextType>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type PropertyResolvers<ContextType = any, ParentType extends ResolversParentTypes['Property'] = ResolversParentTypes['Property']> = ResolversObject<{
  entity?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  valueType?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type QueryResolvers<ContextType = any, ParentType extends ResolversParentTypes['Query'] = ResolversParentTypes['Query']> = ResolversObject<{
  entities?: Resolver<Array<Maybe<ResolversTypes['Entity']>>, ParentType, ContextType, RequireFields<QueryEntitiesArgs, 'limit' | 'offset'>>;
  entity?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType, RequireFields<QueryEntityArgs, 'id'>>;
  types?: Resolver<Array<Maybe<ResolversTypes['Type']>>, ParentType, ContextType, RequireFields<QueryTypesArgs, 'limit' | 'offset'>>;
}>;

export type RelationResolvers<ContextType = any, ParentType extends ResolversParentTypes['Relation'] = ResolversParentTypes['Relation']> = ResolversObject<{
  from?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  fromId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  position?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  spaceId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  to?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  toId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  toSpaceId?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  type?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  typeId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
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
  hasDate?: Resolver<Maybe<ResolversTypes['Boolean']>, ParentType, ContextType>;
  hasTime?: Resolver<Maybe<ResolversTypes['Boolean']>, ParentType, ContextType>;
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
  Entity?: EntityResolvers<ContextType>;
  Property?: PropertyResolvers<ContextType>;
  Query?: QueryResolvers<ContextType>;
  Relation?: RelationResolvers<ContextType>;
  Type?: TypeResolvers<ContextType>;
  Value?: ValueResolvers<ContextType>;
}>;

