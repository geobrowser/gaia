import {SystemIds} from "@graphprotocol/grc-20"
import {Hono} from "hono"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {runtime} from "../services/runtime"
import type {SearchClient, SearchResponse} from "../services/search"

import {createSearchRouter} from "./index"

const COMMENT_TYPE = "82f6123a03234c6ca811701c5bc026e9"

const DEFAULT_EXCLUDED_TYPE_IDS = [
	SystemIds.TEXT_BLOCK,
	SystemIds.IMAGE_BLOCK,
	SystemIds.DATA_BLOCK,
	SystemIds.IMAGE_TYPE,
	SystemIds.VIDEO_TYPE,
	SystemIds.VIDEO_BLOCK,
	COMMENT_TYPE,
]

describe("Search Router - Integration Tests", () => {
	let mockSearchClient: SearchClient
	let app: Hono

	const mockSearchResponse: SearchResponse = {
		results: [
			{
				entityId: "123e4567e89b12d3a456426614174000",
				space: {id: "abcd1234abcd1234abcd1234abcd5678"},
				name: "Test Entity",
				description: "A test entity for search",
				types: [
					{id: "abcd1234abcd1234abcd1234abcd0001", name: "Type A"},
					{id: "abcd1234abcd1234abcd1234abcd0002", name: "Type B"},
				],
				entityGlobalScore: 0.8,
				spaceScore: 0.7,
				entitySpaceScore: 0.9,
				relevanceScore: 15.2,
				textMatchScore: 2.0,
				inCanonicalGraph: false,
			},
		],
		total: 1,
		tookMs: 45,
	}

	beforeEach(() => {
		mockSearchClient = {
			search: vi.fn().mockResolvedValue(mockSearchResponse),
			healthCheck: vi.fn().mockResolvedValue(true),
		}
		app = new Hono()
		app.route("/search", createSearchRouter(mockSearchClient, runtime))
	})

	describe("GET /search", () => {
		it("returns search results for valid query", async () => {
			const request = new Request("http://localhost/search?query=test")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(200)
			expect(result).toEqual(mockSearchResponse)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "GLOBAL",
				limit: 20,
				offset: 0,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		it("accepts 'q' parameter as alias for 'query'", async () => {
			const request = new Request("http://localhost/search?q=test")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(200)
			expect(result).toEqual(mockSearchResponse)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "GLOBAL",
				limit: 20,
				offset: 0,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		it("handles custom limit and offset", async () => {
			const request = new Request("http://localhost/search?query=test&limit=50&offset=10")
			const response = await app.fetch(request)
			const _result = await response.json()

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "GLOBAL",
				limit: 50,
				offset: 10,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		it("handles SPACE_SINGLE scope with valid dashed space_id", async () => {
			const spaceId = "123e4567-e89b-12d3-a456-426614174000"
			const request = new Request(`http://localhost/search?query=test&scope=SPACE_SINGLE&space_id=${spaceId}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "SPACE_SINGLE",
				limit: 20,
				offset: 0,
				space_id: spaceId,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		it("handles SPACE_SINGLE scope with valid dashless space_id", async () => {
			const spaceId = "123e4567e89b12d3a456426614174000"
			const request = new Request(`http://localhost/search?query=test&scope=SPACE_SINGLE&space_id=${spaceId}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "SPACE_SINGLE",
				limit: 20,
				offset: 0,
				space_id: spaceId,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		it("handles SPACE scope with valid space_id", async () => {
			const spaceId = "123e4567-e89b-12d3-a456-426614174000"
			const request = new Request(`http://localhost/search?query=test&scope=SPACE&space_id=${spaceId}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "SPACE",
				limit: 20,
				offset: 0,
				space_id: spaceId,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		it("handles GLOBAL_BY_SPACE_SCORE scope", async () => {
			const request = new Request("http://localhost/search?query=test&scope=GLOBAL_BY_SPACE_SCORE")
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "GLOBAL_BY_SPACE_SCORE",
				limit: 20,
				offset: 0,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		it("handles GLOBAL_BY_ENTITY_SPACE_SCORE scope", async () => {
			const request = new Request("http://localhost/search?query=test&scope=GLOBAL_BY_ENTITY_SPACE_SCORE")
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "GLOBAL_BY_ENTITY_SPACE_SCORE",
				limit: 20,
				offset: 0,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		// Top ranked results for empty queries
		it("returns top ranked results for missing query parameter", async () => {
			const request = new Request("http://localhost/search")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(200)
			expect(result).toEqual(mockSearchResponse)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "",
				scope: "GLOBAL",
				limit: 20,
				offset: 0,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		it("returns top ranked results for empty query parameter", async () => {
			const request = new Request("http://localhost/search?query=")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(200)
			expect(result).toEqual(mockSearchResponse)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "",
				scope: "GLOBAL",
				limit: 20,
				offset: 0,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		it("returns top ranked results for whitespace-only query", async () => {
			const request = new Request("http://localhost/search?query=%20%20")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(200)
			expect(result).toEqual(mockSearchResponse)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "",
				scope: "GLOBAL",
				limit: 20,
				offset: 0,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		// Error cases
		it("returns 400 for query longer than maximum length", async () => {
			const longQuery = "a".repeat(501)
			const request = new Request(`http://localhost/search?query=${longQuery}`)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("must not exceed 500 characters")
		})

		it("returns 400 for invalid scope", async () => {
			const request = new Request("http://localhost/search?query=test&scope=INVALID")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("Invalid scope")
		})

		it("returns 400 for SPACE_SINGLE scope without space_id", async () => {
			const request = new Request("http://localhost/search?query=test&scope=SPACE_SINGLE")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("space_id is required")
		})

		it("returns 400 for SPACE scope without space_id", async () => {
			const request = new Request("http://localhost/search?query=test&scope=SPACE")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("space_id is required")
		})

		it("returns 400 for invalid space_id format", async () => {
			const request = new Request("http://localhost/search?query=test&scope=SPACE_SINGLE&space_id=invalid-uuid")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("must be a valid UUID")
		})

		it("returns 400 for space_id longer than maximum length", async () => {
			const longSpaceId = "a".repeat(37)
			const request = new Request(`http://localhost/search?query=test&scope=SPACE_SINGLE&space_id=${longSpaceId}`)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("must not exceed 36 characters")
		})

		it("returns 400 for invalid limit parameter", async () => {
			const request = new Request("http://localhost/search?query=test&limit=invalid")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("must be a positive integer")
		})

		it("clamps limit exceeding maximum to MAX_LIMIT", async () => {
			const request = new Request("http://localhost/search?query=test&limit=150")
			const response = await app.fetch(request)

			expect(response.status).toBe(200) // Should be clamped to MAX_LIMIT
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "GLOBAL",
				limit: 100, // clamped
				offset: 0,
				exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
			})
		})

		it("returns 400 for negative limit", async () => {
			const request = new Request("http://localhost/search?query=test&limit=-1")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("must be a positive integer")
		})

		it("returns 400 for invalid offset parameter", async () => {
			const request = new Request("http://localhost/search?query=test&offset=invalid")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("must be a non-negative integer")
		})

		it("returns 400 for offset exceeding maximum", async () => {
			const request = new Request("http://localhost/search?query=test&offset=1500")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("must not exceed 1000")
		})

		it("returns 400 for unrecognized query parameters", async () => {
			const request = new Request("http://localhost/search?query=test&unknown_param=value")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("unknown_param")
		})

		it("returns 400 for multiple unrecognized query parameters", async () => {
			const request = new Request("http://localhost/search?query=test&foo=1&bar=2")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("foo")
			expect(result.message).toContain("bar")
		})

		it("returns 500 for search client errors", async () => {
			mockSearchClient.search = vi.fn().mockRejectedValue(new Error("Search failed"))

			const request = new Request("http://localhost/search?query=test")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(500)
			expect(result.error).toBe("Search failed")
			expect(result.message).toBe("Search failed")
		})

		it("handles search client errors with custom messages", async () => {
			mockSearchClient.search = vi.fn().mockRejectedValue(new Error("Custom error message"))

			const request = new Request("http://localhost/search?query=test")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(500)
			expect(result.message).toBe("Custom error message")
		})
	})

	describe("additional_space_ids", () => {
		const SPACE_A = "11111111-1111-1111-1111-111111111111"
		const SPACE_B = "22222222-2222-2222-2222-222222222222"
		const ROOT_ID = "00000000-0000-0000-0000-000000000001"

		it("parses CSV and forwards as a string array on GLOBAL scope", async () => {
			const request = new Request(
				`http://localhost/search?query=test&additional_space_ids=${ROOT_ID},${SPACE_A},${SPACE_B}`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					scope: "GLOBAL",
					additional_space_ids: [ROOT_ID, SPACE_A, SPACE_B],
				}),
			)
		})

		it("works with GLOBAL_BY_SPACE_SCORE scope", async () => {
			const request = new Request(
				`http://localhost/search?query=test&scope=GLOBAL_BY_SPACE_SCORE&additional_space_ids=${SPACE_A}`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					scope: "GLOBAL_BY_SPACE_SCORE",
					additional_space_ids: [SPACE_A],
				}),
			)
		})

		it("trims whitespace and skips empty entries", async () => {
			const request = new Request(
				`http://localhost/search?query=test&additional_space_ids=${SPACE_A}, ${SPACE_B}`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					additional_space_ids: [SPACE_A, SPACE_B],
				}),
			)
		})

		it("does not pass the field through when param is absent", async () => {
			const request = new Request("http://localhost/search?query=test")
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			const callArg = (mockSearchClient.search as ReturnType<typeof vi.fn>).mock.calls[0]?.[0] as
				| Record<string, unknown>
				| undefined
			expect(callArg).toBeDefined()
			expect(callArg).not.toHaveProperty("additional_space_ids")
		})

		it("does not pass the field through when param is the empty string", async () => {
			const request = new Request("http://localhost/search?query=test&additional_space_ids=")
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			const callArg = (mockSearchClient.search as ReturnType<typeof vi.fn>).mock.calls[0]?.[0] as
				| Record<string, unknown>
				| undefined
			expect(callArg).toBeDefined()
			expect(callArg).not.toHaveProperty("additional_space_ids")
		})

		it("returns 400 when used with SPACE scope", async () => {
			const request = new Request(
				`http://localhost/search?query=test&scope=SPACE&space_id=${SPACE_A}&additional_space_ids=${SPACE_B}`,
			)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("additional_space_ids is not valid with SPACE scope")
		})

		it("returns 400 when used with SPACE_SINGLE scope", async () => {
			const request = new Request(
				`http://localhost/search?query=test&scope=SPACE_SINGLE&space_id=${SPACE_A}&additional_space_ids=${SPACE_B}`,
			)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.message).toContain("SPACE_SINGLE")
		})

		it("returns 400 when an entry is not a valid UUID", async () => {
			const request = new Request(`http://localhost/search?query=test&additional_space_ids=not-a-uuid`)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.message).toContain("valid UUIDs")
		})

		it("returns 400 when more than 10 IDs are supplied", async () => {
			const ids = Array.from(
				{length: 11},
				(_, i) => `${i.toString().padStart(8, "0")}-0000-0000-0000-000000000000`,
			).join(",")
			const request = new Request(`http://localhost/search?query=test&additional_space_ids=${ids}`)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.message).toContain("more than 10 IDs")
		})

		it("accepts exactly 10 IDs at the boundary", async () => {
			const ids = Array.from(
				{length: 10},
				(_, i) => `${i.toString().padStart(8, "0")}-0000-0000-0000-000000000000`,
			)
			const request = new Request(`http://localhost/search?query=test&additional_space_ids=${ids.join(",")}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					additional_space_ids: ids,
				}),
			)
		})

		it("accepts a single space ID", async () => {
			const request = new Request(`http://localhost/search?query=test&additional_space_ids=${SPACE_A}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					additional_space_ids: [SPACE_A],
				}),
			)
		})

		it("works with GLOBAL_BY_ENTITY_SPACE_SCORE scope", async () => {
			const request = new Request(
				`http://localhost/search?query=test&scope=GLOBAL_BY_ENTITY_SPACE_SCORE&additional_space_ids=${SPACE_A}`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					scope: "GLOBAL_BY_ENTITY_SPACE_SCORE",
					additional_space_ids: [SPACE_A],
				}),
			)
		})

		it("accepts dashless UUIDs", async () => {
			const dashlessA = SPACE_A.replace(/-/g, "")
			const dashlessB = SPACE_B.replace(/-/g, "")
			const request = new Request(
				`http://localhost/search?query=test&additional_space_ids=${dashlessA},${dashlessB}`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					additional_space_ids: [dashlessA, dashlessB],
				}),
			)
		})

		it("accepts mixed dashed and dashless UUIDs in the same call", async () => {
			const dashlessB = SPACE_B.replace(/-/g, "")
			const request = new Request(
				`http://localhost/search?query=test&additional_space_ids=${SPACE_A},${dashlessB}`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					additional_space_ids: [SPACE_A, dashlessB],
				}),
			)
		})

		it("treats a CSV of only commas/whitespace as absent (no 400, param dropped)", async () => {
			const request = new Request("http://localhost/search?query=test&additional_space_ids=,%20,%20,")
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			const callArg = (mockSearchClient.search as ReturnType<typeof vi.fn>).mock.calls[0]?.[0] as
				| Record<string, unknown>
				| undefined
			expect(callArg).toBeDefined()
			expect(callArg).not.toHaveProperty("additional_space_ids")
		})

		it("drops trailing/leading commas while keeping valid entries", async () => {
			const request = new Request(
				`http://localhost/search?query=test&additional_space_ids=,${SPACE_A},${SPACE_B},`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					additional_space_ids: [SPACE_A, SPACE_B],
				}),
			)
		})

		it("returns 400 naming the invalid entry when one of multiple IDs is malformed", async () => {
			const request = new Request(
				`http://localhost/search?query=test&additional_space_ids=${SPACE_A},not-a-uuid,${SPACE_B}`,
			)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.message).toContain("not-a-uuid")
		})

		it("forwards duplicate IDs as-is (router does not dedupe)", async () => {
			const request = new Request(
				`http://localhost/search?query=test&additional_space_ids=${SPACE_A},${SPACE_A},${SPACE_B}`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					additional_space_ids: [SPACE_A, SPACE_A, SPACE_B],
				}),
			)
		})

		it("coexists with type_ids and custom exclude_type_ids", async () => {
			const TYPE_X = "33333333-3333-3333-3333-333333333333"
			const TYPE_Y = "44444444-4444-4444-4444-444444444444"
			const request = new Request(
				`http://localhost/search?query=test&additional_space_ids=${SPACE_A}&type_ids=${TYPE_X}&exclude_type_ids=${TYPE_Y}`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					additional_space_ids: [SPACE_A],
					type_ids: [TYPE_X],
					exclude_type_ids: [TYPE_Y],
				}),
			)
		})

		it("coexists with include_non_canonical=false (canonical-only + extra spaces)", async () => {
			const request = new Request(
				`http://localhost/search?query=test&additional_space_ids=${SPACE_A}&include_non_canonical=false`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					additional_space_ids: [SPACE_A],
					include_non_canonical: false,
				}),
			)
		})

		it("coexists with limit and offset", async () => {
			const request = new Request(
				`http://localhost/search?query=test&additional_space_ids=${SPACE_A}&limit=42&offset=7`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					additional_space_ids: [SPACE_A],
					limit: 42,
					offset: 7,
				}),
			)
		})

		it("coexists with the q alias and an empty query (top-ranked path)", async () => {
			const request = new Request(`http://localhost/search?q=&additional_space_ids=${SPACE_A}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					query: "",
					additional_space_ids: [SPACE_A],
				}),
			)
		})

		it("returns 400 when an entry contains a UUID-shaped string with extra characters", async () => {
			// Tail-padded — looks UUID-ish but isn't a valid UUID
			const request = new Request(`http://localhost/search?query=test&additional_space_ids=${SPACE_A}xx`)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.message).toContain("valid UUIDs")
		})
	})

	describe("exclude_type_ids", () => {
		it("applies default excluded type IDs when param is not provided", async () => {
			const request = new Request("http://localhost/search?query=test")
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith(
				expect.objectContaining({
					exclude_type_ids: DEFAULT_EXCLUDED_TYPE_IDS,
				}),
			)
		})

		it("disables default exclusions when exclude_type_ids is empty string", async () => {
			const request = new Request("http://localhost/search?query=test&exclude_type_ids=")
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "GLOBAL",
				limit: 20,
				offset: 0,
			})
			// Ensure exclude_type_ids is NOT in the call (empty array is not spread)
			const callArg = (mockSearchClient.search as ReturnType<typeof vi.fn>).mock.calls[0]![0]
			expect(callArg).not.toHaveProperty("exclude_type_ids")
		})

		it("passes user-supplied exclude_type_ids to search client", async () => {
			const customExcludeId = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
			const request = new Request(`http://localhost/search?query=test&exclude_type_ids=${customExcludeId}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "GLOBAL",
				limit: 20,
				offset: 0,
				exclude_type_ids: [customExcludeId],
			})
		})

		it("passes multiple user-supplied exclude_type_ids", async () => {
			const id1 = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
			const id2 = "11111111-2222-3333-4444-555555555555"
			const request = new Request(`http://localhost/search?query=test&exclude_type_ids=${id1},${id2}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "GLOBAL",
				limit: 20,
				offset: 0,
				exclude_type_ids: [id1, id2],
			})
		})

		it("returns 400 for invalid UUID in exclude_type_ids", async () => {
			const request = new Request("http://localhost/search?query=test&exclude_type_ids=not-a-uuid")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("exclude_type_ids must contain valid UUIDs")
		})

		it("returns 400 when exclude_type_ids exceeds maximum count", async () => {
			const ids = Array.from({length: 11}, (_, i) => `aaaaaaaa-bbbb-cccc-dddd-${String(i).padStart(12, "0")}`)
			const request = new Request(`http://localhost/search?query=test&exclude_type_ids=${ids.join(",")}`)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("exclude_type_ids must not contain more than 10 IDs")
		})

		it("returns 400 when user-supplied exclude_type_ids conflicts with type_ids", async () => {
			const sharedId = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
			const request = new Request(
				`http://localhost/search?query=test&type_ids=${sharedId}&exclude_type_ids=${sharedId}`,
			)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("type_ids and exclude_type_ids must not contain the same IDs")
			expect(result.message).toContain(sharedId)
		})

		it("explicit type_ids overrides default exclusions without error", async () => {
			// Request a type that is in the default exclusion list — should succeed, not 400
			const request = new Request(`http://localhost/search?query=test&type_ids=${SystemIds.TEXT_BLOCK}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			const callArg = (mockSearchClient.search as ReturnType<typeof vi.fn>).mock.calls[0]![0]
			expect(callArg.type_ids).toEqual([SystemIds.TEXT_BLOCK])
			// TEXT_BLOCK should be stripped from the default exclusion list
			if (callArg.exclude_type_ids) {
				expect(callArg.exclude_type_ids).not.toContain(SystemIds.TEXT_BLOCK)
			}
		})
	})

	describe("inCanonicalGraph field", () => {
		it("returns inCanonicalGraph: true when set", async () => {
			const response: SearchResponse = {
				results: [
					{
						entityId: "123e4567e89b12d3a456426614174000",
						space: {id: "abcd1234abcd1234abcd1234abcd5678"},
						inCanonicalGraph: true,
					},
				],
				total: 1,
				tookMs: 1,
			}
			mockSearchClient.search = vi.fn().mockResolvedValue(response)

			const res = await app.fetch(new Request("http://localhost/search?q=test"))
			const body = await res.json()

			expect(body.results[0].inCanonicalGraph).toBe(true)
		})

		it("returns inCanonicalGraph: false when set", async () => {
			const response: SearchResponse = {
				results: [
					{
						entityId: "123e4567e89b12d3a456426614174000",
						space: {id: "abcd1234abcd1234abcd1234abcd5678"},
						inCanonicalGraph: false,
					},
				],
				total: 1,
				tookMs: 1,
			}
			mockSearchClient.search = vi.fn().mockResolvedValue(response)

			const res = await app.fetch(new Request("http://localhost/search?q=test"))
			const body = await res.json()

			expect(body.results[0].inCanonicalGraph).toBe(false)
		})

		it("defaults inCanonicalGraph to false when not set", async () => {
			const response: SearchResponse = {
				results: [
					{
						entityId: "123e4567e89b12d3a456426614174000",
						space: {id: "abcd1234abcd1234abcd1234abcd5678"},
						inCanonicalGraph: false,
					},
				],
				total: 1,
				tookMs: 1,
			}
			mockSearchClient.search = vi.fn().mockResolvedValue(response)

			const res = await app.fetch(new Request("http://localhost/search?q=test"))
			const body = await res.json()

			expect(body.results[0].inCanonicalGraph).toBe(false)
		})
	})

	describe("GET /search/health", () => {
		it("should return healthy status when search client is healthy", async () => {
			mockSearchClient.healthCheck = vi.fn().mockResolvedValue(true)

			const request = new Request("http://localhost/search/health")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(200)
			expect(result).toEqual({status: "healthy"})
		})

		it("should return unhealthy status when search client is unhealthy", async () => {
			mockSearchClient.healthCheck = vi.fn().mockResolvedValue(false)

			const request = new Request("http://localhost/search/health")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(503)
			expect(result).toEqual({status: "unhealthy"})
		})

		it("should return unhealthy status when health check throws", async () => {
			mockSearchClient.healthCheck = vi.fn().mockRejectedValue(new Error("Connection failed"))

			const request = new Request("http://localhost/search/health")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(503)
			expect(result).toEqual({status: "unhealthy"})
		})
	})
})
