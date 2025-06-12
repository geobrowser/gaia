import {Effect, Layer} from "effect"
import {v4 as uuid} from "uuid"
import {afterEach, beforeEach, describe, expect, it} from "vitest"
import {SpaceType} from "../generated/graphql"
import {getSpace, getSpaces} from "../kg/resolvers/spaces"
import {Environment, make as makeEnvironment} from "../services/environment"
import {spaces} from "../services/storage/schema"
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

	beforeEach(async () => {
		// Generate fresh UUIDs for each test to ensure isolation
		PERSONAL_SPACE_ID = uuid()
		PUBLIC_SPACE_ID = uuid()
		COMPLETE_SPACE_ID = uuid()

		await Effect.runPromise(
			provideDeps(
				Effect.gen(function* () {
					const db = yield* Storage

					yield* db.use(async (client) => {
						// Clear existing test data
						await client.delete(spaces)

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
						await client.delete(spaces)
					})
				}),
			),
		)
	})

	describe("getSpaces - Get All Spaces", () => {
		it("should return all spaces", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces))

			expect(result).toHaveLength(3)
			const spaceIds = result.map((s) => s.id).sort()
			const expectedIds = [PERSONAL_SPACE_ID, PUBLIC_SPACE_ID, COMPLETE_SPACE_ID].sort()
			expect(spaceIds).toEqual(expectedIds)
		})

		it("should return correct space types", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces))

			const spaceMap = new Map(result.map((s) => [s.id, s]))

			expect(spaceMap.get(PERSONAL_SPACE_ID)?.type).toBe(SpaceType.Personal)
			expect(spaceMap.get(PUBLIC_SPACE_ID)?.type).toBe(SpaceType.Public)
			expect(spaceMap.get(COMPLETE_SPACE_ID)?.type).toBe(SpaceType.Public)
		})

		it("should return all required fields", async () => {
			const result = await Effect.runPromise(provideDeps(getSpaces))

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
			const result = await Effect.runPromise(provideDeps(getSpaces))

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

			const result = await Effect.runPromise(provideDeps(getSpaces))

			expect(result).toHaveLength(0)
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
			const result1 = await Effect.runPromise(provideDeps(getSpaces))
			const result2 = await Effect.runPromise(provideDeps(getSpaces))

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
			const result = await Effect.runPromise(provideDeps(getSpaces))

			// Verify that database enum values are correctly mapped to GraphQL enum values
			for (const space of result) {
				expect([SpaceType.Personal, SpaceType.Public]).toContain(space.type)
			}

			const personalSpace = result.find((s) => s.id === PERSONAL_SPACE_ID)
			const publicSpace = result.find((s) => s.id === PUBLIC_SPACE_ID)

			expect(personalSpace?.type).toBe("PERSONAL")
			expect(publicSpace?.type).toBe("PUBLIC")
		})
	})

	describe("Edge Cases", () => {
		it("should handle empty string ID with database error", async () => {
			// PostgreSQL will throw a UUID validation error for empty strings
			await expect(Effect.runPromise(provideDeps(getSpace("")))).rejects.toThrow()
		})

		it("should handle null ID gracefully", async () => {
			// TypeScript would prevent this, but testing runtime behavior
			const result = await Effect.runPromise(provideDeps(getSpace(null as any)))

			expect(result).toBeNull()
		})

		it("should handle undefined ID gracefully", async () => {
			// TypeScript would prevent this, but testing runtime behavior
			const result = await Effect.runPromise(provideDeps(getSpace(undefined as any)))

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
			const result = await Effect.runPromise(provideDeps(getSpaces))

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
			const result = await Effect.runPromise(provideDeps(getSpaces))

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
			const result = await Effect.runPromise(provideDeps(getSpaces))

			const validTypes = [SpaceType.Personal, SpaceType.Public]
			for (const space of result) {
				expect(validTypes).toContain(space.type)
			}
		})
	})

	describe("Performance", () => {
		it("should handle multiple concurrent getSpaces calls", async () => {
			const promises = Array.from({length: 10}, () => Effect.runPromise(provideDeps(getSpaces)))

			const results = await Promise.all(promises)

			// All results should be identical
			for (let i = 1; i < results.length; i++) {
				expect(results[i]).toEqual(results[0])
			}
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
})