import DataLoader from "dataloader"
import {inArray} from "drizzle-orm"
import {Effect, Layer} from "effect"
import {v4 as uuid} from "uuid"
import {afterEach, beforeAll, beforeEach, describe, expect, it} from "vitest"
import {getAllRelations, getBacklinks, getRelation} from "~/src/kg/resolvers/entities"
import {Environment, make as makeEnvironment} from "~/src/services/environment"
import {entities, relations, values} from "~/src/services/storage/schema"
import {make as makeStorage, Storage} from "~/src/services/storage/storage"
import type {GraphQLContext} from "~/src/types"

// Set up Effect layers like in the main application
const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

// Create a mock GraphQL context with dataloaders for testing
const createMockContext = (): GraphQLContext => {
	return {
		entitiesLoader: new DataLoader(async (ids: readonly string[]) => {
			const storage = Effect.runSync(Storage.pipe(provideDeps))
			const entities = await storage.use(async (client) => {
				return await client.query.entities.findMany({
					where: (entities, {inArray}) => inArray(entities.id, ids),
				})
			})

			const entityMap = new Map(entities.map((e) => [e.id, e]))
			return ids.map((id) => entityMap.get(id) ?? null)
		}),
		entityNamesLoader: new DataLoader(async (ids: readonly string[]) => {
			return ids.map(() => null)
		}),
		entityDescriptionsLoader: new DataLoader(async (ids: readonly string[]) => {
			return ids.map(() => null)
		}),
		entityValuesLoader: new DataLoader(async (keys: readonly any[]) => {
			return keys.map(() => [])
		}),
		entityRelationsLoader: new DataLoader(async (keys: readonly any[]) => {
			const storage = Effect.runSync(Storage.pipe(provideDeps))
			return Promise.all(
				keys.map(async (key) => {
					const {entityId, spaceId} = key
					const relations = await storage.use(async (client) => {
						return await client.query.relations.findMany({
							where: (relations, {eq}) => eq(relations.fromEntityId, entityId),
						})
					})

					const filteredRelations = Array.isArray(relations) ? [...relations] : []

					// Apply spaceId filter if provided
					if (spaceId) {
						return filteredRelations.filter((r) => r.spaceId === spaceId)
					}

					return filteredRelations
				}),
			)
		}),
		propertiesLoader: new DataLoader(async (ids: readonly string[]) => {
			return ids.map(() => null)
		}),
		entityBacklinksLoader: new DataLoader(async (keys: readonly any[]) => {
			const storage = Effect.runSync(Storage.pipe(provideDeps))
			return Promise.all(
				keys.map(async (key) => {
					const {entityId, spaceId} = key
					const backlinks = await storage.use(async (client) => {
						return await client.query.relations.findMany({
							where: (relations, {eq}) => eq(relations.toEntityId, entityId),
						})
					})

					const filteredBacklinks = Array.isArray(backlinks) ? [...backlinks] : []

					// Apply spaceId filter if provided
					if (spaceId) {
						return filteredBacklinks.filter((r) => r.spaceId === spaceId)
					}

					return filteredBacklinks
				}),
			)
		}),
	}
}

