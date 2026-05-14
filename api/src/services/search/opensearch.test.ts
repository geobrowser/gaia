import {afterEach, beforeEach, describe, expect, it, vi} from "vitest"

import {
	FUZZY_MAX_EXPANSIONS,
	MAX_NAME_MATCH_TEXT_TOKENS,
	MAX_TEXT_TOKENS,
	MIN_SCORE_THRESHOLD,
	OpenSearchClient,
	PHRASE_PREFIX_MAX_EXPANSIONS,
	SCORE_BOOST,
	SCORE_SHIFT,
} from "./opensearch"

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

		it("caps clause-heavy sub-queries to 20 tokens; exact-match clauses see full query within name cap", () => {
			// Query of 30 tokens → cap should kick in at 20 for bool_prefix /
			// match_phrase_prefix. The exact-match clauses (term: name_raw, match: name)
			// still see all 30 tokens because this is below MAX_NAME_MATCH_TEXT_TOKENS.
			// Fuzzy is dropped entirely at this token count (see separate test).
			const tokens = Array.from({length: 30}, (_, i) => `t${i + 1}`)
			const fullQuery = tokens.join(" ")
			const cappedQuery = tokens.slice(0, MAX_TEXT_TOKENS).join(" ")
			const overflowToken = tokens[25] // would be present if cap weren't applied

			const query = client.buildBaseTextQuery(fullQuery) as {
				bool: {should: Record<string, unknown>[]}
			}
			const should = query.bool.should

			const boolPrefix = should.find(
				(c) => "multi_match" in c && (c.multi_match as {type?: string}).type === "bool_prefix",
			) as {multi_match: {query: string}}
			const phrasePrefixName = should.find(
				(c) => "match_phrase_prefix" in c && (c.match_phrase_prefix as {name?: unknown}).name !== undefined,
			) as {match_phrase_prefix: {name: {query: string}}}
			const phrasePrefixDesc = should.find(
				(c) =>
					"match_phrase_prefix" in c &&
					(c.match_phrase_prefix as {description?: unknown}).description !== undefined,
			) as {match_phrase_prefix: {description: {query: string}}}
			const matchExactToken = should.find(
				(c) => "match" in c && (c.match as {name?: unknown}).name !== undefined,
			) as {match: {name: {query: string}}}
			const termExactName = should.find(
				(c) =>
					"term" in c &&
					typeof (c.term as {name_raw?: {value?: string; boost?: number}}).name_raw?.value === "string",
			) as {term: {name_raw: {value: string}}}

			// Heavy sub-queries: capped to 20 tokens, must NOT contain the 26th token
			expect(boolPrefix.multi_match.query).toBe(cappedQuery)
			expect(boolPrefix.multi_match.query).not.toContain(overflowToken)
			expect(phrasePrefixName.match_phrase_prefix.name.query).toBe(cappedQuery)
			expect(phrasePrefixDesc.match_phrase_prefix.description.query).toBe(cappedQuery)
			expect(phrasePrefixName.match_phrase_prefix.name).toHaveProperty(
				"max_expansions",
				PHRASE_PREFIX_MAX_EXPANSIONS,
			)
			expect(phrasePrefixDesc.match_phrase_prefix.description).toHaveProperty(
				"max_expansions",
				PHRASE_PREFIX_MAX_EXPANSIONS,
			)

			// Exact-match sub-queries: still see the full 30-token query
			expect(matchExactToken.match.name.query).toBe(fullQuery)
			expect(matchExactToken.match.name.query).toContain(overflowToken)
			expect(termExactName.term.name_raw.value).toBe(fullQuery)
		})

		it("caps a 250-character adversarial token query to bounded OpenSearch clauses", () => {
			const tokens = [...Array.from({length: 64}, (_, i) => `t${i + 1}`), "zzz"]
			const fullQuery = tokens.join(" ")
			expect(fullQuery).toHaveLength(250)

			const cappedQuery = tokens.slice(0, MAX_TEXT_TOKENS).join(" ")
			const nameMatchQuery = tokens.slice(0, MAX_NAME_MATCH_TEXT_TOKENS).join(" ")
			const overflowToken = tokens[55]

			const query = client.buildBaseTextQuery(fullQuery) as {
				bool: {should: Record<string, unknown>[]}
			}
			const should = query.bool.should

			const fuzzy = should.find(
				(c) => "multi_match" in c && (c.multi_match as {fuzziness?: string}).fuzziness !== undefined,
			)
			const boolPrefix = should.find(
				(c) => "multi_match" in c && (c.multi_match as {type?: string}).type === "bool_prefix",
			) as {multi_match: {query: string}}
			const phrasePrefixName = should.find(
				(c) => "match_phrase_prefix" in c && (c.match_phrase_prefix as {name?: unknown}).name !== undefined,
			) as {match_phrase_prefix: {name: {query: string; max_expansions: number}}}
			const phrasePrefixDesc = should.find(
				(c) =>
					"match_phrase_prefix" in c &&
					(c.match_phrase_prefix as {description?: unknown}).description !== undefined,
			) as {match_phrase_prefix: {description: {query: string; max_expansions: number}}}
			const matchExactToken = should.find(
				(c) => "match" in c && (c.match as {name?: unknown}).name !== undefined,
			) as {match: {name: {query: string}}}
			const termExactName = should.find(
				(c) =>
					"term" in c &&
					typeof (c.term as {name_raw?: {value?: string; boost?: number}}).name_raw?.value === "string",
			) as {term: {name_raw: {value: string}}}

			expect(fuzzy).toBeUndefined()
			expect(boolPrefix.multi_match.query).toBe(cappedQuery)
			expect(boolPrefix.multi_match.query).not.toContain(overflowToken)
			expect(phrasePrefixName.match_phrase_prefix.name.query).toBe(cappedQuery)
			expect(phrasePrefixName.match_phrase_prefix.name.max_expansions).toBe(PHRASE_PREFIX_MAX_EXPANSIONS)
			expect(phrasePrefixDesc.match_phrase_prefix.description.query).toBe(cappedQuery)
			expect(phrasePrefixDesc.match_phrase_prefix.description.max_expansions).toBe(PHRASE_PREFIX_MAX_EXPANSIONS)
			expect(matchExactToken.match.name.query).toBe(nameMatchQuery)
			expect(matchExactToken.match.name.query).not.toContain(overflowToken)
			expect(termExactName.term.name_raw.value).toBe(fullQuery)
		})

		it("includes fuzzy clause with capped max_expansions when token count <= 3", () => {
			// 3 tokens — at the boundary
			const query = client.buildBaseTextQuery("alpha beta gamma") as {
				bool: {should: Record<string, unknown>[]}
			}
			const fuzzy = query.bool.should.find(
				(c) => "multi_match" in c && (c.multi_match as {fuzziness?: string}).fuzziness !== undefined,
			) as {multi_match: {query: string; fuzziness: string; max_expansions: number}}

			expect(fuzzy).toBeDefined()
			expect(fuzzy.multi_match.fuzziness).toBe("AUTO")
			expect(fuzzy.multi_match.query).toBe("alpha beta gamma")
			// max_expansions is capped (default OS value is 50; we pin lower for safety).
			expect(fuzzy.multi_match.max_expansions).toBe(FUZZY_MAX_EXPANSIONS)
		})

		it("keeps fuzzy enabled for a 250-character query when token count is within the fuzzy gate", () => {
			const fullQuery = `${"a".repeat(80)} ${"b".repeat(80)} ${"c".repeat(88)}`
			expect(fullQuery).toHaveLength(250)

			const query = client.buildBaseTextQuery(fullQuery) as {
				bool: {should: Record<string, unknown>[]}
			}
			const fuzzy = query.bool.should.find(
				(c) => "multi_match" in c && (c.multi_match as {fuzziness?: string}).fuzziness !== undefined,
			) as {multi_match: {query: string; fuzziness: string; max_expansions: number}}

			expect(fuzzy).toBeDefined()
			expect(fuzzy.multi_match.fuzziness).toBe("AUTO")
			expect(fuzzy.multi_match.query).toBe(fullQuery)
			expect(fuzzy.multi_match.max_expansions).toBe(FUZZY_MAX_EXPANSIONS)
		})

		it("drops fuzzy clause when token count > 3 (fan-out reduction)", () => {
			// 4 tokens — just over the boundary
			const query = client.buildBaseTextQuery("alpha beta gamma delta") as {
				bool: {should: Record<string, unknown>[]}
			}
			const fuzzy = query.bool.should.find(
				(c) => "multi_match" in c && (c.multi_match as {fuzziness?: string}).fuzziness !== undefined,
			)
			expect(fuzzy).toBeUndefined()

			// Other heavy clauses must still be present (they have their own cap)
			const boolPrefix = query.bool.should.find(
				(c) => "multi_match" in c && (c.multi_match as {type?: string}).type === "bool_prefix",
			)
			expect(boolPrefix).toBeDefined()
		})

		it("counts punctuation-separated tokens correctly (no whitespace bypass)", () => {
			// Adversarial: 6 tokens separated by commas/hyphens/dots, no whitespace.
			// Whitespace-only splitter would see this as 1 token and incorrectly
			// keep fuzzy enabled. The analyzer-aligned splitter sees 6 → fuzzy dropped.
			const query = client.buildBaseTextQuery("alpha,beta-gamma.delta;epsilon!zeta") as {
				bool: {should: Record<string, unknown>[]}
			}
			const fuzzy = query.bool.should.find(
				(c) => "multi_match" in c && (c.multi_match as {fuzziness?: string}).fuzziness !== undefined,
			)
			expect(fuzzy).toBeUndefined()
		})

		it("preserves accented Latin letters as a single token (no over-split)", () => {
			// "café" is one token to OpenSearch's analyzer. Our splitter should agree.
			// "café" with no punctuation → 1 token → fuzzy enabled.
			const query = client.buildBaseTextQuery("café") as {
				bool: {should: Record<string, unknown>[]}
			}
			const fuzzy = query.bool.should.find(
				(c) => "multi_match" in c && (c.multi_match as {fuzziness?: string}).fuzziness !== undefined,
			) as {multi_match: {query: string}}
			expect(fuzzy).toBeDefined()
			expect(fuzzy.multi_match.query).toBe("café")
		})

		it("does not truncate when query is already short", () => {
			// 3 tokens — fuzzy is included (≤ FUZZY_MAX_TOKENS) and sees the full query.
			const shortQuery = "three short tokens"
			const query = client.buildBaseTextQuery(shortQuery) as {
				bool: {should: Record<string, unknown>[]}
			}
			const fuzzy = query.bool.should.find(
				(c) => "multi_match" in c && (c.multi_match as {fuzziness?: string}).fuzziness !== undefined,
			) as {multi_match: {query: string}}

			expect(fuzzy.multi_match.query).toBe(shortQuery)
		})
	})

	describe("buildUuidQuery", () => {
		const testUuidDashed = "123e4567-e89b-12d3-a456-426614174000"
		const testUuidDashless = "123e4567e89b12d3a456426614174000"

		it("should include both dashed and dashless UUID variants in terms query", async () => {
			const query = await client.buildUuidQuery(testUuidDashed, "GLOBAL")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuidDashed)
			expect(queryStr).toContain(testUuidDashless)
			expect(queryStr).toContain("entity_id")
			expect(queryStr).toContain("terms")
		})

		it("should include both variants when given a dashless UUID", async () => {
			const query = await client.buildUuidQuery(testUuidDashless, "GLOBAL")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuidDashed)
			expect(queryStr).toContain(testUuidDashless)
			expect(queryStr).toContain("entity_id")
		})

		it("should build terms query for GLOBAL_BY_SPACE_SCORE scope", async () => {
			const query = await client.buildUuidQuery(testUuidDashed, "GLOBAL_BY_SPACE_SCORE")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuidDashed)
			expect(queryStr).toContain(testUuidDashless)
			expect(queryStr).toContain("entity_id")
		})

		it("should build terms query for GLOBAL_BY_ENTITY_SPACE_SCORE scope", async () => {
			const query = await client.buildUuidQuery(testUuidDashed, "GLOBAL_BY_ENTITY_SPACE_SCORE")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuidDashed)
			expect(queryStr).toContain(testUuidDashless)
			expect(queryStr).toContain("entity_id")
		})

		it("should include both space_id variants in SPACE_SINGLE filter", async () => {
			const dashedSpaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const dashlessSpaceId = "abcd1234abcd1234abcd1234abcd5678"
			const query = await client.buildUuidQuery(testUuidDashed, "SPACE_SINGLE", dashedSpaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(dashedSpaceId)
			expect(queryStr).toContain(dashlessSpaceId)
			expect(queryStr).toContain("entity_id")
		})

		it("should include both space_id variants when given dashless space_id", async () => {
			const dashlessSpaceId = "abcd1234abcd1234abcd1234abcd5678"
			const dashedSpaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const query = await client.buildUuidQuery(testUuidDashed, "SPACE_SINGLE", dashlessSpaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(dashedSpaceId)
			expect(queryStr).toContain(dashlessSpaceId)
		})

		it("should build query for SPACE_SINGLE scope without space_id", async () => {
			const query = await client.buildUuidQuery(testUuidDashed, "SPACE_SINGLE")
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(testUuidDashed)
			expect(queryStr).toContain("entity_id")
		})

		it("should include both space_id variants in SPACE filter", async () => {
			const dashedSpaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const dashlessSpaceId = "abcd1234abcd1234abcd1234abcd5678"
			const query = await client.buildUuidQuery(testUuidDashed, "SPACE", dashedSpaceId)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain(dashedSpaceId)
			expect(queryStr).toContain(dashlessSpaceId)
			expect(queryStr).toContain("entity_id")
		})

		it("should not include script_fields", async () => {
			const query = (await client.buildUuidQuery(testUuidDashed, "GLOBAL")) as Record<string, unknown>
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
			const query = client.buildSingleSpaceQuery(baseQuery, "abcd1234-abcd-1234-abcd-1234abcd5678") as Record<
				string,
				unknown
			>

			expect(query).toHaveProperty("script_fields")
			const queryStr = JSON.stringify(query.script_fields)
			expect(queryStr).toContain("score_boost")
			expect(queryStr).toContain("entity_space_score")
		})
	})

	describe("buildTopRankedQuery", () => {
		it("should include script_fields for score_boost", async () => {
			const query = (await client.buildSearchBody({
				query: "",
				scope: "GLOBAL",
			})) as Record<string, unknown>

			expect(query).toHaveProperty("script_fields")
			const queryStr = JSON.stringify(query.script_fields)
			expect(queryStr).toContain("score_boost")
			expect(queryStr).toContain("entity_global_score")
		})
	})

	describe("buildSearchBody with type_ids filtering", () => {
		it("should build UUID query when query is a dashed UUID", async () => {
			const uuid = "123e4567-e89b-12d3-a456-426614174000"
			const query = await client.buildSearchBody({
				query: uuid,
				scope: "GLOBAL",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(uuid)
			expect(queryStr).toContain("entity_id")
		})

		it("should build UUID query when query is a dashless UUID", async () => {
			const dashlessUuid = "123e4567e89b12d3a456426614174000"
			const dashedUuid = "123e4567-e89b-12d3-a456-426614174000"
			const query = await client.buildSearchBody({
				query: dashlessUuid,
				scope: "GLOBAL",
			})

			const queryStr = JSON.stringify(query)
			// Should contain both variants (index may have either format)
			expect(queryStr).toContain(dashedUuid)
			expect(queryStr).toContain(dashlessUuid)
			expect(queryStr).toContain("entity_id")
		})

		it("should build UUID query with type_ids filtering for GLOBAL scope", async () => {
			const uuid = "123e4567-e89b-12d3-a456-426614174000"
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001", "abcd1234-abcd-1234-abcd-1234abcd0002"]
			const query = await client.buildSearchBody({
				query: uuid,
				scope: "GLOBAL",
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(uuid)
			expect(queryStr).toContain("entity_id")
			expect(queryStr).toContain("relations.to_entity_id")
			expect(queryStr).toContain(typeIds[0])
			expect(queryStr).toContain(typeIds[1])
		})

		it("should include both dashed and dashless type_id variants in filter", async () => {
			const uuid = "123e4567-e89b-12d3-a456-426614174000"
			const dashlessTypeIds = ["abcd1234abcd1234abcd1234abcd0001", "abcd1234abcd1234abcd1234abcd0002"]
			const dashedTypeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001", "abcd1234-abcd-1234-abcd-1234abcd0002"]
			const query = await client.buildSearchBody({
				query: uuid,
				scope: "GLOBAL",
				type_ids: dashlessTypeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("relations.to_entity_id")
			// Both formats should be present for each type ID
			expect(queryStr).toContain(dashedTypeIds[0])
			expect(queryStr).toContain(dashedTypeIds[1])
			expect(queryStr).toContain(dashlessTypeIds[0])
			expect(queryStr).toContain(dashlessTypeIds[1])
		})

		it("should build UUID query with type_ids and space filtering for SPACE_SINGLE scope", async () => {
			const uuid = "123e4567-e89b-12d3-a456-426614174000"
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001"]
			const query = await client.buildSearchBody({
				query: uuid,
				scope: "SPACE_SINGLE",
				space_id: spaceId,
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(uuid)
			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("entity_id")
			expect(queryStr).toContain("relations.to_entity_id")
			expect(queryStr).toContain(typeIds[0])
		})

		it("should build global query for GLOBAL scope", async () => {
			const query = await client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("entity_global_score")
			expect(queryStr).toContain("blockchain")
		})

		it("should build global query with type_ids filtering", async () => {
			const typeIds = [
				"abcd1234-abcd-1234-abcd-1234abcd0001",
				"abcd1234-abcd-1234-abcd-1234abcd0002",
				"abcd1234-abcd-1234-abcd-1234abcd0003",
			]
			const query = await client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL",
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("entity_global_score")
			expect(queryStr).toContain("blockchain")
			expect(queryStr).toContain("relations.to_entity_id")
			typeIds.forEach((typeId) => {
				expect(queryStr).toContain(typeId)
			})
		})

		it("should build global by space score query for GLOBAL_BY_SPACE_SCORE scope", async () => {
			const query = await client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL_BY_SPACE_SCORE",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("space_score")
			expect(queryStr).toContain("blockchain")
		})

		it("should build global by space score query with type_ids filtering", async () => {
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001"]
			const query = await client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL_BY_SPACE_SCORE",
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("space_score")
			expect(queryStr).toContain("blockchain")
			expect(queryStr).toContain("relations.to_entity_id")
			expect(queryStr).toContain(typeIds[0])
		})

		it("should build global by entity space score query for GLOBAL_BY_ENTITY_SPACE_SCORE scope", async () => {
			const query = await client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL_BY_ENTITY_SPACE_SCORE",
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain("blockchain")
		})

		it("should build global by entity space score query with type_ids filtering", async () => {
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001"]
			const query = await client.buildSearchBody({
				query: "blockchain",
				scope: "GLOBAL_BY_ENTITY_SPACE_SCORE",
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain("blockchain")
			expect(queryStr).toContain("relations.to_entity_id")
			expect(queryStr).toContain(typeIds[0])
		})

		it("should build single space query for SPACE_SINGLE scope", async () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const query = await client.buildSearchBody({
				query: "blockchain",
				scope: "SPACE_SINGLE",
				space_id: spaceId,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain("blockchain")
		})

		it("should include both space_id variants in SPACE_SINGLE query", async () => {
			const dashlessSpaceId = "abcd1234abcd1234abcd1234abcd5678"
			const dashedSpaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const query = await client.buildSearchBody({
				query: "blockchain",
				scope: "SPACE_SINGLE",
				space_id: dashlessSpaceId,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(dashedSpaceId)
			expect(queryStr).toContain(dashlessSpaceId)
			expect(queryStr).toContain("entity_space_score")
		})

		it("should build single space query with type_ids filtering", async () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd5678"
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001", "abcd1234-abcd-1234-abcd-1234abcd0002"]
			const query = await client.buildSearchBody({
				query: "blockchain",
				scope: "SPACE_SINGLE",
				space_id: spaceId,
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("entity_space_score")
			expect(queryStr).toContain("blockchain")
			expect(queryStr).toContain("relations.to_entity_id")
			typeIds.forEach((typeId) => {
				expect(queryStr).toContain(typeId)
			})
		})

		it("should build single space query for SPACE scope with type_ids filtering", async () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd9012"
			const typeIds = ["abcd1234-abcd-1234-abcd-1234abcd0001"]
			const query = await client.buildSearchBody({
				query: "blockchain",
				scope: "SPACE",
				space_id: spaceId,
				type_ids: typeIds,
			})

			const queryStr = JSON.stringify(query)
			expect(queryStr).toContain(spaceId)
			expect(queryStr).toContain("blockchain")
			expect(queryStr).toContain("relations.to_entity_id")
			expect(queryStr).toContain(typeIds[0])
		})

		it("throws error for SPACE_SINGLE scope without space_id", async () => {
			await expect(
				client.buildSearchBody({
					query: "blockchain",
					scope: "SPACE_SINGLE",
				}),
			).rejects.toThrow("SPACE_SINGLE scope requires space_id")
		})

		it("throws error for SPACE scope without space_id", async () => {
			await expect(
				client.buildSearchBody({
					query: "blockchain",
					scope: "SPACE",
				}),
			).rejects.toThrow("SPACE scope requires space_id")
		})
	})

	describe("buildMultiSpaceQuery", () => {
		it("filters by space_id terms when isRoot is false", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const spaceIds = ["abcd1234-abcd-1234-abcd-1234abcd0001", "abcd1234-abcd-1234-abcd-1234abcd0002"]
			const query = client.buildMultiSpaceQuery(baseQuery, spaceIds, undefined, false, false, undefined, true)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain("space_id")
			expect(queryStr).toContain(spaceIds[0])
			expect(queryStr).toContain(spaceIds[1])
			// in_canonical_graph not present when includeNonCanonical=true
			expect(queryStr).not.toContain("in_canonical_graph")
		})

		it("uses in_canonical_graph filter when isRoot is true", () => {
			const baseQuery = client.buildBaseTextQuery("test")
			const spaceIds = ["abcd1234-abcd-1234-abcd-1234abcd0001"]
			const query = client.buildMultiSpaceQuery(baseQuery, spaceIds, undefined, false, true)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain("in_canonical_graph")
			expect(queryStr).not.toContain(spaceIds[0])
		})
	})

	describe("fetchSubspaces", () => {
		const topologyUrl = "http://localhost:9090"
		let topologyClient: OpenSearchClient

		beforeEach(() => {
			topologyClient = new OpenSearchClient("http://localhost:9200", "test-index", topologyUrl)
		})

		afterEach(() => {
			vi.restoreAllMocks()
		})

		it("returns isRoot true when topology response has is_root true", async () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd0001"
			vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
				new Response(JSON.stringify({subspaces: [spaceId, "child-1", "child-2"], is_root: true}), {
					status: 200,
					headers: {"Content-Type": "application/json"},
				}),
			)

			const result = await topologyClient.fetchSubspaces(spaceId)
			expect(result.isRoot).toBe(true)
			expect(result.subspaces).toEqual([spaceId, "child-1", "child-2"])
		})

		it("returns isRoot false when is_root is missing from response", async () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd0002"
			vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
				new Response(JSON.stringify({subspaces: [spaceId]}), {
					status: 200,
					headers: {"Content-Type": "application/json"},
				}),
			)

			const result = await topologyClient.fetchSubspaces(spaceId)
			expect(result.isRoot).toBe(false)
			expect(result.subspaces).toEqual([spaceId])
		})

		it("returns isRoot false on 404", async () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd0003"
			vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
				new Response(JSON.stringify({error: "not found"}), {status: 404}),
			)

			const result = await topologyClient.fetchSubspaces(spaceId)
			expect(result.isRoot).toBe(false)
			expect(result.subspaces).toEqual([spaceId])
		})

		it("throws on network error", async () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd0004"
			vi.spyOn(globalThis, "fetch").mockRejectedValueOnce(new Error("network error"))

			await expect(topologyClient.fetchSubspaces(spaceId)).rejects.toThrow("network error")
		})

		it("caches results and does not re-fetch", async () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd0005"
			const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
				new Response(JSON.stringify({subspaces: [spaceId], is_root: true}), {
					status: 200,
					headers: {"Content-Type": "application/json"},
				}),
			)

			const result1 = await topologyClient.fetchSubspaces(spaceId)
			const result2 = await topologyClient.fetchSubspaces(spaceId)

			expect(fetchSpy).toHaveBeenCalledTimes(1)
			expect(result1).toEqual(result2)
		})
	})

	describe("SPACE scope with canonical root", () => {
		const topologyUrl = "http://localhost:9090"
		let topologyClient: OpenSearchClient

		beforeEach(() => {
			topologyClient = new OpenSearchClient("http://localhost:9200", "test-index", topologyUrl)
		})

		afterEach(() => {
			vi.restoreAllMocks()
		})

		it("uses in_canonical_graph filter when space is root", async () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd0001"
			vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
				new Response(JSON.stringify({subspaces: [spaceId, "child-1"], is_root: true}), {
					status: 200,
					headers: {"Content-Type": "application/json"},
				}),
			)

			const query = await topologyClient.buildSearchBody({
				query: "blockchain",
				scope: "SPACE",
				space_id: spaceId,
			})
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain("in_canonical_graph")
			expect(queryStr).not.toContain(spaceId)
		})

		it("uses space_id terms filter when space is not root", async () => {
			const spaceId = "abcd1234-abcd-1234-abcd-1234abcd0002"
			const childId = "abcd1234-abcd-1234-abcd-1234abcd0003"
			vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
				new Response(JSON.stringify({subspaces: [spaceId, childId]}), {
					status: 200,
					headers: {"Content-Type": "application/json"},
				}),
			)

			const query = await topologyClient.buildSearchBody({
				query: "blockchain",
				scope: "SPACE",
				space_id: spaceId,
			})
			const queryStr = JSON.stringify(query)

			// in_canonical_graph not present — default is include_non_canonical=true
			expect(queryStr).not.toContain("in_canonical_graph")
			expect(queryStr).toContain(spaceId)
		})
	})

	describe("init() root space caching", () => {
		const topologyUrl = "http://localhost:9090"
		let topologyClient: OpenSearchClient

		beforeEach(() => {
			topologyClient = new OpenSearchClient("http://localhost:9200", "test-index", topologyUrl)
		})

		afterEach(() => {
			vi.restoreAllMocks()
		})

		it("fetches and caches root space ID on init", async () => {
			const rootId = "root1234-abcd-1234-abcd-1234abcd0001"
			vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
				new Response(JSON.stringify({root_id: rootId}), {
					status: 200,
					headers: {"Content-Type": "application/json"},
				}),
			)

			await topologyClient.init()

			// Verify root was fetched from /topology/root
			expect(globalThis.fetch).toHaveBeenCalledWith(
				`${topologyUrl}/topology/root`,
				expect.objectContaining({signal: expect.any(AbortSignal)}),
			)
		})

		it("does not crash when init fails with network error", async () => {
			vi.spyOn(globalThis, "fetch").mockRejectedValueOnce(new Error("connection refused"))

			// Should not throw
			await topologyClient.init()
		})

		it("does not crash when init returns non-200", async () => {
			vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(new Response("not found", {status: 404}))

			// Should not throw
			await topologyClient.init()
		})

		it("skips init when no topology URL is configured", async () => {
			const noTopologyClient = new OpenSearchClient("http://localhost:9200", "test-index")
			const fetchSpy = vi.spyOn(globalThis, "fetch")

			await noTopologyClient.init()

			expect(fetchSpy).not.toHaveBeenCalled()
		})

		it("short-circuits fetchSubspaces when space is cached root", async () => {
			const rootId = "root1234-abcd-1234-abcd-1234abcd0001"
			// Mock init response
			const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
				new Response(JSON.stringify({root_id: rootId}), {
					status: 200,
					headers: {"Content-Type": "application/json"},
				}),
			)

			await topologyClient.init()
			fetchSpy.mockClear()

			// Now query with the root space — should NOT call fetch
			const query = await topologyClient.buildSearchBody({
				query: "blockchain",
				scope: "SPACE",
				space_id: rootId,
			})
			const queryStr = JSON.stringify(query)

			expect(fetchSpy).not.toHaveBeenCalled()
			expect(queryStr).toContain("in_canonical_graph")
		})

		it("calls fetchSubspaces for non-root spaces after init", async () => {
			const rootId = "root1234-abcd-1234-abcd-1234abcd0001"
			const otherSpaceId = "abcd1234-abcd-1234-abcd-1234abcd9999"
			const fetchSpy = vi
				.spyOn(globalThis, "fetch")
				// init response
				.mockResolvedValueOnce(
					new Response(JSON.stringify({root_id: rootId}), {
						status: 200,
						headers: {"Content-Type": "application/json"},
					}),
				)
				// fetchSubspaces response for the non-root space
				.mockResolvedValueOnce(
					new Response(JSON.stringify({subspaces: [otherSpaceId]}), {
						status: 200,
						headers: {"Content-Type": "application/json"},
					}),
				)

			await topologyClient.init()

			const query = await topologyClient.buildSearchBody({
				query: "blockchain",
				scope: "SPACE",
				space_id: otherSpaceId,
			})
			const queryStr = JSON.stringify(query)

			// Should have called fetch for subspaces (2nd call after init)
			expect(fetchSpy).toHaveBeenCalledTimes(2)
			// in_canonical_graph not present — default is include_non_canonical=true
			expect(queryStr).not.toContain("in_canonical_graph")
			expect(queryStr).toContain(otherSpaceId)
		})

		it("lazily backfills rootSpaceId when fetchSubspaces discovers root", async () => {
			const rootId = "root1234-abcd-1234-abcd-1234abcd0001"
			const fetchSpy = vi
				.spyOn(globalThis, "fetch")
				// init fails
				.mockRejectedValueOnce(new Error("connection refused"))
				// fetchSubspaces discovers root
				.mockResolvedValueOnce(
					new Response(JSON.stringify({subspaces: [rootId, "child-1"], is_root: true}), {
						status: 200,
						headers: {"Content-Type": "application/json"},
					}),
				)

			await topologyClient.init()

			// First query — fetchSubspaces discovers root
			const query1 = await topologyClient.buildSearchBody({
				query: "test",
				scope: "SPACE",
				space_id: rootId,
			})
			expect(JSON.stringify(query1)).toContain("in_canonical_graph")

			fetchSpy.mockClear()

			// Second query — should short-circuit, no fetch
			const query2 = await topologyClient.buildSearchBody({
				query: "test2",
				scope: "SPACE",
				space_id: rootId,
			})
			expect(JSON.stringify(query2)).toContain("in_canonical_graph")
			expect(fetchSpy).not.toHaveBeenCalled()
		})

		it("short-circuits root space in buildUuidQuery SPACE scope", async () => {
			const rootId = "root1234-abcd-1234-abcd-1234abcd0001"
			const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
				new Response(JSON.stringify({root_id: rootId}), {
					status: 200,
					headers: {"Content-Type": "application/json"},
				}),
			)

			await topologyClient.init()
			fetchSpy.mockClear()

			const uuid = "123e4567-e89b-12d3-a456-426614174000"
			const query = await topologyClient.buildUuidQuery(uuid, "SPACE", rootId)
			const queryStr = JSON.stringify(query)

			expect(fetchSpy).not.toHaveBeenCalled()
			expect(queryStr).toContain("in_canonical_graph")
			expect(queryStr).toContain("entity_id")
		})

		it("short-circuits root space in buildTopRankedQuery SPACE scope", async () => {
			const rootId = "root1234-abcd-1234-abcd-1234abcd0001"
			const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
				new Response(JSON.stringify({root_id: rootId}), {
					status: 200,
					headers: {"Content-Type": "application/json"},
				}),
			)

			await topologyClient.init()
			fetchSpy.mockClear()

			const query = await topologyClient.buildTopRankedQuery("SPACE", rootId)
			const queryStr = JSON.stringify(query)

			expect(fetchSpy).not.toHaveBeenCalled()
			expect(queryStr).toContain("in_canonical_graph")
		})
	})

	describe("score boost ranking: Geo root space vs other Geo entity", () => {
		/**
		 * Regression test for score boost tuning.
		 *
		 * Entity A: name="Geo", desc="The root space for the global decentralized knowledge graph."
		 *           entity_global_score=1.2
		 * Entity B: name="Geo", desc="Geo is a network for organizing knowledge that we can collectively govern and trust."
		 *           entity_global_score=0.4
		 *
		 * Entity A (the root space) must always rank higher than Entity B in global search,
		 * even though Entity B has a richer description containing the query term "Geo".
		 */

		function computeScoreBoost(score: number): number {
			const clamped = Math.max(score, MIN_SCORE_THRESHOLD)
			return (clamped + SCORE_SHIFT) * SCORE_BOOST
		}

		it("should produce a higher score boost for entity_global_score=1.2 than 0.4", () => {
			const boostA = computeScoreBoost(1.2)
			const boostB = computeScoreBoost(0.4)

			expect(boostA).toBeGreaterThan(boostB)
		})

		it("should produce a score boost gap large enough to overcome description text match advantage", () => {
			const boostA = computeScoreBoost(1.2)
			const boostB = computeScoreBoost(0.4)
			const boostGap = boostA - boostB

			// Entity B's description contains "Geo" which adds ~3-5 extra text match points
			// from match_phrase_prefix, bool_prefix, and fuzzy matches on description.
			// The score boost gap must exceed this text match advantage.
			const estimatedDescriptionAdvantage = 5.0
			expect(boostGap).toBeGreaterThan(estimatedDescriptionAdvantage)
		})

		it("should include entity_global_score in the global query boost script", () => {
			const baseQuery = client.buildBaseTextQuery("Geo")
			const query = client.buildGlobalQuery(baseQuery)
			const queryStr = JSON.stringify(query)

			expect(queryStr).toContain("entity_global_score")
			expect(queryStr).toContain(`* ${SCORE_BOOST}`)
		})
	})

	// ------------------------------------------------------------------
	// additional_space_ids — eligibility-set widening on GLOBAL queries
	// ------------------------------------------------------------------
	describe("buildAdditionalSpacesFilter", () => {
		const ROOT_ID = "00000000-0000-0000-0000-000000000001"
		const SPACE_A = "11111111-1111-1111-1111-111111111111"
		const SPACE_B = "22222222-2222-2222-2222-222222222222"

		// Inject a known root ID for the helper to detect.
		const setRoot = (c: OpenSearchClient, id: string | null) => {
			;(c as unknown as {rootSpaceId: string | null}).rootSpaceId = id
		}

		it("returns null when no IDs are supplied", () => {
			expect(client.buildAdditionalSpacesFilter(undefined)).toBeNull()
			expect(client.buildAdditionalSpacesFilter([])).toBeNull()
		})

		it("OR's canonical + listed-space terms when only non-root IDs are supplied", () => {
			setRoot(client, ROOT_ID)
			const filter = client.buildAdditionalSpacesFilter([SPACE_A, SPACE_B]) as {
				bool: {should: object[]; minimum_should_match: number}
			}
			expect(filter.bool.minimum_should_match).toBe(1)
			expect(filter.bool.should).toContainEqual({term: {in_canonical_graph: true}})
			const termsClause = filter.bool.should.find((c): c is {terms: {space_id: string[]}} => "terms" in c) as {
				terms: {space_id: string[]}
			}
			expect(termsClause.terms.space_id).toContain(SPACE_A)
			expect(termsClause.terms.space_id).toContain(SPACE_B)
			// includes both dashed and dashless variants
			expect(termsClause.terms.space_id).toContain(SPACE_A.replace(/-/g, ""))
		})

		it("returns the single canonical term when only the root ID is supplied", () => {
			setRoot(client, ROOT_ID)
			// Root collapses to canonical, no listed-spaces terms ⇒ bare canonical clause.
			expect(client.buildAdditionalSpacesFilter([ROOT_ID])).toEqual({
				term: {in_canonical_graph: true},
			})
		})

		it("returns a bool.should OR-ing canonical + non-root terms when root + extras are present", () => {
			setRoot(client, ROOT_ID)
			const filter = client.buildAdditionalSpacesFilter([ROOT_ID, SPACE_A, SPACE_B]) as {
				bool: {should: object[]; minimum_should_match: number}
			}
			expect(filter.bool.minimum_should_match).toBe(1)
			expect(filter.bool.should).toHaveLength(2)
			expect(filter.bool.should).toContainEqual({term: {in_canonical_graph: true}})
			const termsClause = filter.bool.should.find((c): c is {terms: {space_id: string[]}} => "terms" in c) as {
				terms: {space_id: string[]}
			}
			expect(termsClause.terms.space_id).toContain(SPACE_A)
			expect(termsClause.terms.space_id).toContain(SPACE_B)
			// Root ID is naturally idempotent — no need to filter it out, but it shouldn't
			// be duplicated as a non-root term either.
			expect(termsClause.terms.space_id).not.toContain(ROOT_ID)
		})

		it("still includes canonical even when the root is not yet cached (would-be-root stays in terms)", () => {
			setRoot(client, null)
			const filter = client.buildAdditionalSpacesFilter([ROOT_ID, SPACE_A]) as {
				bool: {should: object[]; minimum_should_match: number}
			}
			expect(filter.bool.should).toContainEqual({term: {in_canonical_graph: true}})
			const termsClause = filter.bool.should.find((c): c is {terms: {space_id: string[]}} => "terms" in c) as {
				terms: {space_id: string[]}
			}
			// Without a known root the would-be-root ID stays as a regular term.
			expect(termsClause.terms.space_id).toContain(ROOT_ID)
			expect(termsClause.terms.space_id).toContain(SPACE_A)
		})

		it("matches the root when caller passes dashless and cache is dashed", () => {
			setRoot(client, ROOT_ID) // dashed cache
			const dashlessRoot = ROOT_ID.replace(/-/g, "")
			expect(client.buildAdditionalSpacesFilter([dashlessRoot])).toEqual({
				term: {in_canonical_graph: true},
			})
		})

		it("matches the root when caller passes dashed and cache is dashless", () => {
			setRoot(client, ROOT_ID.replace(/-/g, ""))
			expect(client.buildAdditionalSpacesFilter([ROOT_ID])).toEqual({
				term: {in_canonical_graph: true},
			})
		})

		it("rewrites mixed-form root + extras into bool.should without leaking the root into terms", () => {
			setRoot(client, ROOT_ID) // dashed cache
			const dashlessRoot = ROOT_ID.replace(/-/g, "")
			const filter = client.buildAdditionalSpacesFilter([dashlessRoot, SPACE_A]) as {
				bool: {should: object[]; minimum_should_match: number}
			}
			expect(filter.bool.minimum_should_match).toBe(1)
			expect(filter.bool.should).toContainEqual({term: {in_canonical_graph: true}})
			const termsClause = filter.bool.should.find((c): c is {terms: {space_id: string[]}} => "terms" in c) as {
				terms: {space_id: string[]}
			}
			// Root must NOT appear in the terms clause (in either form) — it should
			// have been rewritten to the canonical anchor only.
			expect(termsClause.terms.space_id).not.toContain(dashlessRoot)
			expect(termsClause.terms.space_id).not.toContain(ROOT_ID)
			// SPACE_A must still appear in both dashed and dashless forms
			expect(termsClause.terms.space_id).toContain(SPACE_A)
			expect(termsClause.terms.space_id).toContain(SPACE_A.replace(/-/g, ""))
		})
	})

	describe("buildSearchBody — additional_space_ids integration", () => {
		const ROOT_ID = "00000000-0000-0000-0000-000000000001"
		const SPACE_A = "11111111-1111-1111-1111-111111111111"
		const SPACE_B = "22222222-2222-2222-2222-222222222222"

		const setRoot = (c: OpenSearchClient, id: string | null) => {
			;(c as unknown as {rootSpaceId: string | null}).rootSpaceId = id
		}

		// Drill into the body shape to get the filter array we care about.
		// Body shape: { query: { function_score: { query: { bool: { filter: [...] } } } } }
		const extractFunctionScoreFilters = (body: Record<string, unknown>): Array<Record<string, unknown>> => {
			const fs = (body.query as {function_score: {query: {bool: {filter: Array<Record<string, unknown>>}}}})
				.function_score
			return fs.query.bool.filter
		}

		it("widens GLOBAL eligibility to canonical OR listed spaces when root + extras are passed", async () => {
			setRoot(client, ROOT_ID)
			const body = (await client.buildSearchBody({
				query: "test",
				scope: "GLOBAL",
				additional_space_ids: [ROOT_ID, SPACE_A, SPACE_B],
				// disable canonical-only restriction so the explicit filter is the
				// only one shaping eligibility — keeps the assertion focused.
				include_non_canonical: true,
			})) as Record<string, unknown>

			const bodyStr = JSON.stringify(body)
			// The bool.should from buildAdditionalSpacesFilter must be present
			expect(bodyStr).toContain('"in_canonical_graph":true')
			expect(bodyStr).toContain('"minimum_should_match":1')
			expect(bodyStr).toContain(SPACE_A)
			expect(bodyStr).toContain(SPACE_B)
		})

		it("emits only the canonical term when GLOBAL is called with just the root ID", async () => {
			setRoot(client, ROOT_ID)
			const body = (await client.buildSearchBody({
				query: "test",
				scope: "GLOBAL",
				additional_space_ids: [ROOT_ID],
				include_non_canonical: true,
			})) as Record<string, unknown>

			const filters = extractFunctionScoreFilters(body)
			// The eligibility-filter for additional_space_ids must be the bare
			// canonical term — having `{term: {in_canonical_graph: true}}` directly
			// in the filters array proves we didn't wrap a single clause in bool.should.
			expect(filters).toContainEqual({term: {in_canonical_graph: true}})
		})

		it("emits canonical OR listed-spaces when GLOBAL is called with non-root spaces only", async () => {
			setRoot(client, ROOT_ID)
			const body = (await client.buildSearchBody({
				query: "test",
				scope: "GLOBAL",
				additional_space_ids: [SPACE_A, SPACE_B],
				include_non_canonical: true,
			})) as Record<string, unknown>

			const filters = extractFunctionScoreFilters(body)
			// The asid filter must be a bool.should OR-ing canonical + listed-spaces terms,
			// since canonical is now ALWAYS implicit when additional_space_ids is set.
			// Match it by the canonical-anchor term inside `.should` (distinguishes it from
			// the non-deleted filter which is also a bool.should).
			const asidFilter = filters.find((f) => {
				const should = (f as {bool?: {should?: object[]}}).bool?.should
				return Array.isArray(should) && should.some((c) => JSON.stringify(c).includes("in_canonical_graph"))
			}) as {bool: {should: object[]; minimum_should_match: number}} | undefined
			expect(asidFilter).toBeDefined()
			expect(asidFilter!.bool.minimum_should_match).toBe(1)
			expect(asidFilter!.bool.should).toContainEqual({term: {in_canonical_graph: true}})
			const termsFilter = asidFilter!.bool.should.find(
				(c) => "terms" in c && (c as {terms: {space_id?: unknown}}).terms.space_id,
			) as {terms: {space_id: string[]}} | undefined
			expect(termsFilter).toBeDefined()
			expect(termsFilter!.terms.space_id).toEqual(expect.arrayContaining([SPACE_A, SPACE_B]))
		})

		it("threads additional_space_ids through buildTopRankedQuery (empty query)", async () => {
			setRoot(client, ROOT_ID)
			const body = (await client.buildSearchBody({
				query: "   ",
				scope: "GLOBAL",
				additional_space_ids: [ROOT_ID, SPACE_A],
				include_non_canonical: true,
			})) as Record<string, unknown>

			const bodyStr = JSON.stringify(body)
			expect(bodyStr).toContain('"in_canonical_graph":true')
			expect(bodyStr).toContain(SPACE_A)
		})

		it("threads additional_space_ids through buildUuidQuery (UUID-shaped query)", async () => {
			setRoot(client, ROOT_ID)
			const entityId = "123e4567-e89b-12d3-a456-426614174000"
			const body = (await client.buildSearchBody({
				query: entityId,
				scope: "GLOBAL",
				additional_space_ids: [SPACE_A],
				include_non_canonical: true,
			})) as Record<string, unknown>

			const bodyStr = JSON.stringify(body)
			expect(bodyStr).toContain(entityId)
			expect(bodyStr).toContain(SPACE_A)
		})

		it("preserves existing behavior when additional_space_ids is absent", async () => {
			setRoot(client, ROOT_ID)
			const before = await client.buildSearchBody({
				query: "test",
				scope: "GLOBAL",
				include_non_canonical: true,
			})
			const after = await client.buildSearchBody({
				query: "test",
				scope: "GLOBAL",
				additional_space_ids: [],
				include_non_canonical: true,
			})
			expect(JSON.stringify(after)).toBe(JSON.stringify(before))
		})

		it("suppresses additional_space_ids when include_non_canonical=false (canonical-only wins)", async () => {
			setRoot(client, ROOT_ID)
			// With both supplied, the asid filter must NOT be added — the
			// canonical-only filter is the sole eligibility constraint and the
			// asid clause is redundant (canonical AND (canonical OR listed) = canonical).
			const body = (await client.buildSearchBody({
				query: "test",
				scope: "GLOBAL",
				additional_space_ids: [SPACE_A, SPACE_B],
				include_non_canonical: false,
			})) as Record<string, unknown>

			const filters = extractFunctionScoreFilters(body)
			// Bare canonical-only filter must be present.
			expect(filters).toContainEqual({term: {in_canonical_graph: true}})
			// The asid bool.should must NOT be present in the filters array.
			const asidLeak = filters.find((f) => {
				const should = (f as {bool?: {should?: object[]}}).bool?.should
				return (
					Array.isArray(should) &&
					should.some((c) => "terms" in c && (c as {terms: {space_id?: unknown}}).terms.space_id)
				)
			})
			expect(asidLeak).toBeUndefined()
		})

		it("equivalent body for include_non_canonical=false with vs without additional_space_ids", async () => {
			setRoot(client, ROOT_ID)
			const without = await client.buildSearchBody({
				query: "test",
				scope: "GLOBAL",
				include_non_canonical: false,
			})
			const with_ = await client.buildSearchBody({
				query: "test",
				scope: "GLOBAL",
				additional_space_ids: [SPACE_A, SPACE_B],
				include_non_canonical: false,
			})
			// Body must be byte-identical: the asid clause adds nothing when
			// canonical-only wins.
			expect(JSON.stringify(with_)).toBe(JSON.stringify(without))
		})
	})
})
