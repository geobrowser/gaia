import {describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

async function executeGraphQL(query: string, variables?: Record<string, unknown>) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query, variables}),
		}),
		{},
	)
	return response.json()
}

describe("ValueOrderByScorePlugin", () => {
	it("adds LOCAL_SCORE_ASC and LOCAL_SCORE_DESC to ValuesOrderBy enum", async () => {
		const result = await executeGraphQL(`
			{
				__type(name: "ValuesOrderBy") {
					enumValues { name }
				}
			}
		`)

		expect(result.errors).toBeUndefined()
		const enumNames = result.data.__type.enumValues.map((v: {name: string}) => v.name)
		expect(enumNames).toContain("LOCAL_SCORE_ASC")
		expect(enumNames).toContain("LOCAL_SCORE_DESC")
	})

	it("accepts LOCAL_SCORE_DESC as an orderBy argument without schema errors", async () => {
		const result = await executeGraphQL(`
			{
				values(orderBy: LOCAL_SCORE_DESC, first: 5) {
					nodes { id entityId spaceId }
					pageInfo { hasNextPage endCursor }
				}
			}
		`)

		// Should not have GraphQL schema/validation errors
		// (may have DB errors if no database is connected, but the schema should accept the enum)
		const schemaErrors = (result.errors ?? []).filter(
			(e: {message: string}) =>
				e.message.includes("is not a valid enum value") ||
				e.message.includes("Unknown argument") ||
				e.message.includes("Expected type"),
		)
		expect(schemaErrors).toHaveLength(0)
	})

	it("accepts GLOBAL_SCORE_DESC as an orderBy argument without schema errors", async () => {
		const result = await executeGraphQL(`
			{
				values(orderBy: GLOBAL_SCORE_DESC, first: 5) {
					nodes { id entityId }
					pageInfo { hasNextPage endCursor }
				}
			}
		`)

		const schemaErrors = (result.errors ?? []).filter(
			(e: {message: string}) =>
				e.message.includes("is not a valid enum value") ||
				e.message.includes("Unknown argument") ||
				e.message.includes("Expected type"),
		)
		expect(schemaErrors).toHaveLength(0)
	})

	it("accepts multi-column orderBy with score and other fields", async () => {
		const result = await executeGraphQL(`
			{
				values(orderBy: [LOCAL_SCORE_DESC, PROPERTY_ID_ASC], first: 5) {
					nodes { id entityId propertyId }
				}
			}
		`)

		const schemaErrors = (result.errors ?? []).filter(
			(e: {message: string}) =>
				e.message.includes("is not a valid enum value") ||
				e.message.includes("Unknown argument") ||
				e.message.includes("Expected type"),
		)
		expect(schemaErrors).toHaveLength(0)
	})

	it("adds GLOBAL_SCORE_ASC and GLOBAL_SCORE_DESC to ValuesOrderBy enum", async () => {
		const result = await executeGraphQL(`
			{
				__type(name: "ValuesOrderBy") {
					enumValues { name }
				}
			}
		`)

		expect(result.errors).toBeUndefined()
		const enumNames = result.data.__type.enumValues.map((v: {name: string}) => v.name)
		expect(enumNames).toContain("GLOBAL_SCORE_ASC")
		expect(enumNames).toContain("GLOBAL_SCORE_DESC")
	})
})
