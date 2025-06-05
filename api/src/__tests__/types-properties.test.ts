import {SystemIds} from "@graphprotocol/grc-20"
import {Effect, Layer} from "effect"
import {v4 as uuid} from "uuid"
import {afterEach, beforeEach, describe, expect, it} from "vitest"
import {DataType} from "../generated/graphql"
import {getProperties} from "../kg/resolvers/properties"
import {getTypes} from "../kg/resolvers/types"
import {Environment, make as makeEnvironment} from "../services/environment"
import {entities, properties, relations, values} from "../services/storage/schema"
import {Storage, make as makeStorage} from "../services/storage/storage"

// Set up Effect layers like in the main application
const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

describe("Types and Properties Integration Tests", () => {
	// Test data variables - will be regenerated for each test
	let TEST_SPACE_ID: string
	let TEST_SPACE_ID_2: string
	let TYPE_ID_1: string
	let TYPE_ID_2: string
	let TYPE_ID_3: string
	let PROPERTY_ID_1: string
	let PROPERTY_ID_2: string
	let PROPERTY_ID_3: string
	let PROPERTY_ID_4: string

	beforeEach(async () => {
		// Generate fresh UUIDs for each test to ensure isolation
		TEST_SPACE_ID = uuid()
		TEST_SPACE_ID_2 = uuid()
		TYPE_ID_1 = uuid()
		TYPE_ID_2 = uuid()
		TYPE_ID_3 = uuid()
		PROPERTY_ID_1 = uuid()
		PROPERTY_ID_2 = uuid()
		PROPERTY_ID_3 = uuid()
		PROPERTY_ID_4 = uuid()
		await Effect.runPromise(
			provideDeps(
				Effect.gen(function* () {
					const db = yield* Storage

					yield* db.use(async (client) => {
						// Clear existing test data
						await client.delete(relations)
						await client.delete(values)
						await client.delete(entities)
						await client.delete(properties)

						// Insert test entities (types)
						await client.insert(entities).values([
							{
								id: TYPE_ID_1,
								createdAt: "2023-01-01T00:00:00Z",
								createdAtBlock: "1",
								updatedAt: "2023-01-01T00:00:00Z",
								updatedAtBlock: "1",
							},
							{
								id: TYPE_ID_2,
								createdAt: "2023-01-01T00:00:00Z",
								createdAtBlock: "1",
								updatedAt: "2023-01-01T00:00:00Z",
								updatedAtBlock: "1",
							},
							{
								id: TYPE_ID_3,
								createdAt: "2023-01-01T00:00:00Z",
								createdAtBlock: "1",
								updatedAt: "2023-01-01T00:00:00Z",
								updatedAtBlock: "1",
							},
						])

						// Insert test properties
						await client.insert(properties).values([
							{id: PROPERTY_ID_1, type: "Text"},
							{id: PROPERTY_ID_2, type: "Number"},
							{id: PROPERTY_ID_3, type: "Checkbox"},
							{id: PROPERTY_ID_4, type: "Point"},
						])

						// Insert relations to mark entities as types
						await client.insert(relations).values([
							{
								id: uuid(),
								entityId: TYPE_ID_1,
								typeId: SystemIds.TYPES_PROPERTY,
								fromEntityId: TYPE_ID_1,
								toEntityId: SystemIds.SCHEMA_TYPE,
								spaceId: TEST_SPACE_ID,
							},
							{
								id: uuid(),
								entityId: TYPE_ID_2,
								typeId: SystemIds.TYPES_PROPERTY,
								fromEntityId: TYPE_ID_2,
								toEntityId: SystemIds.SCHEMA_TYPE,
								spaceId: TEST_SPACE_ID,
							},
							{
								id: uuid(),
								entityId: TYPE_ID_3,
								typeId: SystemIds.TYPES_PROPERTY,
								fromEntityId: TYPE_ID_3,
								toEntityId: SystemIds.SCHEMA_TYPE,
								spaceId: TEST_SPACE_ID_2,
							},
						])

						// Insert property relations
						await client.insert(relations).values([
							{
								id: uuid(),
								entityId: TYPE_ID_1,
								typeId: SystemIds.PROPERTIES,
								fromEntityId: TYPE_ID_1,
								toEntityId: PROPERTY_ID_1,
								spaceId: TEST_SPACE_ID,
							},
							{
								id: uuid(),
								entityId: TYPE_ID_1,
								typeId: SystemIds.PROPERTIES,
								fromEntityId: TYPE_ID_1,
								toEntityId: PROPERTY_ID_2,
								spaceId: TEST_SPACE_ID,
							},
							{
								id: uuid(),
								entityId: TYPE_ID_2,
								typeId: SystemIds.PROPERTIES,
								fromEntityId: TYPE_ID_2,
								toEntityId: PROPERTY_ID_3,
								spaceId: TEST_SPACE_ID,
							},
							{
								id: uuid(),
								entityId: TYPE_ID_3,
								typeId: SystemIds.PROPERTIES,
								fromEntityId: TYPE_ID_3,
								toEntityId: PROPERTY_ID_4,
								spaceId: TEST_SPACE_ID_2,
							},
						])

						// Insert type names and descriptions
						await client.insert(values).values([
							{
								id: uuid(),
								propertyId: SystemIds.NAME_PROPERTY,
								entityId: TYPE_ID_1,
								spaceId: TEST_SPACE_ID,
								value: "User Type",
							},
							{
								id: uuid(),
								propertyId: SystemIds.DESCRIPTION_PROPERTY,
								entityId: TYPE_ID_1,
								spaceId: TEST_SPACE_ID,
								value: "A user entity type",
							},
							{
								id: uuid(),
								propertyId: SystemIds.NAME_PROPERTY,
								entityId: TYPE_ID_2,
								spaceId: TEST_SPACE_ID,
								value: "Product Type",
							},
							{
								id: uuid(),
								propertyId: SystemIds.DESCRIPTION_PROPERTY,
								entityId: TYPE_ID_2,
								spaceId: TEST_SPACE_ID,
								value: "A product entity type",
							},
							{
								id: uuid(),
								propertyId: SystemIds.NAME_PROPERTY,
								entityId: TYPE_ID_3,
								spaceId: TEST_SPACE_ID_2,
								value: "Order Type",
							},
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
						await client.delete(relations)
						await client.delete(values)
						await client.delete(entities)
						await client.delete(properties)
					})
				}),
			),
		)
	})

	describe("Types Query", () => {
		it("should return all types without spaceId filter", async () => {
			const result = await Effect.runPromise(provideDeps(getTypes({limit: 10, offset: 0})))

			expect(result).toHaveLength(3)
			expect(result.map((r) => r.id).sort()).toEqual([TYPE_ID_1, TYPE_ID_2, TYPE_ID_3].sort())
		})

		it("should filter types by spaceId", async () => {
			const result = await Effect.runPromise(
				provideDeps(getTypes({limit: 10, offset: 0, spaceId: TEST_SPACE_ID})),
			)

			expect(result).toHaveLength(2)
			expect(result.map((r) => r.id).sort()).toEqual([TYPE_ID_1, TYPE_ID_2].sort())
		})

		it("should filter types by different spaceId", async () => {
			const result = await Effect.runPromise(
				provideDeps(getTypes({limit: 10, offset: 0, spaceId: TEST_SPACE_ID_2})),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe(TYPE_ID_3)
		})

		it("should return empty array for non-existent spaceId", async () => {
			const result = await Effect.runPromise(provideDeps(getTypes({limit: 10, offset: 0, spaceId: uuid()})))

			expect(result).toHaveLength(0)
		})

		it("should include name and description from values", async () => {
			const result = await Effect.runPromise(
				provideDeps(getTypes({limit: 10, offset: 0, spaceId: TEST_SPACE_ID})),
			)

			const userType = result.find((r) => r.id === TYPE_ID_1)
			const productType = result.find((r) => r.id === TYPE_ID_2)

			expect(userType).toEqual({
				id: TYPE_ID_1,
				name: "User Type",
				description: "A user entity type",
			})

			expect(productType).toEqual({
				id: TYPE_ID_2,
				name: "Product Type",
				description: "A product entity type",
			})
		})

		it("should handle missing name or description", async () => {
			const result = await Effect.runPromise(
				provideDeps(getTypes({limit: 10, offset: 0, spaceId: TEST_SPACE_ID_2})),
			)

			const orderType = result.find((r) => r.id === TYPE_ID_3)
			expect(orderType).toEqual({
				id: TYPE_ID_3,
				name: "Order Type",
				description: undefined,
			})
		})

		it("should respect limit parameter", async () => {
			const result = await Effect.runPromise(provideDeps(getTypes({limit: 1, offset: 0})))

			expect(result).toHaveLength(1)
		})

		it("should respect offset parameter", async () => {
			const allResults = await Effect.runPromise(provideDeps(getTypes({limit: 10, offset: 0})))

			const offsetResults = await Effect.runPromise(provideDeps(getTypes({limit: 10, offset: 1})))

			expect(offsetResults).toHaveLength(2)
			expect(offsetResults.map((r) => r.id)).not.toContain(allResults[0]?.id)
		})

		it("should handle limit and offset together", async () => {
			const result = await Effect.runPromise(provideDeps(getTypes({limit: 1, offset: 1})))

			expect(result).toHaveLength(1)
		})
	})

	describe("Properties Child Query", () => {
		it("should return properties for a type", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties(TYPE_ID_1, {limit: 10, offset: 0})))

			expect(result).toHaveLength(2)
			expect(result.map((r) => r.id).sort()).toEqual([PROPERTY_ID_1, PROPERTY_ID_2].sort())
		})

		it("should return correct data types", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties(TYPE_ID_1, {limit: 10, offset: 0})))

			const textProperty = result.find((r) => r.id === PROPERTY_ID_1)
			const numberProperty = result.find((r) => r.id === PROPERTY_ID_2)

			expect(textProperty?.dataType).toBe(DataType.Text)
			expect(numberProperty?.dataType).toBe(DataType.Number)
		})

		it("should return different property types", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties(TYPE_ID_2, {limit: 10, offset: 0})))

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe(PROPERTY_ID_3)
			expect(result[0]?.dataType).toBe(DataType.Checkbox)
		})

		it("should filter properties by spaceId", async () => {
			const result = await Effect.runPromise(
				provideDeps(getProperties(TYPE_ID_1, {limit: 10, offset: 0, spaceId: TEST_SPACE_ID})),
			)

			expect(result).toHaveLength(2)
			expect(result.map((r) => r.id).sort()).toEqual([PROPERTY_ID_1, PROPERTY_ID_2].sort())
		})

		it("should return empty array for different spaceId", async () => {
			const result = await Effect.runPromise(
				provideDeps(getProperties(TYPE_ID_1, {limit: 10, offset: 0, spaceId: TEST_SPACE_ID_2})),
			)

			expect(result).toHaveLength(0)
		})

		it("should return properties for type in different space", async () => {
			const result = await Effect.runPromise(
				provideDeps(getProperties(TYPE_ID_3, {limit: 10, offset: 0, spaceId: TEST_SPACE_ID_2})),
			)

			expect(result).toHaveLength(1)
			expect(result[0]?.id).toBe(PROPERTY_ID_4)
			expect(result[0]?.dataType).toBe(DataType.Point)
		})

		it("should return empty array for non-existent type", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties(uuid(), {limit: 10, offset: 0})))

			expect(result).toHaveLength(0)
		})

		it("should return empty array for type with no properties", async () => {
			// Create a type with no properties
			const emptyTypeId = uuid()
			await Effect.runPromise(
				provideDeps(
					Effect.gen(function* () {
						const db = yield* Storage
						yield* db.use(async (client) => {
							await client.insert(entities).values({
								id: emptyTypeId,
								createdAt: "2023-01-01T00:00:00Z",
								createdAtBlock: "1",
								updatedAt: "2023-01-01T00:00:00Z",
								updatedAtBlock: "1",
							})

							await client.insert(relations).values({
								id: uuid(),
								entityId: emptyTypeId,
								typeId: SystemIds.TYPES_PROPERTY,
								fromEntityId: emptyTypeId,
								toEntityId: SystemIds.SCHEMA_TYPE,
								spaceId: TEST_SPACE_ID,
							})
						})
					}),
				),
			)

			const result = await Effect.runPromise(provideDeps(getProperties(emptyTypeId, {limit: 10, offset: 0})))

			expect(result).toHaveLength(0)
		})

		it("should handle non-existent spaceId", async () => {
			const result = await Effect.runPromise(
				provideDeps(getProperties(TYPE_ID_1, {limit: 10, offset: 0, spaceId: uuid()})),
			)

			expect(result).toHaveLength(0)
		})
	})

	describe("Data Type Mapping", () => {
		it("should map all supported data types correctly", async () => {
			// We have Text, Number, Checkbox, Point in our test data
			const type1Props = await Effect.runPromise(provideDeps(getProperties(TYPE_ID_1, {limit: 10, offset: 0})))

			const type2Props = await Effect.runPromise(provideDeps(getProperties(TYPE_ID_2, {limit: 10, offset: 0})))

			const type3Props = await Effect.runPromise(
				provideDeps(getProperties(TYPE_ID_3, {limit: 10, offset: 0, spaceId: TEST_SPACE_ID_2})),
			)

			const allProps = [...type1Props, ...type2Props, ...type3Props]
			const dataTypes = allProps.map((p) => p.dataType).sort()

			expect(dataTypes).toEqual([DataType.Checkbox, DataType.Number, DataType.Point, DataType.Text])
		})

		it("should map Time and Relation data types correctly", async () => {
			// Insert properties with Time and Relation types that aren't in our default test data
			const timePropId = uuid()
			const relationPropId = uuid()
			await Effect.runPromise(
				provideDeps(
					Effect.gen(function* () {
						const db = yield* Storage
						yield* db.use(async (client) => {
							await client.insert(properties).values([
								{
									id: timePropId,
									type: "Time",
								},
								{
									id: relationPropId,
									type: "Relation",
								},
							])

							await client.insert(relations).values([
								{
									id: uuid(),
									entityId: TYPE_ID_1,
									typeId: SystemIds.PROPERTIES,
									fromEntityId: TYPE_ID_1,
									toEntityId: timePropId,
									spaceId: TEST_SPACE_ID,
								},
								{
									id: uuid(),
									entityId: TYPE_ID_1,
									typeId: SystemIds.PROPERTIES,
									fromEntityId: TYPE_ID_1,
									toEntityId: relationPropId,
									spaceId: TEST_SPACE_ID,
								},
							])
						})
					}),
				),
			)

			const result = await Effect.runPromise(provideDeps(getProperties(TYPE_ID_1, {limit: 10, offset: 0})))

			const timeProp = result.find((p) => p.id === timePropId)
			const relationProp = result.find((p) => p.id === relationPropId)

			expect(timeProp?.dataType).toBe(DataType.Time)
			expect(relationProp?.dataType).toBe(DataType.Relation)
		})
	})

	describe("Edge Cases", () => {
		it("should handle limit of 0", async () => {
			const result = await Effect.runPromise(provideDeps(getTypes({limit: 0, offset: 0})))

			expect(result).toHaveLength(0)
		})

		it("should handle large offset", async () => {
			const result = await Effect.runPromise(provideDeps(getTypes({limit: 10, offset: 1000})))

			expect(result).toHaveLength(0)
		})

		it("should handle properties query with limit 0", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties(TYPE_ID_1, {limit: 0, offset: 0})))

			expect(result).toHaveLength(0)
		})

		it("should handle properties query with large offset", async () => {
			const result = await Effect.runPromise(provideDeps(getProperties(TYPE_ID_1, {limit: 10, offset: 1000})))

			expect(result).toHaveLength(0)
		})
	})
})
