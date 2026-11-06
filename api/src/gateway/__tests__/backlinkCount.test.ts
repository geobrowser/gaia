import {parse} from "graphql"
import {describe, expect, it} from "vitest"
import {analyzeLightEntities} from "../lightShadow"

const ID = "aaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa"

// The entity tiebreaker batch the web app sends: counts only, no lists.
const TIEBREAK = `query EntityTiebreakerBatch($filter: EntityFilter) {
	entities(filter: $filter) { id createdAt backlinks { totalCount } relations { totalCount } values { totalCount } }
}`

describe("backlinks { totalCount } in an entities selection", () => {
	it("is planned, with the count left uncompared", () => {
		const plan = analyzeLightEntities(parse(TIEBREAK), "EntityTiebreakerBatch", {filter: {id: {in: [ID]}}})
		expect(plan.ok).toBe(true)
		if (plan.ok) expect(plan.plan.uncompared).toEqual(expect.arrayContaining(["backlink_count", "relation_count"]))
	})

	it("reads a backlinks selection that asks for rows, and compares none of them", () => {
		const q = `query { entities(filter: {id: {in: ["${ID}"]}}) { id backlinks { nodes { id } } } }`
		const a = analyzeLightEntities(parse(q), null, {})
		expect(a.ok && a.plan.uncompared).toContain("backlink_rows")
		// Rows only, with nothing of the entity to judge, stay with the backlinks shadow.
		const rows = `query { entities(filter: {id: {in: ["${ID}"]}}) { backlinks { nodes { id } } } }`
		expect(analyzeLightEntities(parse(rows), null, {})).toEqual({ok: false, reason: "selection"})
	})
})
