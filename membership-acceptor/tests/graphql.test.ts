import {afterEach, describe, expect, test} from "bun:test"

import {createGraphQLClient, GraphQLError} from "../src/graphql.js"

const realFetch = globalThis.fetch

afterEach(() => {
	globalThis.fetch = realFetch
})

function mockFetch(impl: (url: string, init: RequestInit) => Promise<Response> | Response) {
	// biome-ignore lint/suspicious/noExplicitAny: test seam for the global fetch
	globalThis.fetch = ((url: any, init: any) => Promise.resolve(impl(String(url), init))) as typeof fetch
}

const client = createGraphQLClient({endpoint: "https://api.example/graphql"})

describe("createGraphQLClient", () => {
	test("posts the query and returns data", async () => {
		let capturedUrl = ""
		mockFetch((url) => {
			capturedUrl = url
			return new Response(JSON.stringify({data: {editor: {memberSpaceId: "abc"}}}), {status: 200})
		})

		const data = await client.query<{editor: {memberSpaceId: string} | null}>("query { editor { memberSpaceId } }")
		expect(data.editor?.memberSpaceId).toBe("abc")
		expect(capturedUrl).toBe("https://api.example/graphql")
	})

	test("passes variables through", async () => {
		let sentVars: unknown
		mockFetch((_url, init) => {
			sentVars = (JSON.parse(String(init.body)) as {variables: unknown}).variables
			return new Response(JSON.stringify({data: {ok: true}}), {status: 200})
		})
		await client.query("query($x: String!){ ok }", {x: "y"})
		expect(sentVars).toEqual({x: "y"})
	})

	test("throws GraphQLError on a non-empty errors array", async () => {
		mockFetch(() => new Response(JSON.stringify({errors: [{message: "boom"}]}), {status: 200}))
		await expect(client.query("query { x }")).rejects.toThrow(GraphQLError)
	})

	test("throws GraphQLError on an HTTP error", async () => {
		mockFetch(() => new Response("nope", {status: 500, statusText: "Server Error"}))
		await expect(client.query("query { x }")).rejects.toThrow(GraphQLError)
	})

	test("throws GraphQLError when data is absent", async () => {
		mockFetch(() => new Response(JSON.stringify({}), {status: 200}))
		await expect(client.query("query { x }")).rejects.toThrow(GraphQLError)
	})

	test("throws GraphQLError on a network failure", async () => {
		mockFetch(() => {
			throw new Error("ECONNREFUSED")
		})
		await expect(client.query("query { x }")).rejects.toThrow(GraphQLError)
	})
})
