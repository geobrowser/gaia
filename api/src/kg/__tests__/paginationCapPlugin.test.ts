import {describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"
import {assertPaginationWithinLimit} from "../paginationCapPlugin"

async function executeGraphQL(query: string, variables?: Record<string, unknown>) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query, variables}),
		}),
		{},
	)
	return {
		status: response.status,
		body: await response.json(),
	}
}

describe("assertPaginationWithinLimit", () => {
	it("allows pagination arguments at or below the limit", () => {
		expect(() => assertPaginationWithinLimit({first: 1000, last: 1000, offset: 1000})).not.toThrow()
	})

	it("rejects oversized pagination arguments", () => {
		expect(() => assertPaginationWithinLimit({first: 1001})).toThrow(/first.*1000.*1001/i)
		expect(() => assertPaginationWithinLimit({last: 1001})).toThrow(/last.*1000.*1001/i)
		expect(() => assertPaginationWithinLimit({offset: 1001})).toThrow(/offset.*1000.*1001/i)
	})
})

describe("PaginationCapPlugin", () => {
	it("rejects oversized inline pagination arguments", async () => {
		const result = await executeGraphQL(`
			{
				entities(first: 10000) {
					id
				}
			}
		`)

		expect(result.status).toBe(400)
		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("rejects oversized variable pagination arguments", async () => {
		const result = await executeGraphQL(
			`
				query TestPaginationLimit($first: Int!) {
					entities(first: $first) {
						id
					}
				}
			`,
			{first: 10000},
		)

		expect(result.status).toBe(400)
		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})
})
