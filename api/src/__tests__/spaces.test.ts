import {SystemIds} from "@graphprotocol/grc-20"
import {Effect, Layer} from "effect"
import {v4 as uuid} from "uuid"
import {afterEach, beforeEach, describe, expect, it} from "vitest"
import {SpaceType} from "../generated/graphql"
import {getSpace, getSpaces, getSpaceEntity} from "../kg/resolvers/spaces"
import {Environment, make as makeEnvironment} from "../services/environment"
import {entities, relations, spaces} from "../services/storage/schema"
import {Storage, make as makeStorage} from "../services/storage/storage"

// Set up Effect layers like in the main application
const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

describe("Spaces Query Integration Tests", () => {
	// Test data variables - will be regenerated for each test
	let PERSONAL_SPACE_ID: string
	let PUBLIC_SPACE_ID: string
	let COMPLETE_SPACE_ID: string
	let SPACE_ENTITY_ID: string
	let SPACE_ENTITY_ID_2: string
	let NON_SPACE_ENTITY_ID: string

	beforeEach(async () => {
		// Generate fresh UUIDs for each test to ensure isolation
		PERSONAL_SPACE_ID = uuid()
		PUBLIC_SPACE_ID = uuid()
		COMPLETE_SPACE_ID = uuid()
		SPACE_ENTITY_ID = uuid()
		SPACE_ENTITY_ID_2 = uuid()
		NON_SPACE_ENTITY_ID = uuid()

		await Effect.runPromise(
			provideDeps(
				Effect.gen(function* () {
					const db = yield* Storage

					yield* db.use(async (client) => {
						// Clear existing test data
						await client.delete(relations)
						await client.delete(entities)
						await client.delete(spaces)

						// Insert test entities
						await client.insert(entities).values([
							{
								id: SPACE_ENTITY_ID,
								createdAt: "2024-01-01T00:00:00Z",
								createdAtBlock: "1000000",
								updatedAt: "2024-01-01T00:00:00Z",
								updatedAtBlock: "1000000",
							},
							{
								id: SPACE_ENTITY_ID_2,
								createdAt: "2024-01-01T00:00:00Z",
								createdAtBlock: "1000001",
								updatedAt: "2024-01-01T00:00:00Z",
								updatedAtBlock: "1000001",
							},
							{
								id: NON_SPACE_ENTITY_ID,
								createdAt: "2024-01-01T00:00:00Z",
								createdAtBlock: "1000002",
								updatedAt: "2024-01-01T00:00:00Z",
								updatedAtBlock: "1000002",
							},
						])

						// Insert test spaces with different configurations
						await client.insert(spaces).values([
							{
								id: PERSONAL_SPACE_ID,
								type: "Personal",
								daoAddress: "0x1234567890123456789012345678901234567890",
								spaceAddress: "0x1111111111111111111111111111111111111111",
								mainVotingAddress: null,
								membershipAddress: null,
								personalAddress: "0x2222222222222222222222222222222222222222",
							},
							{
								id: PUBLIC_SPACE_ID,
								type: "Public",
								daoAddress: "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
								spaceAddress: "0x3333333333333333333333333333333333333333",
								mainVotingAddress: "0x4444444444444444444444444444444444444444",
								membershipAddress: "0x5555555555555555555555555555555555555555",
								personalAddress: null,
							},
							{
								id: COMPLETE_SPACE_ID,
								type: "Public",
								daoAddress: "0xfedcbafedcbafedcbafedcbafedcbafedcbafedcb",
								spaceAddress: "0x6666666666666666666666666666666666666666",
								mainVotingAddress: "0x7777777777777777777777777777777777777777",
								membershipAddress: "0x8888888888888888888888888888888888888888",
								personalAddress: "0x9999999999999999999999999999999999999999",
							},
						])

						// Insert test relations - linking spaces to entities with SPACE_TYPE
						await client.insert(relations).values([
							{
								id: uuid(),
								entityId: uuid(), // Relation entity ID
								spaceId: PERSONAL_SPACE_ID,
								typeId: SystemIds.SPACE_TYPE,
								fromEntityId: PERSONAL_SPACE_ID,
								toEntityId: SPACE_ENTITY_ID,
								toSpaceId: PERSONAL_SPACE_ID,
								verified: true,
							},
							{
								id: uuid(),
								entityId: uuid(), // Relation entity ID
								spaceId: PUBLIC_SPACE_ID,
								typeId: SystemIds.SPACE_TYPE,
								fromEntityId: PUBLIC_SPACE_ID,
								toEntityId: SPACE_ENTITY_ID_2,
								toSpaceId: PUBLIC_SPACE_ID,
								verified: true,
							},
							// Add a non-SPACE_TYPE relation for testing
							{
								id: uuid(),
								entityId: uuid(), // Relation entity ID
								spaceId: COMPLETE_SPACE_ID,
								typeId: uuid(), // Not SPACE_TYPE
								fromEntityId: COMPLETE_SPACE_ID,
								toEntityId: NON_SPACE_ENTITY_ID,
								toSpaceId: COMPLETE_SPACE_ID,
								verified: false,
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
						await client.delete(entities)
						await client.delete(spaces)
					})
				}),
			),
		)
	})

	describe("getSpaces - Get All Spaces", () => {
		it("should return all spaces", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces({})))

			expect(result).toHaveLength(3)
			const spaceIds = result.map((s) => s.id).sort()
			const expectedIds = [PERSONAL_SPACE_ID, PUBLIC_SPACE_ID, COMPLETE_SPACE_ID].sort()
			expect(spaceIds).toEqual(expectedIds)
		})

		it("should return correct space types", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces({})))

			const spaceMap = new Map(result.map((s) => [s.id, s]))

			expect(spaceMap.get(PERSONAL_SPACE_ID)?.type).toBe(SpaceType.Personal)
			expect(spaceMap.get(PUBLIC_SPACE_ID)?.type).toBe(SpaceType.Public)
			expect(spaceMap.get(COMPLETE_SPACE_ID)?.type).toBe(SpaceType.Public)
		})

		it("should return all required fields", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces({})))

			for (const space of result) {
				expect(space).toHaveProperty("id")
				expect(space).toHaveProperty("type")
				expect(space).toHaveProperty("daoAddress")
				expect(space).toHaveProperty("spaceAddress")
				expect(space).toHaveProperty("mainVotingAddress")
				expect(space).toHaveProperty("membershipAddress")
				expect(space).toHaveProperty("personalAddress")

				expect(typeof space.id).toBe("string")
				expect(Object.values(SpaceType)).toContain(space.type)
				expect(typeof space.daoAddress).toBe("string")
				expect(typeof space.spaceAddress).toBe("string")
			}
		})

		it("should handle optional fields correctly", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces({})))

			const personalSpace = result.find((s) => s.id === PERSONAL_SPACE_ID)
			const publicSpace = result.find((s) => s.id === PUBLIC_SPACE_ID)
			const completeSpace = result.find((s) => s.id === COMPLETE_SPACE_ID)

			// Personal space should have personalAddress but not voting/membership
			expect(personalSpace?.personalAddress).toBe("0x2222222222222222222222222222222222222222")
			expect(personalSpace?.mainVotingAddress).toBeNull()
			expect(personalSpace?.membershipAddress).toBeNull()

			// Public space should have voting/membership but not personal
			expect(publicSpace?.mainVotingAddress).toBe("0x4444444444444444444444444444444444444444")
			expect(publicSpace?.membershipAddress).toBe("0x5555555555555555555555555555555555555555")
			expect(publicSpace?.personalAddress).toBeNull()

			// Complete space should have all optional fields
			expect(completeSpace?.mainVotingAddress).toBe("0x7777777777777777777777777777777777777777")
			expect(completeSpace?.membershipAddress).toBe("0x8888888888888888888888888888888888888888")
			expect(completeSpace?.personalAddress).toBe("0x9999999999999999999999999999999999999999")
		})

		it("should return empty array when no spaces exist", async () => {
			// Clear all spaces
			await Effect.runPromise(
				provideDeps(
					Effect.gen(function* () {
						const db = yield* Storage
						yield* db.use(async (client) => {
							await client.delete(spaces)
						})
					}),
				),
			)

			const result = await Effect.runPromise(provideDeps(getSpaces({})))

			expect(result).toHaveLength(0)
		})
	})

	describe("getSpaces - Filtering", () => {
		it("should filter by single space ID", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {
							id: {
								in: [PERSONAL_SPACE_ID],
							},
						},
					}),
				),
			)

			expect(result).toHaveLength(1)
			expect(result[0]).toBeDefined()
			expect(result[0]?.id).toBe(PERSONAL_SPACE_ID)
			expect(result[0]?.type).toBe(SpaceType.Personal)
		})

		it("should filter by multiple space IDs", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {
							id: {
								in: [PERSONAL_SPACE_ID, PUBLIC_SPACE_ID],
							},
						},
					}),
				),
			)

			expect(result).toHaveLength(2)
			const resultIds = result.map((s) => s.id).sort()
			const expectedIds = [PERSONAL_SPACE_ID, PUBLIC_SPACE_ID].sort()
			expect(resultIds).toEqual(expectedIds)
		})

		it("should filter by all space IDs", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {
							id: {
								in: [PERSONAL_SPACE_ID, PUBLIC_SPACE_ID, COMPLETE_SPACE_ID],
							},
						},
					}),
				),
			)

			expect(result).toHaveLength(3)
			const resultIds = result.map((s) => s.id).sort()
			const expectedIds = [PERSONAL_SPACE_ID, PUBLIC_SPACE_ID, COMPLETE_SPACE_ID].sort()
			expect(resultIds).toEqual(expectedIds)
		})

		it("should return empty array for non-existent space IDs", async () => {
			const nonExistentId1 = uuid()
			const nonExistentId2 = uuid()

			const result = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {
							id: {
								in: [nonExistentId1, nonExistentId2],
							},
						},
					}),
				),
			)

			expect(result).toHaveLength(0)
		})

		it("should filter correctly with mix of existing and non-existent IDs", async () => {
			const nonExistentId = uuid()

			const result = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {
							id: {
								in: [PERSONAL_SPACE_ID, nonExistentId, PUBLIC_SPACE_ID],
							},
						},
					}),
				),
			)

			expect(result).toHaveLength(2)
			const resultIds = result.map((s) => s.id).sort()
			const expectedIds = [PERSONAL_SPACE_ID, PUBLIC_SPACE_ID].sort()
			expect(resultIds).toEqual(expectedIds)
		})

		it("should return empty array when filter ID array is empty", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {
							id: {
								in: [],
							},
						},
					}),
				),
			)

			expect(result).toHaveLength(0)
		})

		it("should return all spaces when no filter is provided", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {},
					}),
				),
			)

			expect(result).toHaveLength(3)
		})

		it("should return all spaces when filter is undefined", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: undefined,
					}),
				),
			)

			expect(result).toHaveLength(3)
		})

		it("should work with limit and offset when filtering", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {
							id: {
								in: [PERSONAL_SPACE_ID, PUBLIC_SPACE_ID, COMPLETE_SPACE_ID],
							},
						},
						limit: 2,
						offset: 1,
					}),
				),
			)

			expect(result).toHaveLength(2)
			// Should skip the first result and return the next 2
		})

		it("should respect limit when filtering", async () => {
			const result = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {
							id: {
								in: [PERSONAL_SPACE_ID, PUBLIC_SPACE_ID, COMPLETE_SPACE_ID],
							},
						},
						limit: 1,
					}),
				),
			)

			expect(result).toHaveLength(1)
		})

		it("should handle invalid UUID in filter array", async () => {
			// Database should handle invalid UUIDs gracefully in the filter
			await expect(
				Effect.runPromise(
					provideDeps(
						getSpaces({
							filter: {
								id: {
									in: ["invalid-uuid"],
								},
							},
						}),
					),
				),
			).rejects.toThrow()
		})

		it("should maintain data integrity when filtering", async () => {
			const result1 = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {
							id: {
								in: [PERSONAL_SPACE_ID],
							},
						},
					}),
				),
			)

			const result2 = await Effect.runPromise(
				provideDeps(
					getSpaces({
						filter: {
							id: {
								in: [PERSONAL_SPACE_ID],
							},
						},
					}),
				),
			)

			expect(result1).toEqual(result2)
		})
	})

	describe("getSpace - Get Single Space", () => {
		it("should return a specific space by ID", async () => {
			const result = await Effect.runPromise(provideDeps(getSpace(PERSONAL_SPACE_ID)))

			expect(result).not.toBeNull()
			expect(result?.id).toBe(PERSONAL_SPACE_ID)
			expect(result?.type).toBe(SpaceType.Personal)
			expect(result?.daoAddress).toBe("0x1234567890123456789012345678901234567890")
			expect(result?.spaceAddress).toBe("0x1111111111111111111111111111111111111111")
		})

		it("should return correct data for different space types", async () => {
			const personalResult = await Effect.runPromise(provideDeps(getSpace(PERSONAL_SPACE_ID)))
			const publicResult = await Effect.runPromise(provideDeps(getSpace(PUBLIC_SPACE_ID)))

			expect(personalResult?.type).toBe(SpaceType.Personal)
			expect(publicResult?.type).toBe(SpaceType.Public)

			expect(personalResult?.personalAddress).toBe("0x2222222222222222222222222222222222222222")
			expect(personalResult?.mainVotingAddress).toBeNull()

			expect(publicResult?.mainVotingAddress).toBe("0x4444444444444444444444444444444444444444")
			expect(publicResult?.personalAddress).toBeNull()
		})

		it("should return null for non-existent space ID", async () => {
			const nonExistentId = uuid()
			const result = await Effect.runPromise(provideDeps(getSpace(nonExistentId)))

			expect(result).toBeNull()
		})

		it("should handle invalid UUID format with database error", async () => {
			// PostgreSQL will throw a UUID validation error for invalid UUID formats
			await expect(Effect.runPromise(provideDeps(getSpace("invalid-uuid")))).rejects.toThrow()
		})

		it("should return complete space data", async () => {
			const result = await Effect.runPromise(provideDeps(getSpace(COMPLETE_SPACE_ID)))

			expect(result).not.toBeNull()
			expect(result?.id).toBe(COMPLETE_SPACE_ID)
			expect(result?.type).toBe(SpaceType.Public)
			expect(result?.daoAddress).toBe("0xfedcbafedcbafedcbafedcbafedcbafedcbafedcb")
			expect(result?.spaceAddress).toBe("0x6666666666666666666666666666666666666666")
			expect(result?.mainVotingAddress).toBe("0x7777777777777777777777777777777777777777")
			expect(result?.membershipAddress).toBe("0x8888888888888888888888888888888888888888")
			expect(result?.personalAddress).toBe("0x9999999999999999999999999999999999999999")
		})
	})

	describe("Data Integrity", () => {
		it("should maintain consistency across multiple queries", async () => {
			const result1 = await Effect.runPromise(provideDeps(getSpaces({})))
			const result2 = await Effect.runPromise(provideDeps(getSpaces({})))

			expect(result1).toEqual(result2)
		})

		it("should maintain single space query consistency", async () => {
			const result1 = await Effect.runPromise(provideDeps(getSpace(PERSONAL_SPACE_ID)))
			const result2 = await Effect.runPromise(provideDeps(getSpace(PERSONAL_SPACE_ID)))

			expect(result1).toEqual(result2)
		})

		it("should return space with correct structure", async () => {
			const result = await Effect.runPromise(provideDeps(getSpace(PERSONAL_SPACE_ID)))

			expect(result).toHaveProperty("id")
			expect(result).toHaveProperty("type")
			expect(result).toHaveProperty("daoAddress")
			expect(result).toHaveProperty("spaceAddress")
			expect(result).toHaveProperty("mainVotingAddress")
			expect(result).toHaveProperty("membershipAddress")
			expect(result).toHaveProperty("personalAddress")

			expect(typeof result?.id).toBe("string")
			expect(Object.values(SpaceType)).toContain(result?.type)
			expect(typeof result?.daoAddress).toBe("string")
			expect(typeof result?.spaceAddress).toBe("string")
		})

		it("should properly map database enum values to GraphQL enum values", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces({})))

			// Verify that database enum values are correctly mapped to GraphQL enum values
			for (const space of result) {
				expect([SpaceType.Personal, SpaceType.Public]).toContain(space.type)
			}

			const personalSpace = result.find((s) => s.id === PERSONAL_SPACE_ID)
			const publicSpace = result.find((s) => s.id === PUBLIC_SPACE_ID)

			expect(personalSpace?.type).toBe(SpaceType.Personal)
			expect(publicSpace?.type).toBe(SpaceType.Public)
		})
	})

	describe("Edge Cases", () => {
		it("should handle empty string ID with database error", async () => {
			// PostgreSQL will throw a UUID validation error for empty strings
			await expect(Effect.runPromise(provideDeps(getSpace("")))).rejects.toThrow()
		})

		it("should handle null ID gracefully", async () => {
			// TypeScript would prevent this, but testing runtime behavior
			const result = await Effect.runPromise(provideDeps(getSpace(null as unknown as string)))

			expect(result).toBeNull()
		})

		it("should handle undefined ID gracefully", async () => {
			// TypeScript would prevent this, but testing runtime behavior
			const result = await Effect.runPromise(provideDeps(getSpace(undefined as unknown as string)))

			expect(result).toBeNull()
		})

		it("should handle very long ID strings with database error", async () => {
			const veryLongId = "a".repeat(1000)
			// PostgreSQL will throw a UUID validation error for invalid UUID formats
			await expect(Effect.runPromise(provideDeps(getSpace(veryLongId)))).rejects.toThrow()
		})

		it("should handle special characters in ID with database error", async () => {
			const specialCharId = "!@#$%^&*()_+-={}[]|;:,.<>?"
			// PostgreSQL will throw a UUID validation error for invalid UUID formats
			await expect(Effect.runPromise(provideDeps(getSpace(specialCharId)))).rejects.toThrow()
		})
	})

	describe("Database Schema Validation", () => {
		it("should validate required fields are present", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces({})))

			for (const space of result) {
				// Required fields should never be null/undefined
				expect(space.id).toBeTruthy()
				expect(space.type).toBeTruthy()
				expect(space.daoAddress).toBeTruthy()
				expect(space.spaceAddress).toBeTruthy()

				// Optional fields can be null
				// mainVotingAddress, membershipAddress, personalAddress can be null
			}
		})

		it("should validate address field formats", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces({})))

			for (const space of result) {
				// All addresses should be strings when present
				expect(typeof space.daoAddress).toBe("string")
				expect(typeof space.spaceAddress).toBe("string")

				if (space.mainVotingAddress !== null) {
					expect(typeof space.mainVotingAddress).toBe("string")
				}
				if (space.membershipAddress !== null) {
					expect(typeof space.membershipAddress).toBe("string")
				}
				if (space.personalAddress !== null) {
					expect(typeof space.personalAddress).toBe("string")
				}
			}
		})

		it("should validate space type enum values", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces({})))

			const validTypes = [SpaceType.Personal, SpaceType.Public]
			for (const space of result) {
				expect(validTypes).toContain(space.type)
			}
		})
	})

	describe("Performance", () => {
		it("should handle multiple concurrent getSpaces calls", async () => {
			const promises = Array.from({length: 10}, () => Effect.runPromise(provideDeps(getSpaces({}))))

			const results = await Promise.all(promises)

			// All results should be identical
			for (let i = 1; i < results.length; i++) {
				expect(results[i]).toEqual(results[0])
			}
		})

		it("should handle multiple concurrent filtered getSpaces calls", async () => {
			const promises = Array.from({length: 10}, () =>
				Effect.runPromise(
					provideDeps(
						getSpaces({
							filter: {
								id: {
									in: [PERSONAL_SPACE_ID, PUBLIC_SPACE_ID],
								},
							},
						}),
					),
				),
			)

			const results = await Promise.all(promises)

			// All results should be identical
			for (let i = 1; i < results.length; i++) {
				expect(results[i]).toEqual(results[0])
			}

			// Verify the filter worked correctly
			expect(results[0]).toHaveLength(2)
		})

		it("should handle multiple concurrent getSpace calls", async () => {
			const promises = Array.from({length: 10}, () =>
				Effect.runPromise(provideDeps(getSpace(PERSONAL_SPACE_ID))),
			)

			const results = await Promise.all(promises)

			// All results should be identical
			for (let i = 1; i < results.length; i++) {
				expect(results[i]).toEqual(results[0])
			}
		})
	})

	describe("getSpaceEntity - Get Space Entity", () => {
		it("should return the entity associated with a space", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaceEntity(PERSONAL_SPACE_ID)))

			expect(result).not.toBeNull()
			expect(result?.id).toBe(SPACE_ENTITY_ID)
			expect(result?.createdAt).toBe("2024-01-01T00:00:00Z")
			expect(result?.createdAtBlock).toBe("1000000")
			expect(result?.updatedAt).toBe("2024-01-01T00:00:00Z")
			expect(result?.updatedAtBlock).toBe("1000000")
		})

		it("should return the correct entity for different spaces", async () => {
			const personalResult = await Effect.runPromise(provideDeps(getSpaceEntity(PERSONAL_SPACE_ID)))
			const publicResult = await Effect.runPromise(provideDeps(getSpaceEntity(PUBLIC_SPACE_ID)))

			expect(personalResult).not.toBeNull()
			expect(publicResult).not.toBeNull()
			expect(personalResult?.id).toBe(SPACE_ENTITY_ID)
			expect(publicResult?.id).toBe(SPACE_ENTITY_ID_2)
			expect(personalResult?.id).not.toBe(publicResult?.id)
		})

		it("should return null for space without SPACE_TYPE relation", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaceEntity(COMPLETE_SPACE_ID)))

			expect(result).toBeNull()
		})

		it("should return null for non-existent space ID", async () => {
			const nonExistentId = uuid()
			const result = await Effect.runPromise(provideDeps(getSpaceEntity(nonExistentId)))

			expect(result).toBeNull()
		})

		it("should handle invalid UUID format with database error", async () => {
			await expect(Effect.runPromise(provideDeps(getSpaceEntity("invalid-uuid")))).rejects.toThrow()
		})

		it("should return complete entity structure", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaceEntity(PERSONAL_SPACE_ID)))

			expect(result).not.toBeNull()
			expect(result).toHaveProperty("id")
			expect(result).toHaveProperty("createdAt")
			expect(result).toHaveProperty("createdAtBlock")
			expect(result).toHaveProperty("updatedAt")
			expect(result).toHaveProperty("updatedAtBlock")

			expect(typeof result?.id).toBe("string")
			expect(typeof result?.createdAt).toBe("string")
			expect(typeof result?.createdAtBlock).toBe("string")
			expect(typeof result?.updatedAt).toBe("string")
			expect(typeof result?.updatedAtBlock).toBe("string")
		})

		it("should maintain consistency across multiple queries", async () => {
			const result1 = await Effect.runPromise(provideDeps(getSpaceEntity(PERSONAL_SPACE_ID)))
			const result2 = await Effect.runPromise(provideDeps(getSpaceEntity(PERSONAL_SPACE_ID)))

			expect(result1).toEqual(result2)
		})

		it("should handle empty string ID with database error", async () => {
			await expect(Effect.runPromise(provideDeps(getSpaceEntity("")))).rejects.toThrow()
		})

		it("should handle null ID gracefully", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaceEntity(null as unknown as string)))

			expect(result).toBeNull()
		})

		it("should handle undefined ID gracefully", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaceEntity(undefined as unknown as string)))

			expect(result).toBeNull()
		})

		it("should handle very long ID strings with database error", async () => {
			const veryLongId = "a".repeat(1000)
			await expect(Effect.runPromise(provideDeps(getSpaceEntity(veryLongId)))).rejects.toThrow()
		})

		it("should handle special characters in ID with database error", async () => {
			const specialCharId = "!@#$%^&*()_+-={}[]|;:,.<>?"
			await expect(Effect.runPromise(provideDeps(getSpaceEntity(specialCharId)))).rejects.toThrow()
		})

		it("should only return entities with SPACE_TYPE relation", async () => {
			// Verify that the space with non-SPACE_TYPE relation returns null
			const result = await Effect.runPromise(provideDeps(getSpaceEntity(COMPLETE_SPACE_ID)))
			expect(result).toBeNull()

			// Verify that spaces with SPACE_TYPE relation return entities
			const personalResult = await Effect.runPromise(provideDeps(getSpaceEntity(PERSONAL_SPACE_ID)))
			const publicResult = await Effect.runPromise(provideDeps(getSpaceEntity(PUBLIC_SPACE_ID)))
			expect(personalResult).not.toBeNull()
			expect(publicResult).not.toBeNull()
		})

		it("should handle multiple concurrent getSpaceEntity calls", async () => {
			const promises = Array.from({length: 10}, () =>
				Effect.runPromise(provideDeps(getSpaceEntity(PERSONAL_SPACE_ID))),
			)

			const results = await Promise.all(promises)

			// All results should be identical
			for (let i = 1; i < results.length; i++) {
				expect(results[i]).toEqual(results[0])
			}

			// Verify the result is correct
			expect(results[0]).not.toBeNull()
			expect(results[0]?.id).toBe(SPACE_ENTITY_ID)
		})

		it("should validate entity data types", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaceEntity(PERSONAL_SPACE_ID)))

			expect(result).not.toBeNull()
			expect(typeof result?.id).toBe("string")
			expect(typeof result?.createdAt).toBe("string")
			expect(typeof result?.createdAtBlock).toBe("string")
			expect(typeof result?.updatedAt).toBe("string")
			expect(typeof result?.updatedAtBlock).toBe("string")

			// Validate timestamp format (ISO 8601)
			expect(result?.createdAt).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/)
			expect(result?.updatedAt).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/)

			// Validate block numbers are strings with numeric content
			expect(result?.createdAtBlock).toMatch(/^\d+$/)
			expect(result?.updatedAtBlock).toMatch(/^\d+$/)
		})

		it("should handle concurrent requests for different spaces", async () => {
			const promises = [
				Effect.runPromise(provideDeps(getSpaceEntity(PERSONAL_SPACE_ID))),
				Effect.runPromise(provideDeps(getSpaceEntity(PUBLIC_SPACE_ID))),
				Effect.runPromise(provideDeps(getSpaceEntity(COMPLETE_SPACE_ID))),
			]

			const [personalResult, publicResult, completeResult] = await Promise.all(promises)

			expect(personalResult).not.toBeNull()
			expect(publicResult).not.toBeNull()
			expect(completeResult).toBeNull()

			expect(personalResult?.id).toBe(SPACE_ENTITY_ID)
			expect(publicResult?.id).toBe(SPACE_ENTITY_ID_2)
		})
	})
})