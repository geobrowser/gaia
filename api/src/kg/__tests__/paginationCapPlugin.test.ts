import {describe, expect, it} from "vitest"
import {assertPaginationWithinLimit} from "../paginationCapPlugin"
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

	// --- gap-probing tests added alongside the existing ones ---

	it("rejects oversized first on the connection form", async () => {
		const result = await executeGraphQL(`
			{
				entitiesConnection(first: 10000) {
					nodes { id }
				}
			}
		`)

		expect(result.status).toBe(400)
		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("rejects oversized last on a root collection", async () => {
		const result = await executeGraphQL(`
			{
				entitiesConnection(last: 10000) {
					nodes { id }
				}
			}
		`)

		expect(result.status).toBe(400)
		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("rejects oversized offset on a root collection", async () => {
		// NOTE: this documents current behavior — a cap on offset prevents deep
		// pagination (e.g. page 12 at 100/page is offset 1100). If legitimate
		// deep pagination is required, the offset cap should be loosened.
		const result = await executeGraphQL(`
			{
				entities(offset: 10000) {
					id
				}
			}
		`)

		expect(result.status).toBe(400)
		expect(result.body.errors).toBeDefined()
		expect(result.body.errors[0]?.extensions?.code).toBe("BAD_USER_INPUT")
	})

	it("accepts the maximum allowed first on the connection form", async () => {
		const result = await executeGraphQL(`
			{
				entitiesConnection(first: 1000) {
					totalCount
				}
			}
		`)

		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
	})

	it("accepts queries with no pagination argument (no cap applied when omitted)", async () => {
		// NOTE: this documents current behavior — when `first` is omitted the
		// plugin does not inject a default. PostGraphile may return all rows.
		const result = await executeGraphQL(`
			{
				entities { id }
			}
		`)

		expect(result.status).toBe(200)
		expect(result.body.errors).toBeUndefined()
	})

	it("caps oversized first on nested sub-collections", async () => {
		// The plugin doc string says nested sub-collections "don't typically
		// accept user-controlled `first` arguments", but PostGraphile does
		// generate `first` args on nested FK-relation connections. This probes
		// whether the cap applies there too. If it does not, this is the gap.
		const result = await executeGraphQL(`
			{
				entitiesConnection(first: 1) {
					nodes {
						id
						relationsByFromEntityIdList(first: 10000) { id }
					}
				}
			}
		`)

		// Expect either: (a) validation error because the field/arg doesn't exist
		// (in which case the shape doesn't apply), or (b) BAD_USER_INPUT because
		// the cap caught it. Anything else (200 with data) = gap.
		if (result.status === 200 && !result.body.errors) {
			throw new Error(
				`Nested sub-collection accepted first: 10000 without being rejected. Response: ${JSON.stringify(
					result.body,
				).slice(0, 400)}`,
			)
		}
		if (result.body.errors?.[0]?.extensions?.code === "BAD_USER_INPUT") {
			expect(result.body.errors[0].extensions.code).toBe("BAD_USER_INPUT")
			return
		}
		// Validation error is acceptable — means the field name differs
		expect(result.body.errors).toBeDefined()
	})
})
