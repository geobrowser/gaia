import {Effect, Layer} from "effect"
import {v4 as uuid} from "uuid"
import {afterEach, beforeEach, describe, expect, it} from "vitest"
import {DataType} from "../generated/graphql"
import {getProperties} from "../kg/resolvers/properties"
import {Environment, make as makeEnvironment} from "../services/environment"
import {properties} from "../services/storage/schema"
import {make as makeStorage, Storage} from "../services/storage/storage"

// Set up Effect layers like in the main application
const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

describe("Properties Query Integration Tests", () => {
	// Test data variables - will be regenerated for each test
	let TEXT_PROPERTY_ID: string
	let NUMBER_PROPERTY_ID: string
	let CHECKBOX_PROPERTY_ID: string
	let TIME_PROPERTY_ID: string
	let POINT_PROPERTY_ID: string
	let RELATION_PROPERTY_ID: string
	let EXTRA_TEXT_PROPERTY_ID: string

	beforeEach(async () => {
		// Generate fresh UUIDs for each test to ensure isolation
		TEXT_PROPERTY_ID = uuid()
		NUMBER_PROPERTY_ID = uuid()
		CHECKBOX_PROPERTY_ID = uuid()
		TIME_PROPERTY_ID = uuid()
		POINT_PROPERTY_ID = uuid()
		RELATION_PROPERTY_ID = uuid()
		EXTRA_TEXT_PROPERTY_ID = uuid()

		await Effect.runPromise(
			provideDeps(
				Effect.gen(function* () {
					const db = yield* Storage

					yield* db.use(async (client) => {
						// Clear existing test data
						await client.delete(properties)

						// Insert test properties with different data types
						await client.insert(properties).values([
							{id: TEXT_PROPERTY_ID, type: "Text"},
							{id: NUMBER_PROPERTY_ID, type: "Number"},
							{id: CHECKBOX_PROPERTY_ID, type: "Checkbox"},
							{id: TIME_PROPERTY_ID, type: "Time"},
							{id: POINT_PROPERTY_ID, type: "Point"},
							{id: RELATION_PROPERTY_ID, type: "Relation"},
							{id: EXTRA_TEXT_PROPERTY_ID, type: "Text"},
						])
					})
				}),
			),
		)
	})

	afterEach(async () => {
		await Effect.runPromise(
			provideDeps(
				Effect.gen(function* () {
					const db = yield* Storage
					yield* db.use(async (client) => {
						await client.delete(properties)
					})
				}),
			),
		)
	})

	describe("Basic Properties Query", () => {
		it("should return all properties without filter", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({limit: 100, offset: 0})))

			expect(result).toHaveLength(7)
			const propertyIds = result.map((p) => p.id).sort()
			const expectedIds = [
				TEXT_PROPERTY_ID,
				NUMBER_PROPERTY_ID,
				CHECKBOX_PROPERTY_ID,
				TIME_PROPERTY_ID,
				POINT_PROPERTY_ID,
				RELATION_PROPERTY_ID,
				EXTRA_TEXT_PROPERTY_ID,
			].sort()
			expect(propertyIds).toEqual(expectedIds)
		})

		it("should return correct data types for all properties", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({limit: 100, offset: 0})))

			const propertyMap = new Map(result.map((p) => [p.id, p.dataType]))

			expect(propertyMap.get(TEXT_PROPERTY_ID)).toBe(DataType.Text)
			expect(propertyMap.get(NUMBER_PROPERTY_ID)).toBe(DataType.Number)
			expect(propertyMap.get(CHECKBOX_PROPERTY_ID)).toBe(DataType.Checkbox)
			expect(propertyMap.get(TIME_PROPERTY_ID)).toBe(DataType.Time)
			expect(propertyMap.get(POINT_PROPERTY_ID)).toBe(DataType.Point)
			expect(propertyMap.get(RELATION_PROPERTY_ID)).toBe(DataType.Relation)
			expect(propertyMap.get(EXTRA_TEXT_PROPERTY_ID)).toBe(DataType.Text)
		})
	})

	describe("Data Type Filtering", () => {
		it("should filter properties by Text data type", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Text},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(2)
			const propertyIds = result.map((p) => p.id).sort()
			expect(propertyIds).toEqual([TEXT_PROPERTY_ID, EXTRA_TEXT_PROPERTY_ID].sort())
			expect(result.every((p) => p.dataType === DataType.Text)).toBe(true)
		})

		it("should filter properties by Number data type", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Number},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe(NUMBER_PROPERTY_ID)
			expect(result[0]?.dataType).toBe(DataType.Number)
		})

		it("should filter properties by Checkbox data type", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Checkbox},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe(CHECKBOX_PROPERTY_ID)
			expect(result[0]?.dataType).toBe(DataType.Checkbox)
		})

		it("should filter properties by Time data type", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Time},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe(TIME_PROPERTY_ID)
			expect(result[0]?.dataType).toBe(DataType.Time)
		})

		it("should filter properties by Point data type", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Point},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe(POINT_PROPERTY_ID)
			expect(result[0]?.dataType).toBe(DataType.Point)
		})

		it("should filter properties by Relation data type", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Relation},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe(RELATION_PROPERTY_ID)
			expect(result[0]?.dataType).toBe(DataType.Relation)
		})

		it("should return empty array for non-existent data type filter", async () => {
			// Create a property with a different type and then filter for something that doesn't exist
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Text},
						limit: 100,
						offset: 0,
					}),
				),
			)

			// Delete all text properties first
			await Effect.runPromise(
				provideDeps(
					Effect.gen(function* () {
						const db = yield* Storage
						yield* db.use(async (client) => {
							await client.delete(properties)
							// Insert only non-text properties
							await client.insert(properties).values([
								{id: NUMBER_PROPERTY_ID, type: "Number"},
								{id: CHECKBOX_PROPERTY_ID, type: "Checkbox"},
							])
						})
					}),
				),
			)

			const filteredResult = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Text},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(filteredResult).toHaveLength(0)
		})
	})

	describe("ID Filtering", () => {
		it("should filter properties by single ID", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {id: {in: [TEXT_PROPERTY_ID]}},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe(TEXT_PROPERTY_ID)
			expect(result[0]?.dataType).toBe(DataType.Text)
		})

		it("should filter properties by multiple IDs", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {
							id: {
								in: [TEXT_PROPERTY_ID, NUMBER_PROPERTY_ID, CHECKBOX_PROPERTY_ID],
							},
						},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(3)
			const propertyIds = result.map((p) => p.id).sort()
			expect(propertyIds).toEqual([TEXT_PROPERTY_ID, NUMBER_PROPERTY_ID, CHECKBOX_PROPERTY_ID].sort())

			// Verify each property has correct data type
			const propertyMap = new Map(result.map((p) => [p.id, p.dataType]))
			expect(propertyMap.get(TEXT_PROPERTY_ID)).toBe(DataType.Text)
			expect(propertyMap.get(NUMBER_PROPERTY_ID)).toBe(DataType.Number)
			expect(propertyMap.get(CHECKBOX_PROPERTY_ID)).toBe(DataType.Checkbox)
		})

		it("should filter properties by all available IDs", async () => {
			const allIds = [
				TEXT_PROPERTY_ID,
				NUMBER_PROPERTY_ID,
				CHECKBOX_PROPERTY_ID,
				TIME_PROPERTY_ID,
				POINT_PROPERTY_ID,
				RELATION_PROPERTY_ID,
				EXTRA_TEXT_PROPERTY_ID,
			]

			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {id: {in: allIds}},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(7)
			const propertyIds = result.map((p) => p.id).sort()
			expect(propertyIds).toEqual(allIds.sort())
		})

		it("should return empty array for non-existent ID", async () => {
			const nonExistentId = uuid()

			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {id: {in: [nonExistentId]}},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(0)
		})

		it("should return partial results for mix of existing and non-existent IDs", async () => {
			const nonExistentId = uuid()

			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {
							id: {in: [TEXT_PROPERTY_ID, nonExistentId, NUMBER_PROPERTY_ID]},
						},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(2)
			const propertyIds = result.map((p) => p.id).sort()
			expect(propertyIds).toEqual([TEXT_PROPERTY_ID, NUMBER_PROPERTY_ID].sort())
		})

		it("should return empty array for empty ID array", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {id: {in: []}},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(0)
		})

		it("should handle duplicate IDs in filter", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {
							id: {
								in: [TEXT_PROPERTY_ID, TEXT_PROPERTY_ID, NUMBER_PROPERTY_ID],
							},
						},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(2)
			const propertyIds = result.map((p) => p.id).sort()
			expect(propertyIds).toEqual([TEXT_PROPERTY_ID, NUMBER_PROPERTY_ID].sort())
		})
	})

	describe("Combined Filtering", () => {
		it("should filter by both ID and data type - matching case", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {
							id: {in: [TEXT_PROPERTY_ID, NUMBER_PROPERTY_ID]},
							dataType: DataType.Text,
						},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe(TEXT_PROPERTY_ID)
			expect(result[0]?.dataType).toBe(DataType.Text)
		})

		it("should filter by both ID and data type - non-matching case", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {
							id: {in: [TEXT_PROPERTY_ID, EXTRA_TEXT_PROPERTY_ID]},
							dataType: DataType.Number,
						},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(0)
		})

		it("should filter by both ID and data type with multiple matching results", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {
							id: {
								in: [TEXT_PROPERTY_ID, EXTRA_TEXT_PROPERTY_ID, NUMBER_PROPERTY_ID],
							},
							dataType: DataType.Text,
						},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(2)
			const propertyIds = result.map((p) => p.id).sort()
			expect(propertyIds).toEqual([TEXT_PROPERTY_ID, EXTRA_TEXT_PROPERTY_ID].sort())
			expect(result.every((p) => p.dataType === DataType.Text)).toBe(true)
		})

		it("should handle combined filtering with pagination", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {
							id: {
								in: [TEXT_PROPERTY_ID, EXTRA_TEXT_PROPERTY_ID, NUMBER_PROPERTY_ID],
							},
							dataType: DataType.Text,
						},
						limit: 1,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.dataType).toBe(DataType.Text)

			const offsetResult = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {
							id: {
								in: [TEXT_PROPERTY_ID, EXTRA_TEXT_PROPERTY_ID, NUMBER_PROPERTY_ID],
							},
							dataType: DataType.Text,
						},
						limit: 1,
						offset: 1,
					}),
				),
			)

			expect(offsetResult).toHaveLength(1)
			expect(offsetResult[0]?.dataType).toBe(DataType.Text)
			expect(offsetResult[0]?.id).not.toBe(result[0]?.id)
		})
	})

	describe("Pagination", () => {
		it("should respect limit parameter", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({limit: 3, offset: 0})))

			expect(result).toHaveLength(3)
		})

		it("should respect offset parameter", async () => {
			const allResults = await Effect.runPromise(provideDeps(getProperties({limit: 100, offset: 0})))
			const offsetResults = await Effect.runPromise(provideDeps(getProperties({limit: 100, offset: 2})))

			expect(offsetResults).toHaveLength(5) // 7 total - 2 offset

			// Ensure the first two results are not in the offset results
			const allIds = allResults.map((p) => p.id)
			const offsetIds = offsetResults.map((p) => p.id)
			expect(offsetIds).not.toContain(allIds[0])
			expect(offsetIds).not.toContain(allIds[1])
		})

		it("should handle limit and offset together", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({limit: 2, offset: 1})))

			expect(result).toHaveLength(2)
		})

		it("should handle limit and offset with filtering", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Text},
						limit: 1,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.dataType).toBe(DataType.Text)

			const offsetResult = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Text},
						limit: 1,
						offset: 1,
					}),
				),
			)

			expect(offsetResult).toHaveLength(1)
			expect(offsetResult[0]?.dataType).toBe(DataType.Text)
			expect(offsetResult[0]?.id).not.toBe(result[0]?.id)
		})
	})

	describe("Default Values", () => {
		it("should use default limit when not provided", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({offset: 0})))

			// Should return all 7 properties since default limit is 100
			expect(result).toHaveLength(7)
		})

		it("should use default offset when not provided", async () => {
			const resultWithExplicitOffset = await Effect.runPromise(
				provideDeps(getProperties({limit: 100, offset: 0})),
			)
			const resultWithDefaultOffset = await Effect.runPromise(provideDeps(getProperties({limit: 100})))

			expect(resultWithDefaultOffset).toEqual(resultWithExplicitOffset)
		})
	})

	describe("Edge Cases", () => {
		it("should handle limit of 0", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({limit: 0, offset: 0})))

			expect(result).toHaveLength(0)
		})

		it("should handle large offset", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({limit: 100, offset: 1000})))

			expect(result).toHaveLength(0)
		})

		it("should handle no filter", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({limit: 100, offset: 0})))

			expect(result).toHaveLength(7)
		})

		it("should handle empty filter object", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(result).toHaveLength(7)
		})

		it("should handle limit larger than available properties", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({limit: 1000, offset: 0})))

			expect(result).toHaveLength(7)
		})

		it("should handle offset equal to total count", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({limit: 100, offset: 7})))

			expect(result).toHaveLength(0)
		})
	})

	describe("Data Integrity", () => {
		it("should return properties with correct structure", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties({limit: 1, offset: 0})))

			expect(result).toHaveLength(1)
			expect(result[0]).toHaveProperty("id")
			expect(result[0]).toHaveProperty("dataType")
			expect(typeof result[0]?.id).toBe("string")
			expect(Object.values(DataType)).toContain(result[0]?.dataType)
		})

		it("should maintain consistency across multiple queries", async () => {
			const result1 = await Effect.runPromise(provideDeps(getProperties({limit: 100, offset: 0})))
			const result2 = await Effect.runPromise(provideDeps(getProperties({limit: 100, offset: 0})))

			expect(result1).toEqual(result2)
		})

		it("should maintain filter consistency", async () => {
			const textProps1 = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Text},
						limit: 100,
						offset: 0,
					}),
				),
			)
			const textProps2 = await Effect.runPromise(
				provideDeps(
					getProperties({
						filter: {dataType: DataType.Text},
						limit: 100,
						offset: 0,
					}),
				),
			)

			expect(textProps1).toEqual(textProps2)
			expect(textProps1.every((p) => p.dataType === DataType.Text)).toBe(true)
		})
	})
})
