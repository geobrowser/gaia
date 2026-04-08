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

function filterSchemaErrors(errors: Array<{message: string; extensions?: {code?: string}}> | undefined) {
	return (errors ?? []).filter(
		(e) =>
			e.extensions?.code === "GRAPHQL_VALIDATION_FAILED" ||
			e.extensions?.code === "GRAPHQL_PARSE_FAILED",
	)
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

	it("accepts LOCAL_SCORE_DESC as an orderBy argument", async () => {
		const result = await executeGraphQL(`
			{
				valuesConnection(orderBy: LOCAL_SCORE_DESC, first: 5) {
					nodes { id entityId spaceId }
					pageInfo { hasNextPage endCursor }
				}
			}
		`)

		expect(filterSchemaErrors(result.errors)).toHaveLength(0)
	})

	it("accepts GLOBAL_SCORE_DESC as an orderBy argument", async () => {
		const result = await executeGraphQL(`
			{
				valuesConnection(orderBy: GLOBAL_SCORE_DESC, first: 5) {
					nodes { id entityId }
					pageInfo { hasNextPage endCursor }
				}
			}
		`)

		expect(filterSchemaErrors(result.errors)).toHaveLength(0)
	})

	it("accepts multi-column orderBy with score and other fields", async () => {
		const result = await executeGraphQL(`
			{
				valuesConnection(orderBy: [LOCAL_SCORE_DESC, PROPERTY_ID_ASC], first: 5) {
					nodes { id entityId propertyId }
				}
			}
		`)

		expect(filterSchemaErrors(result.errors)).toHaveLength(0)
	})

	it("supports cursor-based pagination with score ordering", async () => {
		const firstPage = await executeGraphQL(`
			{
				valuesConnection(orderBy: LOCAL_SCORE_DESC, first: 2) {
					nodes { id }
					pageInfo { hasNextPage endCursor }
				}
			}
		`)

		expect(filterSchemaErrors(firstPage.errors)).toHaveLength(0)

		// Second page using cursor — validates schema accepts after + orderBy together
		const nextPage = await executeGraphQL(`
			query NextPage($cursor: Cursor!) {
				valuesConnection(orderBy: LOCAL_SCORE_DESC, first: 2, after: $cursor) {
					nodes { id }
					pageInfo { hasNextPage endCursor }
				}
			}
		`, {cursor: "fakecursor"})

		expect(filterSchemaErrors(nextPage.errors)).toHaveLength(0)
	})

	it("composes orderBy with spaceId filter", async () => {
		const result = await executeGraphQL(`
			{
				valuesConnection(
					filter: { spaceId: { is: "00000000000000000000000000000000" } }
					orderBy: LOCAL_SCORE_DESC
					first: 5
				) {
					nodes { id entityId spaceId }
				}
			}
		`)

		expect(filterSchemaErrors(result.errors)).toHaveLength(0)
	})
})
