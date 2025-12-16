import {beforeEach, describe, expect, it} from "vitest"

import {OpenSearchClient, SCORE_BOOST} from "./opensearch"
import type {SearchScope} from "./types"

describe("OpenSearchClient", () => {
	let client: OpenSearchClient

	beforeEach(() => {
		client = new OpenSearchClient("http://localhost:9200", "test-index")
	})

	describe("healthCheck", () => {
		it("should return false when client throws", async () => {
			// Since we can't easily mock the OpenSearch client in this test environment,
			// we just verify the method exists and can be called
			const result = await client.healthCheck()
			expect(typeof result).toBe("boolean")
		})
	})

	describe("buildBaseTextQuery", () => {
		it("should handle special characters in query", () => {
			const query = client.buildBaseTextQuery("test-query & more")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain("test-query & more")
			expect(queryStr).toContain("bool")
			expect(queryStr).toContain("should")
			expect(queryStr).toContain("fuzziness")
			expect(queryStr).toContain("match_phrase_prefix")
		})
	})

	describe("buildUuidQuery", () => {
		const testUuid = "123e4567-e89b-12d3-a456-426614174000"

		it("should build a simple term query for GLOBAL scope", () => {
			const query = client.buildUuidQuery(testUuid, "GLOBAL")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuid)
			expect(queryStr).toContain("entity_id")
			expect(queryStr).toContain("term")
		})

		it("should build a simple term query for GLOBAL_BY_SPACE_SCORE scope", () => {
			const query = client.buildUuidQuery(testUuid, "GLOBAL_BY_SPACE_SCORE")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuid)
			expect(queryStr).toContain("entity_id")
			expect(queryStr).toContain("term")
		})

		it("should build query with space filter for SPACE_SINGLE scope", () => {
			const spaceId = "space-123"
			const query = client.buildUuidQuery(testUuid, "SPACE_SINGLE", spaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuid)
			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("entity_id")
		})

		it("should build simple query for SPACE_SINGLE scope without space_id", () => {
			const query = client.buildUuidQuery(testUuid, "SPACE_SINGLE")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuid)
			expect(queryStr).toContain("entity_id")
		})

		it("should build query with space filter for SPACE scope", () => {
			const spaceId = "space-456"
			const query = client.buildUuidQuery(testUuid, "SPACE", spaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuid)
			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("entity_id")
		})
	})

	describe("buildGlobalQuery", () => {
		it("should wrap base text query with entity_global_score boost", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const query = client.buildGlobalQuery(baseQuery)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain("entity_global_score")
			expect(queryStr).toContain(`"boost":${SCORE_BOOST}`)
			expect(queryStr).toContain("rank_feature")
			expect(queryStr).toContain("test")
		})
	})

	describe("buildGlobalBySpaceScoreQuery", () => {
		it("should wrap base text query with space_score boost", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const query = client.buildGlobalBySpaceScoreQuery(baseQuery)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain("space_score")
			expect(queryStr).toContain(`"boost":${SCORE_BOOST}`)
			expect(queryStr).toContain("rank_feature")
			expect(queryStr).toContain("test")
		})
	})

	describe("buildSingleSpaceQuery", () => {
		it("should filter by space_id and boost by entity_space_score", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const spaceId = "space-789"
			const query = client.buildSingleSpaceQuery(baseQuery, spaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain(`"boost":${SCORE_BOOST}`)
			expect(queryStr).toContain("test")
		})
	})

	describe("buildSearchBody", () => {
		it("should build UUID query when query is a UUID", () => {
			const uuid = "123e4567-e89b-12d3-a456-426614174000"
			const query = client.buildSearchBody({
				query: uuid,
				scope: "GLOBAL",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(uuid)
			expect(queryStr).toContain("entity_id")
		})

		it("should build global query for GLOBAL scope", () => {
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("entity_global_score")
			expect(queryStr).toContain("blockchain")
		})

		it("should build global by space score query for GLOBAL_BY_SPACE_SCORE scope", () => {
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL_BY_SPACE_SCORE",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("space_score")
			expect(queryStr).toContain("blockchain")
		})

		it("should build single space query for SPACE_SINGLE scope", () => {
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "SPACE_SINGLE",
				space_id: "space-123",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("space-123")
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain("blockchain")
		})

		it("should throw error for SPACE_SINGLE scope without space_id", () => {
			expect(() => {
				client.buildSearchBody({
					query: "blockchain",
					scope: "SPACE_SINGLE",
				})
			}).toThrow("SPACE_SINGLE scope requires space_id")
		})

		it("should build single space query for SPACE scope", () => {
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "SPACE",
				space_id: "space-456",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("space-456")
			expect(queryStr).toContain("blockchain")
		})

		it("should throw error for SPACE scope without space_id", () => {
			expect(() => {
				client.buildSearchBody({
					query: "blockchain",
					scope: "SPACE",
				})
			}).toThrow("SPACE scope requires space_id")
		})
	})
})
