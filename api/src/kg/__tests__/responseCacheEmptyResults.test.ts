import {describe, expect, it} from "vitest"
import {hasCacheableData} from "../responseCachePolicy"

// =============================================================================
// hasCacheableData — gates what the response cache is allowed to store.
//
// The rule it enforces: a result that says "nothing here" must never be
// cached, because writes land out of band (chain -> indexer -> Postgres) and
// nothing will invalidate it before its TTL expires. See #655.
// =============================================================================

describe("hasCacheableData — empty results are not cacheable", () => {
	it("rejects a null or missing payload", () => {
		expect(hasCacheableData(null)).toBe(false)
		expect(hasCacheableData(undefined)).toBe(false)
	})

	it("rejects an empty list — the #655 case", () => {
		// A client polling for a space it just created, before the indexer
		// caught up. Caching this is what stuck the modal for 60s.
		expect(hasCacheableData({spaces: []})).toBe(false)
	})

	it("rejects a null single-entity lookup", () => {
		expect(hasCacheableData({entity: null})).toBe(false)
	})

	it("rejects an empty Relay connection", () => {
		expect(hasCacheableData({spacesConnection: {nodes: [], totalCount: 0}})).toBe(false)
		expect(hasCacheableData({spacesConnection: {edges: [], totalCount: 0}})).toBe(false)
	})

	it("rejects a payload with no root fields at all", () => {
		expect(hasCacheableData({})).toBe(false)
	})
})

describe("hasCacheableData — real results stay cacheable", () => {
	it("accepts a populated list", () => {
		expect(hasCacheableData({spaces: [{id: "a"}]})).toBe(true)
	})

	it("accepts a populated connection", () => {
		expect(hasCacheableData({spacesConnection: {nodes: [{id: "a"}], totalCount: 1}})).toBe(true)
	})

	it("accepts a single entity that exists", () => {
		expect(hasCacheableData({entity: {id: "a", name: "Thing"}})).toBe(true)
	})

	it("accepts a partially populated payload", () => {
		// One empty root field must not disqualify a response that carries data
		// elsewhere — otherwise the large, stable queries stop being cached.
		expect(hasCacheableData({spaces: [], entities: [{id: "a"}]})).toBe(true)
	})

	it("treats a zero totalCount as empty but a positive one as data", () => {
		expect(hasCacheableData({c: {totalCount: 0}})).toBe(false)
		expect(hasCacheableData({c: {totalCount: 3}})).toBe(true)
	})

	it("accepts scalar root fields", () => {
		expect(hasCacheableData({__typename: "Query"})).toBe(true)
		// `false` and `0` are legitimate answers, not absence.
		expect(hasCacheableData({flag: false})).toBe(true)
		expect(hasCacheableData({count: 0})).toBe(true)
	})

	it("looks through nesting rather than trusting the wrapper", () => {
		expect(hasCacheableData({a: {b: {c: []}}})).toBe(false)
		expect(hasCacheableData({a: {b: {c: [1]}}})).toBe(true)
	})
})
