import { GraphQLResolveInfo } from 'graphql';
import { DbEntity, DbProperty } from '../services/storage/schema';
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

export type Entity = {
  __typename?: 'Entity';
  createdAt: Scalars['String']['output'];
  createdAtBlock: Scalars['String']['output'];
  id: Scalars['ID']['output'];
  name?: Maybe<Scalars['String']['output']>;
  relations: Array<Maybe<Relation>>;
  spaces: Array<Maybe<Scalars['String']['output']>>;
  types: Array<Maybe<Entity>>;
  updatedAt: Scalars['String']['output'];
  updatedAtBlock: Scalars['String']['output'];
  values: Array<Maybe<Value>>;
};

export type Query = {
  __typename?: 'Query';
  entities: Array<Maybe<Entity>>;
  entity?: Maybe<Entity>;
};


export type QueryEntitiesArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
};


export type QueryEntityArgs = {
  id: Scalars['String']['input'];
};

export type Relation = {
  __typename?: 'Relation';
  from?: Maybe<Entity>;
  fromId: Scalars['String']['output'];
  id: Scalars['ID']['output'];
  index?: Maybe<Scalars['String']['output']>;
  spaceId: Scalars['String']['output'];
  to?: Maybe<Entity>;
  toId: Scalars['String']['output'];
  type?: Maybe<Entity>;
  typeId: Scalars['String']['output'];
};

export type Value = {
  __typename?: 'Value';
  attribute?: Maybe<Entity>;
  attributeId: Scalars['String']['output'];
  entity?: Maybe<Entity>;
  entityId: Scalars['String']['output'];
  formatOption?: Maybe<Scalars['String']['output']>;
  id: Scalars['ID']['output'];
  languageOption?: Maybe<Scalars['String']['output']>;
  spaceId: Scalars['String']['output'];
  unitOption?: Maybe<Scalars['String']['output']>;
  value?: Maybe<Scalars['String']['output']>;
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
  Entity: ResolverTypeWrapper<DbEntity>;
  ID: ResolverTypeWrapper<Scalars['ID']['output']>;
  Int: ResolverTypeWrapper<Scalars['Int']['output']>;
  Query: ResolverTypeWrapper<{}>;
  Relation: ResolverTypeWrapper<Omit<Relation, 'from' | 'to' | 'type'> & { from?: Maybe<ResolversTypes['Entity']>, to?: Maybe<ResolversTypes['Entity']>, type?: Maybe<ResolversTypes['Entity']> }>;
  String: ResolverTypeWrapper<Scalars['String']['output']>;
  Value: ResolverTypeWrapper<Omit<Value, 'attribute' | 'entity'> & { attribute?: Maybe<ResolversTypes['Entity']>, entity?: Maybe<ResolversTypes['Entity']> }>;
}>;

/** Mapping between all available schema types and the resolvers parents */
export type ResolversParentTypes = ResolversObject<{
  Boolean: Scalars['Boolean']['output'];
  Entity: DbEntity;
  ID: Scalars['ID']['output'];
  Int: Scalars['Int']['output'];
  Query: {};
  Relation: Omit<Relation, 'from' | 'to' | 'type'> & { from?: Maybe<ResolversParentTypes['Entity']>, to?: Maybe<ResolversParentTypes['Entity']>, type?: Maybe<ResolversParentTypes['Entity']> };
  String: Scalars['String']['output'];
  Value: Omit<Value, 'attribute' | 'entity'> & { attribute?: Maybe<ResolversParentTypes['Entity']>, entity?: Maybe<ResolversParentTypes['Entity']> };
}>;

export type EntityResolvers<ContextType = any, ParentType extends ResolversParentTypes['Entity'] = ResolversParentTypes['Entity']> = ResolversObject<{
  createdAt?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  createdAtBlock?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
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

export type QueryResolvers<ContextType = any, ParentType extends ResolversParentTypes['Query'] = ResolversParentTypes['Query']> = ResolversObject<{
  entities?: Resolver<Array<Maybe<ResolversTypes['Entity']>>, ParentType, ContextType, RequireFields<QueryEntitiesArgs, 'limit' | 'offset'>>;
  entity?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType, RequireFields<QueryEntityArgs, 'id'>>;
}>;

export type RelationResolvers<ContextType = any, ParentType extends ResolversParentTypes['Relation'] = ResolversParentTypes['Relation']> = ResolversObject<{
  from?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  fromId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  index?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  spaceId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  to?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  toId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  type?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  typeId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type ValueResolvers<ContextType = any, ParentType extends ResolversParentTypes['Value'] = ResolversParentTypes['Value']> = ResolversObject<{
  attribute?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  attributeId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  entity?: Resolver<Maybe<ResolversTypes['Entity']>, ParentType, ContextType>;
  entityId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  formatOption?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  id?: Resolver<ResolversTypes['ID'], ParentType, ContextType>;
  languageOption?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  spaceId?: Resolver<ResolversTypes['String'], ParentType, ContextType>;
  unitOption?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  value?: Resolver<Maybe<ResolversTypes['String']>, ParentType, ContextType>;
  __isTypeOf?: IsTypeOfResolverFn<ParentType, ContextType>;
}>;

export type Resolvers<ContextType = any> = ResolversObject<{
  Entity?: EntityResolvers<ContextType>;
  Query?: QueryResolvers<ContextType>;
  Relation?: RelationResolvers<ContextType>;
  Value?: ValueResolvers<ContextType>;
}>;

