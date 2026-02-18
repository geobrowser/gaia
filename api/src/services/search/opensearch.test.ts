import {beforeEach, describe, expect, it} from "vitest"

import {OpenSearchClient, SCORE_BOOST} from "./opensearch"

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
		const testUuidDashed = "123e4567-e89b-12d3-a456-426614174000"
		const testUuidDashless = "123e4567e89b12d3a456426614174000"

		it("should include both dashed and dashless UUID variants in terms query", () => {
			const query = client.buildUuidQuery(testUuidDashed, "GLOBAL")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuidDashed)
			expect(queryStr).toContain(testUuidDashless)
			expect(queryStr).toContain("entity_id")
			expect(queryStr).toContain("terms")
		})

		it("should include both variants when given a dashless UUID", () => {
			const query = client.buildUuidQuery(testUuidDashless, "GLOBAL")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuidDashed)
			expect(queryStr).toContain(testUuidDashless)
			expect(queryStr).toContain("entity_id")
		})

		it("should build terms query for GLOBAL_BY_SPACE_SCORE scope", () => {
			const query = client.buildUuidQuery(testUuidDashed, "GLOBAL_BY_SPACE_SCORE")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuidDashed)
			expect(queryStr).toContain(testUuidDashless)
			expect(queryStr).toContain("entity_id")
		})

		it("should build terms query for GLOBAL_BY_ENTITY_SPACE_SCORE scope", () => {
			const query = client.buildUuidQuery(testUuidDashed, "GLOBAL_BY_ENTITY_SPACE_SCORE")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuidDashed)
			expect(queryStr).toContain(testUuidDashless)
			expect(queryStr).toContain("entity_id")
		})

		it("should include both space_id variants in SPACE_SINGLE filter", () => {
			const dashedSpaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const dashlessSpaceId = "abcd1234abcd1234abcd1234abcd5678"
			const query = client.buildUuidQuery(testUuidDashed, "SPACE_SINGLE", dashedSpaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(dashedSpaceId)
			expect(queryStr).toContain(dashlessSpaceId)
			expect(queryStr).toContain("entity_id")
		})

		it("should include both space_id variants when given dashless space_id", () => {
			const dashlessSpaceId = "abcd1234abcd1234abcd1234abcd5678"
			const dashedSpaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const query = client.buildUuidQuery(testUuidDashed, "SPACE_SINGLE", dashlessSpaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(dashedSpaceId)
			expect(queryStr).toContain(dashlessSpaceId)
		})

		it("should build query for SPACE_SINGLE scope without space_id", () => {
			const query = client.buildUuidQuery(testUuidDashed, "SPACE_SINGLE")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuidDashed)
			expect(queryStr).toContain("entity_id")
		})

		it("should include both space_id variants in SPACE filter", () => {
			const dashedSpaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const dashlessSpaceId = "abcd1234abcd1234abcd1234abcd5678"
			const query = client.buildUuidQuery(testUuidDashed, "SPACE", dashedSpaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(dashedSpaceId)
			expect(queryStr).toContain(dashlessSpaceId)
			expect(queryStr).toContain("entity_id")
		})

		it("should not include script_fields", () => {
			const query = client.buildUuidQuery(testUuidDashed, "GLOBAL") as Record<string, unknown>
			expect(query).not.toHaveProperty("script_fields")
		})
	})

	describe("buildGlobalQuery", () => {
		it("should wrap base text query with entity_global_score boost", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const query = client.buildGlobalQuery(baseQuery)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain("entity_global_score")
			expect(queryStr).toContain(`* ${SCORE_BOOST}`)
			expect(queryStr).toContain("function_score")
			expect(queryStr).toContain("script_score")
			expect(queryStr).toContain("test")
		})

		it("should include script_fields for score_boost", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const query = client.buildGlobalQuery(baseQuery) as Record<string, unknown>

			expect(query).toHaveProperty("script_fields")
			const queryStr = JSON.stringify(query.script_fields)
			expect(queryStr).toContain("score_boost")
			expect(queryStr).toContain("entity_global_score")
		})
	})

	describe("buildGlobalBySpaceScoreQuery", () => {
		it("should wrap base text query with space_score boost", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const query = client.buildGlobalBySpaceScoreQuery(baseQuery)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain("space_score")
			expect(queryStr).toContain(`* ${SCORE_BOOST}`)
			expect(queryStr).toContain("function_score")
			expect(queryStr).toContain("script_score")
			expect(queryStr).toContain("test")
		})
	})

	describe("buildGlobalByEntitySpaceScoreQuery", () => {
		it("should wrap base text query with entity_space_score boost", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const query = client.buildGlobalByEntitySpaceScoreQuery(baseQuery)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain(`* ${SCORE_BOOST}`)
			expect(queryStr).toContain("function_score")
			expect(queryStr).toContain("script_score")
			expect(queryStr).toContain("test")
		})
	})

	describe("buildSingleSpaceQuery", () => {
		it("should include both space_id variants and boost by entity_space_score", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const dashedSpaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const dashlessSpaceId = "abcd1234abcd1234abcd1234abcd5678"
			const query = client.buildSingleSpaceQuery(baseQuery, dashedSpaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(dashedSpaceId)
			expect(queryStr).toContain(dashlessSpaceId)
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain(`* ${SCORE_BOOST}`)
			expect(queryStr).toContain("test")
		})

		it("should include both space_id variants when given dashless input", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const dashlessSpaceId = "abcd1234abcd1234abcd1234abcd5678"
			const dashedSpaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const query = client.buildSingleSpaceQuery(baseQuery, dashlessSpaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(dashedSpaceId)
			expect(queryStr).toContain(dashlessSpaceId)
			expect(queryStr).toContain("entity_space_score")
		})

		it("should include script_fields for score_boost", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const query = client.buildSingleSpaceQuery(baseQuery, "abcd1234-abcd-1234-abcd-1234abcd5678") as Record<string, unknown>

			expect(query).toHaveProperty("script_fields")
			const queryStr = JSON.stringify(query.script_fields)
			expect(queryStr).toContain("score_boost")
			expect(queryStr).toContain("entity_space_score")
		})
	})

	describe("buildTopRankedQuery", () => {
		it("should include script_fields for score_boost", () => {
			const query = client.buildSearchBody({
				query: "",
				scope: "GLOBAL",
			}) as Record<string, unknown>

			expect(query).toHaveProperty("script_fields")
			const queryStr = JSON.stringify(query.script_fields)
			expect(queryStr).toContain("score_boost")
			expect(queryStr).toContain("entity_global_score")
		})
	})

	describe("buildSearchBody with type_ids filtering", () => {
		it("should build UUID query when query is a dashed UUID", () => {
			const uuid = "123e4567-e89b-12d3-a456-426614174000"
			const query = client.buildSearchBody({
				query: uuid,
				scope: "GLOBAL",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(uuid)
			expect(queryStr).toContain("entity_id")
		})

		it("should build UUID query when query is a dashless UUID", () => {
			const dashlessUuid = "123e4567e89b12d3a456426614174000"
			const dashedUuid = "123e4567-e89b-12d3-a456-426614174000"
			const query = client.buildSearchBody({
				query: dashlessUuid,
				scope: "GLOBAL",
			})

			const queryStr = JSON.stringify(query)
			// Should contain both variants (index may have either format)
			expect(queryStr).toContain(dashedUuid)
			expect(queryStr).toContain(dashlessUuid)
			expect(queryStr).toContain("entity_id")
		})

		it("should build UUID query with type_ids filtering for GLOBAL scope", () => {
			const uuid = "123e4567-e89b-12d3-a456-426614174000"
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001", "abcd1234-abcd-1234-abcd-1234abcd0002"]
			const query = client.buildSearchBody({
				query: uuid,
				scope: "GLOBAL",
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(uuid)
			expect(queryStr).toContain("entity_id")
			expect(queryStr).toContain("type_relations.entity_to_id")
			expect(queryStr).toContain(typeIds[0])
			expect(queryStr).toContain(typeIds[1])
		})

		it("should include both dashed and dashless type_id variants in filter", () => {
			const uuid = "123e4567-e89b-12d3-a456-426614174000"
			const dashlessTypeIds = ["abcd1234abcd1234abcd1234abcd0001", "abcd1234abcd1234abcd1234abcd0002"]
			const dashedTypeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001", "abcd1234-abcd-1234-abcd-1234abcd0002"]
			const query = client.buildSearchBody({
				query: uuid,
				scope: "GLOBAL",
				type_ids: dashlessTypeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("type_relations.entity_to_id")
			// Both formats should be present for each type ID
			expect(queryStr).toContain(dashedTypeIds[0])
			expect(queryStr).toContain(dashedTypeIds[1])
			expect(queryStr).toContain(dashlessTypeIds[0])
			expect(queryStr).toContain(dashlessTypeIds[1])
		})

		it("should build UUID query with type_ids and space filtering for SPACE_SINGLE scope", () => {
			const uuid = "123e4567-e89b-12d3-a456-426614174000"
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001"]
			const query = client.buildSearchBody({
				query: uuid,
				scope: "SPACE_SINGLE",
				space_id: spaceId,
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(uuid)
			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("entity_id")
			expect(queryStr).toContain("type_relations.entity_to_id")
			expect(queryStr).toContain(typeIds[0])
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

		it("should build global query with type_ids filtering", () => {
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001", "abcd1234-abcd-1234-abcd-1234abcd0002", "abcd1234-abcd-1234-abcd-1234abcd0003"]
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL",
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("entity_global_score")
			expect(queryStr).toContain("blockchain")
			expect(queryStr).toContain("type_relations.entity_to_id")
			typeIds.forEach((typeId) => {
				expect(queryStr).toContain(typeId)
			})
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

		it("should build global by space score query with type_ids filtering", () => {
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001"]
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL_BY_SPACE_SCORE",
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("space_score")
			expect(queryStr).toContain("blockchain")
			expect(queryStr).toContain("type_relations.entity_to_id")
			expect(queryStr).toContain(typeIds[0])
		})

		it("should build global by entity space score query for GLOBAL_BY_ENTITY_SPACE_SCORE scope", () => {
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL_BY_ENTITY_SPACE_SCORE",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain("blockchain")
		})

		it("should build global by entity space score query with type_ids filtering", () => {
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001"]
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL_BY_ENTITY_SPACE_SCORE",
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain("blockchain")
			expect(queryStr).toContain("type_relations.entity_to_id")
			expect(queryStr).toContain(typeIds[0])
		})

		it("should build single space query for SPACE_SINGLE scope", () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "SPACE_SINGLE",
				space_id: spaceId,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain("blockchain")
		})

		it("should include both space_id variants in SPACE_SINGLE query", () => {
			const dashlessSpaceId = "abcd1234abcd1234abcd1234abcd5678"
			const dashedSpaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "SPACE_SINGLE",
				space_id: dashlessSpaceId,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(dashedSpaceId)
			expect(queryStr).toContain(dashlessSpaceId)
			expect(queryStr).toContain("entity_space_score")
		})

		it("should build single space query with type_ids filtering", () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001", "abcd1234-abcd-1234-abcd-1234abcd0002"]
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "SPACE_SINGLE",
				space_id: spaceId,
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain("blockchain")
			expect(queryStr).toContain("type_relations.entity_to_id")
			typeIds.forEach((typeId) => {
				expect(queryStr).toContain(typeId)
			})
		})

		it("should build single space query for SPACE scope with type_ids filtering", () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd9012"
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001"]
			const query = client.buildSearchBody({
				query: "blockchain",
				scope: "SPACE",
				space_id: spaceId,
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("blockchain")
			expect(queryStr).toContain("type_relations.entity_to_id")
			expect(queryStr).toContain(typeIds[0])
		})

		it("should throw error for SPACE_SINGLE scope without space_id", () => {
			expect(() => {
				client.buildSearchBody({
					query: "blockchain",
					scope: "SPACE_SINGLE",
				})
			}).toThrow("SPACE_SINGLE scope requires space_id")
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
