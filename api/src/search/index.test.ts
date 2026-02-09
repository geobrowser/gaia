import {Hono} from "hono"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {runtime} from "../services/runtime"
import type {SearchClient, SearchResponse} from "../services/search"
import {uuidToBase58} from "../utils/uuid"

import {createSearchRouter} from "./index"

const DASHED_HEX_UUID = "123e4567-e89b-12d3-a456-426614174000"
const SPACE_ID_BASE58 = uuidToBase58(DASHED_HEX_UUID)

describe("Search Router - Integration Tests", () => {
	let mockSearchClient: SearchClient
	let app: Hono

	const mockSearchResponse: SearchResponse = {
		results: [
			{
				entityId: "123e4567-e89b-12d3-a456-426614174000",
				spaceId: "space-123",
				name: "Test Entity",
				description: "A test entity for search",
				typeIds: ["type-id-1", "type-id-2"],
				entityGlobalScore: 0.8,
				spaceScore: 0.7,
				entitySpaceScore: 0.9,
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
			})
		})

		it("handles SPACE_SINGLE scope with valid space_id", async () => {
			const request = new Request(
				`http://localhost/search?query=test&scope=SPACE_SINGLE&space_id=${SPACE_ID_BASE58}`,
			)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "SPACE_SINGLE",
				limit: 20,
				offset: 0,
				space_id: DASHED_HEX_UUID,
			})
		})

		it("handles SPACE scope with valid space_id", async () => {
			const request = new Request(`http://localhost/search?query=test&scope=SPACE&space_id=${SPACE_ID_BASE58}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "SPACE",
				limit: 20,
				offset: 0,
				space_id: DASHED_HEX_UUID,
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
			const request = new Request("http://localhost/search?query=test&scope=SPACE_SINGLE&space_id=invalid-id!")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("must be a valid Base58 ID")
		})

		it("returns 400 for space_id longer than maximum length", async () => {
			const longSpaceId = "a".repeat(23)
			const request = new Request(`http://localhost/search?query=test&scope=SPACE_SINGLE&space_id=${longSpaceId}`)
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("must not exceed 22 characters")
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