describe("Relation Queries Integration Tests", () => {
	// Test data variables - will be regenerated for each test
	let TEST_ENTITY_1_ID: string
	let TEST_ENTITY_2_ID: string
	let TEST_ENTITY_3_ID: string
	let TEST_RELATION_1_ID: string
	let TEST_RELATION_2_ID: string
	let TEST_RELATION_3_ID: string
	let TEST_RELATION_4_ID: string
	let TEST_RELATION_ENTITY_1_ID: string
	let TEST_RELATION_ENTITY_2_ID: string
	let TEST_RELATION_ENTITY_3_ID: string
	let TEST_RELATION_ENTITY_4_ID: string
	let TEST_TYPE_1_ID: string
	let TEST_TYPE_2_ID: string
	let TEST_TYPE_3_ID: string
	let TEST_SPACE_1_ID: string
	let TEST_SPACE_2_ID: string

	beforeAll(async () => {
		// Clear all data from the database before running tests
		await Effect.runPromise(
			Effect.gen(function* () {
				const storage = yield* Storage

				yield* storage.use(async (client) => {
					// Delete all relations first (due to foreign key constraints)
					await client.delete(relations)
					// Delete all values
					await client.delete(values)
					// Delete all entities
					await client.delete(entities)
				})
			}).pipe(provideDeps),
		)
	})

	beforeEach(async () => {
		// Generate new UUIDs for each test to avoid conflicts
		TEST_ENTITY_1_ID = uuid()
		TEST_ENTITY_2_ID = uuid()
		TEST_ENTITY_3_ID = uuid()
		TEST_RELATION_1_ID = uuid()
		TEST_RELATION_2_ID = uuid()
		TEST_RELATION_3_ID = uuid()
		TEST_RELATION_4_ID = uuid()
		TEST_RELATION_ENTITY_1_ID = uuid()
		TEST_RELATION_ENTITY_2_ID = uuid()
		TEST_RELATION_ENTITY_3_ID = uuid()
		TEST_RELATION_ENTITY_4_ID = uuid()
		TEST_TYPE_1_ID = uuid()
		TEST_TYPE_2_ID = uuid()
		TEST_TYPE_3_ID = uuid()
		TEST_SPACE_1_ID = uuid()
		TEST_SPACE_2_ID = uuid()

		// Seed test data
		await Effect.runPromise(
			Effect.gen(function* () {
				const storage = yield* Storage

				yield* storage.use(async (client) => {
					// Insert test entities
					await client.insert(entities).values([
						{
							id: TEST_ENTITY_1_ID,
							createdAt: "2024-01-01T00:00:00Z",
							createdAtBlock: "block-1",
							updatedAt: "2024-01-01T00:00:00Z",
							updatedAtBlock: "block-1",
						},
						{
							id: TEST_ENTITY_2_ID,
							createdAt: "2024-01-02T00:00:00Z",
							createdAtBlock: "block-2",
							updatedAt: "2024-01-02T00:00:00Z",
							updatedAtBlock: "block-2",
						},
						{
							id: TEST_ENTITY_3_ID,
							createdAt: "2024-01-03T00:00:00Z",
							createdAtBlock: "block-3",
							updatedAt: "2024-01-03T00:00:00Z",
							updatedAtBlock: "block-3",
						},
						{
							id: TEST_RELATION_ENTITY_1_ID,
							createdAt: "2024-01-01T00:00:00Z",
							createdAtBlock: "block-1",
							updatedAt: "2024-01-01T00:00:00Z",
							updatedAtBlock: "block-1",
						},
						{
							id: TEST_RELATION_ENTITY_2_ID,
							createdAt: "2024-01-02T00:00:00Z",
							createdAtBlock: "block-2",
							updatedAt: "2024-01-02T00:00:00Z",
							updatedAtBlock: "block-2",
						},
						{
							id: TEST_RELATION_ENTITY_3_ID,
							createdAt: "2024-01-03T00:00:00Z",
							createdAtBlock: "block-3",
							updatedAt: "2024-01-03T00:00:00Z",
							updatedAtBlock: "block-3",
						},
						{
							id: TEST_RELATION_ENTITY_4_ID,
							createdAt: "2024-01-04T00:00:00Z",
							createdAtBlock: "block-4",
							updatedAt: "2024-01-04T00:00:00Z",
							updatedAtBlock: "block-4",
						},
					])

					// Insert test relations
					await client.insert(relations).values([
						{
							id: TEST_RELATION_1_ID,
							entityId: TEST_RELATION_ENTITY_1_ID,
							typeId: TEST_TYPE_1_ID,
							fromEntityId: TEST_ENTITY_1_ID,
							toEntityId: TEST_ENTITY_2_ID,
							spaceId: TEST_SPACE_1_ID,
							toSpaceId: TEST_SPACE_1_ID,
							verified: true,
							position: "1",
						},
						{
							id: TEST_RELATION_2_ID,
							entityId: TEST_RELATION_ENTITY_2_ID,
							typeId: TEST_TYPE_2_ID,
							fromEntityId: TEST_ENTITY_2_ID,
							toEntityId: TEST_ENTITY_3_ID,
							spaceId: TEST_SPACE_1_ID,
							toSpaceId: TEST_SPACE_1_ID,
							verified: false,
							position: "2",
						},
						{
							id: TEST_RELATION_3_ID,
							entityId: TEST_RELATION_ENTITY_3_ID,
							typeId: TEST_TYPE_1_ID,
							fromEntityId: TEST_ENTITY_1_ID,
							toEntityId: TEST_ENTITY_3_ID,
							spaceId: TEST_SPACE_2_ID,
							toSpaceId: TEST_SPACE_2_ID,
							verified: true,
							position: "3",
						},
						{
							id: TEST_RELATION_4_ID,
							entityId: TEST_RELATION_ENTITY_4_ID,
							typeId: TEST_TYPE_3_ID,
							fromEntityId: TEST_ENTITY_3_ID,
							toEntityId: TEST_ENTITY_1_ID,
							spaceId: TEST_SPACE_1_ID,
							toSpaceId: TEST_SPACE_1_ID,
							verified: true,
							position: "4",
						},
					])
				})
			}).pipe(provideDeps),
		)
	})

	afterEach(async () => {
		// Clean up test data
		await Effect.runPromise(
			Effect.gen(function* () {
				const storage = yield* Storage

				yield* storage.use(async (client) => {
					// Delete test relations
					await client
						.delete(relations)
						.where(
							inArray(relations.id, [
								TEST_RELATION_1_ID,
								TEST_RELATION_2_ID,
								TEST_RELATION_3_ID,
								TEST_RELATION_4_ID,
							]),
						)

					// Delete test entities
					await client
						.delete(entities)
						.where(
							inArray(entities.id, [
								TEST_ENTITY_1_ID,
								TEST_ENTITY_2_ID,
								TEST_ENTITY_3_ID,
								TEST_RELATION_ENTITY_1_ID,
								TEST_RELATION_ENTITY_2_ID,
								TEST_RELATION_ENTITY_3_ID,
								TEST_RELATION_ENTITY_4_ID,
							]),
						)
				})
			}).pipe(provideDeps),
		)
	})

	describe("Single Relation Query", () => {
		it("should return a relation by ID", async () => {
			const result = await Effect.runPromise(getRelation(TEST_RELATION_1_ID).pipe(provideDeps))

			expect(result).not.toBeNull()
			if (result) {
				expect(result.id).toBe(TEST_RELATION_1_ID)
				expect(result.entityId).toBe(TEST_RELATION_ENTITY_1_ID)
				expect(result.typeId).toBe(TEST_TYPE_1_ID)
				expect(result.fromId).toBe(TEST_ENTITY_1_ID)
				expect(result.toId).toBe(TEST_ENTITY_2_ID)
				expect(result.spaceId).toBe(TEST_SPACE_1_ID)
				expect(result.verified).toBe(true)
			}
		})

		it("should return null for non-existent relation ID", async () => {
			const nonExistentId = uuid()
			const result = await Effect.runPromise(getRelation(nonExistentId).pipe(provideDeps))

			expect(result).toBeNull()
		})

		it("should return relation with all expected fields", async () => {
			const result = await Effect.runPromise(getAllRelations({toEntityId: TEST_ENTITY_2_ID}).pipe(provideDeps))

			expect(result).not.toBeNull()
			if (result) {
				expect(result).toEqual({
					id: TEST_RELATION_2_ID,
					entityId: TEST_RELATION_ENTITY_2_ID,
					typeId: TEST_TYPE_2_ID,
					fromId: TEST_ENTITY_2_ID,
					toId: TEST_ENTITY_3_ID,
					toSpaceId: TEST_SPACE_1_ID,
					verified: false,
					position: "2",
					spaceId: TEST_SPACE_1_ID,
				})
			}
		})
	})

	describe("Multiple Relations Query", () => {
		it("should return all relations when no filter is provided", async () => {
			const result = await Effect.runPromise(getAllRelations({}).pipe(provideDeps))

			expect(result).toHaveLength(4)
			expect(result.map((r) => r.id)).toEqual(
				expect.arrayContaining([
					TEST_RELATION_1_ID,
					TEST_RELATION_2_ID,
					TEST_RELATION_3_ID,
					TEST_RELATION_4_ID,
				]),
			)
		})

		it("should filter relations by typeId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {typeId: TEST_TYPE_1_ID},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(2)
			expect(result.map((r) => r.id)).toEqual(expect.arrayContaining([TEST_RELATION_1_ID, TEST_RELATION_3_ID]))
			expect(result.every((r) => r.typeId === TEST_TYPE_1_ID)).toBe(true)
		})

		it("should filter relations by fromEntityId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {fromEntityId: TEST_ENTITY_1_ID},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(2)
			expect(result.map((r) => r.id)).toEqual(expect.arrayContaining([TEST_RELATION_1_ID, TEST_RELATION_3_ID]))
			expect(result.every((r) => r.fromId === TEST_ENTITY_1_ID)).toBe(true)
		})

		it("should filter relations by toEntityId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {toEntityId: TEST_ENTITY_3_ID},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(2)
			expect(result.map((r) => r.id)).toEqual(expect.arrayContaining([TEST_RELATION_2_ID, TEST_RELATION_3_ID]))
			expect(result.every((r) => r.toId === TEST_ENTITY_3_ID)).toBe(true)
		})

		it("should filter relations by multiple criteria", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {
						typeId: TEST_TYPE_1_ID,
						fromEntityId: TEST_ENTITY_1_ID,
					},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(2)
			expect(result.map((r) => r.id)).toEqual(expect.arrayContaining([TEST_RELATION_1_ID, TEST_RELATION_3_ID]))
			expect(result.every((r) => r.typeId === TEST_TYPE_1_ID && r.fromId === TEST_ENTITY_1_ID)).toBe(true)
		})

		it("should filter relations by specific combination", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {
						typeId: TEST_TYPE_1_ID,
						fromEntityId: TEST_ENTITY_1_ID,
						toEntityId: TEST_ENTITY_2_ID,
					},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_1_ID)
				expect(result[0].typeId).toBe(TEST_TYPE_1_ID)
				expect(result[0].fromId).toBe(TEST_ENTITY_1_ID)
				expect(result[0].toId).toBe(TEST_ENTITY_2_ID)
			}
		})

		it("should return empty array for non-matching filter", async () => {
			const nonExistentTypeId = uuid()
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {typeId: nonExistentTypeId},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0)
		})

		it("should return empty array for impossible filter combination", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {
						fromEntityId: TEST_ENTITY_1_ID,
						toEntityId: TEST_ENTITY_1_ID, // self-relation that doesn't exist
					},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0)
		})

		it("should filter relations by relationEntityId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {relationEntityId: TEST_RELATION_ENTITY_1_ID},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_1_ID)
				expect(result[0].entityId).toBe(TEST_RELATION_ENTITY_1_ID)
			}
		})

		it("should filter relations by different relationEntityId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {relationEntityId: TEST_RELATION_ENTITY_2_ID},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_2_ID)
				expect(result[0].entityId).toBe(TEST_RELATION_ENTITY_2_ID)
			}
		})

		it("should filter relations by relationEntityId combined with typeId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {
						relationEntityId: TEST_RELATION_ENTITY_1_ID,
						typeId: TEST_TYPE_1_ID,
					},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_1_ID)
				expect(result[0].entityId).toBe(TEST_RELATION_ENTITY_1_ID)
				expect(result[0].typeId).toBe(TEST_TYPE_1_ID)
			}
		})

		it("should filter relations by relationEntityId combined with fromEntityId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {
						relationEntityId: TEST_RELATION_ENTITY_3_ID,
						fromEntityId: TEST_ENTITY_1_ID,
					},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_3_ID)
				expect(result[0].entityId).toBe(TEST_RELATION_ENTITY_3_ID)
				expect(result[0].fromId).toBe(TEST_ENTITY_1_ID)
			}
		})

		it("should return empty array for non-existent relationEntityId", async () => {
			const nonExistentRelationEntityId = uuid()
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {relationEntityId: nonExistentRelationEntityId},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0)
		})

		it("should return empty array for impossible relationEntityId combination", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {
						relationEntityId: TEST_RELATION_ENTITY_1_ID,
						typeId: TEST_TYPE_2_ID, // TYPE_2 doesn't match RELATION_ENTITY_1
					},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0)
		})
	})

	describe("Pagination", () => {
		it("should respect limit parameter", async () => {
			const result = await Effect.runPromise(getAllRelations({limit: 2}).pipe(provideDeps))

			expect(result).toHaveLength(2)
		})

		it("should respect offset parameter", async () => {
			const allResults = await Effect.runPromise(getAllRelations({}).pipe(provideDeps))

			const offsetResult = await Effect.runPromise(getAllRelations({offset: 2}).pipe(provideDeps))

			expect(offsetResult).toHaveLength(2)
			expect(offsetResult.map((r) => r.id)).not.toEqual(allResults.slice(0, 2).map((r) => r.id))
		})

		it("should combine limit and offset", async () => {
			const result = await Effect.runPromise(getAllRelations({limit: 1, offset: 1}).pipe(provideDeps))

			expect(result).toHaveLength(1)
		})

		it("should handle offset beyond available results", async () => {
			const result = await Effect.runPromise(getAllRelations({offset: 100}).pipe(provideDeps))

			expect(result).toHaveLength(0)
		})

		it("should use default limit when not specified", async () => {
			const result = await Effect.runPromise(getAllRelations({}).pipe(provideDeps))

			expect(result).toHaveLength(4) // All test relations
		})

		it("should combine pagination with filters", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {typeId: TEST_TYPE_1_ID},
					limit: 1,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].typeId).toBe(TEST_TYPE_1_ID)
			}
		})
	})

	describe("Edge Cases", () => {
		it("should handle empty filter object", async () => {
			const result = await Effect.runPromise(getAllRelations({filter: {}}).pipe(provideDeps))

			expect(result).toHaveLength(4)
		})

		it("should handle null filter", async () => {
			const result = await Effect.runPromise(getAllRelations({filter: null}).pipe(provideDeps))

			expect(result).toHaveLength(4)
		})

		it("should handle undefined filter", async () => {
			const result = await Effect.runPromise(getAllRelations({filter: undefined}).pipe(provideDeps))

			expect(result).toHaveLength(4)
		})

		it("should handle zero limit", async () => {
			const result = await Effect.runPromise(getAllRelations({limit: 0}).pipe(provideDeps))

			expect(result).toHaveLength(0)
		})

		it("should handle negative limit", async () => {
			const result = await Effect.runPromise(getAllRelations({limit: -1}).pipe(provideDeps))

			// Assuming the database treats negative limits as no limit
			expect(result.length).toBeGreaterThan(0)
		})

		it("should handle negative offset", async () => {
			// Negative offset should throw an error
			await expect(Effect.runPromise(getAllRelations({offset: -1}).pipe(provideDeps))).rejects.toThrow()
		})

		it("should handle null relationEntityId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {relationEntityId: null as any},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(4) // Should return all relations
		})

		it("should handle undefined relationEntityId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {relationEntityId: undefined},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(4) // Should return all relations
		})

		it("should handle empty string relationEntityId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {relationEntityId: ""},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0) // Should return no relations
		})

		it("should handle empty string typeId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {typeId: ""},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0) // Should return no relations
		})

		it("should handle empty string fromEntityId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {fromEntityId: ""},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0) // Should return no relations
		})

		it("should handle empty string toEntityId", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {toEntityId: ""},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0) // Should return no relations
		})

		it("should handle relationEntityId with other null filters", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {
						relationEntityId: TEST_RELATION_ENTITY_1_ID,
						typeId: null as any,
						fromEntityId: undefined,
					},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].entityId).toBe(TEST_RELATION_ENTITY_1_ID)
			}
		})
	})

	describe("Different Relation Types", () => {
		it("should filter by type-2 relations", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {typeId: TEST_TYPE_2_ID},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_2_ID)
				expect(result[0].typeId).toBe(TEST_TYPE_2_ID)
			}
		})

		it("should filter by type-3 relations", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {typeId: TEST_TYPE_3_ID},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_4_ID)
				expect(result[0].typeId).toBe(TEST_TYPE_3_ID)
			}
		})

		it("should handle reverse relations", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {fromEntityId: TEST_ENTITY_3_ID},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_4_ID)
				expect(result[0].fromId).toBe(TEST_ENTITY_3_ID)
				expect(result[0].toId).toBe(TEST_ENTITY_1_ID)
			}
		})

		it("should filter by relationEntityId for type-1 relations", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {
						relationEntityId: TEST_RELATION_ENTITY_1_ID,
						typeId: TEST_TYPE_1_ID,
					},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_1_ID)
				expect(result[0].entityId).toBe(TEST_RELATION_ENTITY_1_ID)
				expect(result[0].typeId).toBe(TEST_TYPE_1_ID)
			}
		})

		it("should filter by relationEntityId for type-2 relations", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {
						relationEntityId: TEST_RELATION_ENTITY_2_ID,
						typeId: TEST_TYPE_2_ID,
					},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_2_ID)
				expect(result[0].entityId).toBe(TEST_RELATION_ENTITY_2_ID)
				expect(result[0].typeId).toBe(TEST_TYPE_2_ID)
			}
		})

		it("should filter by relationEntityId for type-3 relations", async () => {
			const result = await Effect.runPromise(
				getAllRelations({
					filter: {
						relationEntityId: TEST_RELATION_ENTITY_4_ID,
						typeId: TEST_TYPE_3_ID,
					},
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_4_ID)
				expect(result[0].entityId).toBe(TEST_RELATION_ENTITY_4_ID)
				expect(result[0].typeId).toBe(TEST_TYPE_3_ID)
			}
		})

		it("should return all relations with same relationEntityId across different types", async () => {
			// First, let's verify we have different relation entities for different types
			const type1Results = await Effect.runPromise(
				getAllRelations({
					filter: {typeId: TEST_TYPE_1_ID},
				}).pipe(provideDeps),
			)

			const relationEntityIds = type1Results.map((r) => r.entityId)
			expect(relationEntityIds).toContain(TEST_RELATION_ENTITY_1_ID)
			expect(relationEntityIds).toContain(TEST_RELATION_ENTITY_3_ID)
		})
	})

	describe("Verified Relations", () => {
		it("should return both verified and unverified relations", async () => {
			const result = await Effect.runPromise(getAllRelations({}).pipe(provideDeps))

			const verifiedCount = result.filter((r) => r.verified).length
			const unverifiedCount = result.filter((r) => !r.verified).length

			expect(verifiedCount).toBe(3)
			expect(unverifiedCount).toBe(1)
		})

		it("should preserve verified status in query results", async () => {
			const verifiedRelation = await Effect.runPromise(getRelation(TEST_RELATION_1_ID).pipe(provideDeps))

			const unverifiedRelation = await Effect.runPromise(getRelation(TEST_RELATION_2_ID).pipe(provideDeps))

			if (verifiedRelation && unverifiedRelation) {
				expect(verifiedRelation.verified).toBe(true)
				expect(unverifiedRelation.verified).toBe(false)
			}
		})
	})

	describe("Backlinks", () => {
		it("should return relations where toEntityId equals the entity's id", async () => {
			// Get backlinks for TEST_ENTITY_2_ID (should find TEST_RELATION_1_ID)
			const backlinks = await Effect.runPromise(
				getBacklinks({id: TEST_ENTITY_2_ID}, createMockContext()).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_1_ID)
				expect(result[0].toId).toBe(TEST_ENTITY_2_ID)
				expect(result[0].fromId).toBe(TEST_ENTITY_1_ID)
			}
		})

		it("should return multiple backlinks when entity is referenced multiple times", async () => {
			// Get backlinks for TEST_ENTITY_3_ID (should find TEST_RELATION_2_ID and TEST_RELATION_3_ID)
			const backlinks = await Effect.runPromise(
				getBacklinks({id: TEST_ENTITY_3_ID}, createMockContext()).pipe(provideDeps),
			)

			expect(result).toHaveLength(2)
			expect(result.map((r) => r.id)).toEqual(expect.arrayContaining([TEST_RELATION_2_ID, TEST_RELATION_3_ID]))
			expect(result.every((r) => r.toId === TEST_ENTITY_3_ID)).toBe(true)
		})

		it("should return empty array when entity has no backlinks", async () => {
			// Create a new entity that has no incoming relations
			const newEntityId = uuid()
			await Effect.runPromise(
				Effect.gen(function* () {
					const storage = yield* Storage
					yield* storage.use(async (client) => {
						await client.insert(entities).values([
							{
								id: newEntityId,
								createdAt: "2024-01-01T00:00:00Z",
								createdAtBlock: "block-new",
								updatedAt: "2024-01-01T00:00:00Z",
								updatedAtBlock: "block-new",
							},
						])
					})
				}).pipe(provideDeps),
			)

			const result = await Effect.runPromise(getBacklinks(newEntityId).pipe(provideDeps))

			expect(result).toHaveLength(0)

			// Clean up
			await Effect.runPromise(
				Effect.gen(function* () {
					const storage = yield* Storage
					yield* storage.use(async (client) => {
						await client.delete(entities).where(inArray(entities.id, [newEntityId]))
					})
				}).pipe(provideDeps),
			)
		})

		it("should filter backlinks by spaceId when provided", async () => {
			// Get backlinks for TEST_ENTITY_3_ID in TEST_SPACE_1_ID (should find only TEST_RELATION_2_ID)
			const backlinks = await Effect.runPromise(
				getBacklinks({id: TEST_ENTITY_2_ID, spaceId: TEST_SPACE_ID}, createMockContext()).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0].id).toBe(TEST_RELATION_2_ID)
				expect(result[0].toId).toBe(TEST_ENTITY_3_ID)
				expect(result[0].spaceId).toBe(TEST_SPACE_1_ID)
			}
		})

		it("should return backlinks from different spaces when spaceId is not provided", async () => {
			// Get all backlinks for TEST_ENTITY_3_ID (should find relations from both spaces)
			const backlinks = await Effect.runPromise(
				getBacklinks({id: TEST_ENTITY_2_ID}, createMockContext()).pipe(provideDeps),
			)

			expect(result).toHaveLength(2)
			const spaceIds = result.map((r) => r.spaceId)
			expect(spaceIds).toContain(TEST_SPACE_1_ID)
			expect(spaceIds).toContain(TEST_SPACE_2_ID)
		})

		it("should return backlinks with all expected fields", async () => {
			const result = await Effect.runPromise(getBacklinks(TEST_ENTITY_2_ID).pipe(provideDeps))

			expect(result).toHaveLength(1)
			if (result[0]) {
				expect(result[0]).toEqual({
					id: TEST_RELATION_1_ID,
					entityId: TEST_RELATION_ENTITY_1_ID,
					typeId: TEST_TYPE_1_ID,
					fromId: TEST_ENTITY_1_ID,
					toId: TEST_ENTITY_2_ID,
					toSpaceId: TEST_SPACE_1_ID,
					verified: true,
					position: "1",
					spaceId: TEST_SPACE_1_ID,
				})
			}
		})
	})
})
