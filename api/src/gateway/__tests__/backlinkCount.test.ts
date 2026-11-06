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

	it("still refuses a backlinks selection that asks for rows", () => {
		const q = `query { entities(filter: {id: {in: ["${ID}"]}}) { id backlinks { nodes { id } } } }`
		expect(analyzeLightEntities(parse(q), null, {})).toEqual({ok: false, reason: "selection"})
	})
})
