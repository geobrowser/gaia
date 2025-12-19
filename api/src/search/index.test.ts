import {Hono} from "hono"
import {beforeEach, describe, expect, it, vi} from "vitest"
import type {SearchClient, SearchResponse} from "../services/search"

import {createSearchRouter} from "./index"

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
		app.route("/search", createSearchRouter(mockSearchClient))
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
			const spaceId = "123e4567-e89b-12d3-a456-426614174000"
			const request = new Request(`http://localhost/search?query=test&scope=SPACE_SINGLE&space_id=${spaceId}`)
			const response = await app.fetch(request)

			expect(response.status).toBe(200)
			expect(mockSearchClient.search).toHaveBeenCalledWith({
				query: "test",
				scope: "SPACE_SINGLE",
				space_id: spaceId,
				limit: 20,
				offset: 0,
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
				space_id: spaceId,
				limit: 20,
				offset: 0,
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

		// Error cases
		it("returns 400 for missing query parameter", async () => {
			const request = new Request("http://localhost/search")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Missing required parameter")
			expect(result.message).toContain("required")
		})

		it("returns 400 for empty query parameter", async () => {
			const request = new Request("http://localhost/search?query=")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Missing required parameter")
		})

		it("returns 400 for query shorter than minimum length", async () => {
			const request = new Request("http://localhost/search?query=a")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Invalid parameter")
			expect(result.message).toContain("at least 2 characters")
		})

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
			expect(result.error).toBe("Missing required parameter")
			expect(result.message).toContain("space_id is required")
		})

		it("returns 400 for SPACE scope without space_id", async () => {
			const request = new Request("http://localhost/search?query=test&scope=SPACE")
			const response = await app.fetch(request)
			const result = await response.json()

			expect(response.status).toBe(400)
			expect(result.error).toBe("Missing required parameter")
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
