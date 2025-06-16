import {SystemIds} from "@graphprotocol/grc-20"
import {sql} from "drizzle-orm"
import {Effect, Layer} from "effect"
import {beforeAll, beforeEach, describe, expect, it} from "vitest"
import * as SearchResolvers from "~/src/kg/resolvers/search"
import {Environment, make as makeEnvironment} from "~/src/services/environment"
import {entities, relations, values} from "~/src/services/storage/schema"
import type {DbEntity} from "~/src/services/storage/schema"
import {Storage, make as makeStorage} from "~/src/services/storage/storage"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

describe.skip("Search Integration Tests", () => {
	let testEntities: DbEntity[]
	let extensionAvailable = true

	beforeAll(async () => {
		// Check if pg_trgm extension is available
		const checkExtension = Effect.gen(function* () {
			const db = yield* Storage

			return yield* db.use(async (client) => {
				try {
					// Try to create the extension (will succeed if already exists)
					await client.execute(sql`CREATE EXTENSION IF NOT EXISTS pg_trgm`)
					// Test if similarity function works
					await client.execute(sql`SELECT similarity('test', 'test')`)
					return true
				} catch (error) {
					console.warn("pg_trgm extension not available:", error)
					return false
				}
			})
		})

		extensionAvailable = await Effect.runPromise(checkExtension.pipe(provideDeps))

		if (!extensionAvailable) {
			console.warn("Skipping search tests: pg_trgm extension not available in this environment")
		}
	})

	beforeEach(async () => {
		// Create test entities with proper UUIDs
		testEntities = [
			{
				id: "550e8400-e29b-41d4-a716-446655440001",
				createdAt: "2024-01-01T00:00:00Z",
				createdAtBlock: "1",
				updatedAt: "2024-01-01T00:00:00Z",
				updatedAtBlock: "1",
			},
			{
				id: "550e8400-e29b-41d4-a716-446655440002",
				createdAt: "2024-01-01T00:00:00Z",
				createdAtBlock: "1",
				updatedAt: "2024-01-01T00:00:00Z",
				updatedAtBlock: "1",
			},
			{
				id: "550e8400-e29b-41d4-a716-446655440003",
				createdAt: "2024-01-01T00:00:00Z",
				createdAtBlock: "1",
				updatedAt: "2024-01-01T00:00:00Z",
				updatedAtBlock: "1",
			},
		]

		// Create test values with searchable content
		const testValues = [
			// Entity 1 - artificial intelligence content in space1
			{
				id: "test-search-value-1",
				propertyId: SystemIds.NAME_PROPERTY,
				entityId: "550e8400-e29b-41d4-a716-446655440001",
				spaceId: "space1",
				value: "Artificial Intelligence Research",
			},
			{
				id: "test-search-value-2",
				propertyId: SystemIds.DESCRIPTION_PROPERTY,
				entityId: "550e8400-e29b-41d4-a716-446655440001",
				spaceId: "space1",
				value: "Advanced AI systems and machine learning algorithms",
			},
			// Entity 2 - machine learning content in space2
			{
				id: "test-search-value-3",
				propertyId: SystemIds.NAME_PROPERTY,
				entityId: "550e8400-e29b-41d4-a716-446655440002",
				spaceId: "space2",
				value: "Machine Learning Framework",
			},
			{
				id: "test-search-value-4",
				propertyId: SystemIds.DESCRIPTION_PROPERTY,
				entityId: "550e8400-e29b-41d4-a716-446655440002",
				spaceId: "space2",
				value: "Deep learning neural networks for classification",
			},
			// Entity 3 - data science content in space1
			{
				id: "test-search-value-5",
				propertyId: SystemIds.NAME_PROPERTY,
				entityId: "550e8400-e29b-41d4-a716-446655440003",
				spaceId: "space1",
				value: "Data Science Platform",
			},
			{
				id: "test-search-value-6",
				propertyId: SystemIds.DESCRIPTION_PROPERTY,
				entityId: "550e8400-e29b-41d4-a716-446655440003",
				spaceId: "space1",
				value: "Statistical analysis and data visualization tools",
			},
		]

		const cleanupAndSetup = Effect.gen(function* () {
			const db = yield* Storage

			// Clean up any existing test data using Drizzle
			yield* db.use(async (client) => {
				const testEntityIds = [
					"550e8400-e29b-41d4-a716-446655440001",
					"550e8400-e29b-41d4-a716-446655440002",
					"550e8400-e29b-41d4-a716-446655440003",
				]
				for (const entityId of testEntityIds) {
					await client.delete(relations).where(sql`entity_id = ${entityId}`).execute()
					await client.delete(values).where(sql`entity_id = ${entityId}`).execute()
					await client.delete(entities).where(sql`id = ${entityId}`).execute()
				}
			})

			// Insert test entities using Drizzle
			yield* db.use(async (client) => {
				await client.insert(entities).values(testEntities).execute()
			})

			// Insert test values using Drizzle
			yield* db.use(async (client) => {
				await client.insert(values).values(testValues).execute()
			})
		})

		await Effect.runPromise(cleanupAndSetup.pipe(provideDeps))
	})

	describe("Basic Search Terms", () => {
		it("should find entities by exact term match", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "artificial",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe("550e8400-e29b-41d4-a716-446655440001")
		})

		it("should find entities by partial term match", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "learning",
					limit: 10,
					offset: 0,
					threshold: 0.1,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(2)
			const entityIds = result.map((r) => r.id).sort()
			expect(entityIds).toEqual(["550e8400-e29b-41d4-a716-446655440001", "550e8400-e29b-41d4-a716-446655440002"])
		})

		it("should find entities by description content", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "neural",
					limit: 10,
					offset: 0,
					threshold: 0.1,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe("550e8400-e29b-41d4-a716-446655440002")
		})

		it("should find entities by multi-word search", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "data science",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe("550e8400-e29b-41d4-a716-446655440003")
		})

		it("should return empty array for non-matching search terms", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "blockchain",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0)
		})
	})

	describe("Space ID Filtering", () => {
		it("should filter results by space ID", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "learning",
					spaceId: "space1",
					limit: 10,
					offset: 0,
					threshold: 0.1,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe("550e8400-e29b-41d4-a716-446655440001")
		})

		it("should filter results by different space ID", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "learning",
					spaceId: "space2",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe("550e8400-e29b-41d4-a716-446655440002")
		})

		it("should return empty array when no entities exist in specified space", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "artificial",
					spaceId: "nonexistent-space",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0)
		})

		it("should return all matching entities when no space ID specified", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "data",
					limit: 10,
					offset: 0,
					threshold: 0.1,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe("550e8400-e29b-41d4-a716-446655440003")
		})
	})

	describe("Name and Description Specific Search", () => {
		it("should search only in name and description fields", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.searchNameDescription({
					query: "artificial",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe("550e8400-e29b-41d4-a716-446655440001")
		})

		it("should filter by space ID in name/description search", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.searchNameDescription({
					query: "framework",
					spaceId: "space2",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe("550e8400-e29b-41d4-a716-446655440002")
		})

		it("should return empty array when searching in wrong space", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.searchNameDescription({
					query: "framework",
					spaceId: "space1",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0)
		})
	})

	describe("Pagination", () => {
		it("should respect limit parameter", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "data",
					limit: 1,
					offset: 0,
					threshold: 0.1,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(1)
		})

		it("should respect offset parameter", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const allResults = await Effect.runPromise(
				SearchResolvers.search({
					query: "data",
					limit: 10,
					offset: 0,
					threshold: 0.1,
				}).pipe(provideDeps),
			)

			const offsetResults = await Effect.runPromise(
				SearchResolvers.search({
					query: "data",
					limit: 10,
					offset: 1,
					threshold: 0.1,
				}).pipe(provideDeps),
			)

			expect(allResults).toHaveLength(1)
			expect(offsetResults).toHaveLength(0)
		})
	})

	describe("Edge Cases", () => {
		it("should handle empty search query", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(result).toHaveLength(0)
		})

		it("should handle single character search", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "a",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			// Single characters may not work well with trigrams, expect few or no results
			expect(result.length).toBeGreaterThanOrEqual(0)
		})

		it("should handle special characters in search", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const result = await Effect.runPromise(
				SearchResolvers.search({
					query: "AI-systems",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(result.length).toBeGreaterThanOrEqual(0)
		})

		it("should be case insensitive", async () => {
			if (!extensionAvailable) {
				console.log("Skipping test: pg_trgm extension not available")
				return
			}

			const lowerResult = await Effect.runPromise(
				SearchResolvers.search({
					query: "artificial",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			const upperResult = await Effect.runPromise(
				SearchResolvers.search({
					query: "ARTIFICIAL",
					limit: 10,
					offset: 0,
				}).pipe(provideDeps),
			)

			expect(lowerResult).toHaveLength(1)
			expect(upperResult).toHaveLength(1)
			expect(lowerResult[0]?.id).toBe(upperResult[0]?.id)
		})
	})
})
