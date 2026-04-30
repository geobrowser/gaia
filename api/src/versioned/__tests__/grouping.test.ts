import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {describe, expect, it} from "vitest"
import {type NormalizedUuid, normalizeUuid} from "../../utils/uuid"
import {type DiscoveredEntity, groupEntitiesByContext, mergeDiscoveryResults} from "../grouping"

// =============================================================================
// Test Helpers
// =============================================================================

/** Cast a synthetic test string to NormalizedUuid. */
const nuuid = (s: string) => s as NormalizedUuid

/** Normalize a real UUID (SystemIds) to match the dashless format used in source code. */
const norm = normalizeUuid

/**
 * Helper to run Effects synchronously in tests.
 */
const run = <A>(effect: Effect.Effect<A, never, never>): A => Effect.runSync(effect)

function makeEntity(
	entityId: NormalizedUuid,
	contextEdgeTypeId: NormalizedUuid | null = null,
	position: string | null = null,
): DiscoveredEntity {
	return {entityId, contextEdgeTypeId, position}
}

// =============================================================================
// groupEntitiesByContext Tests
// =============================================================================

describe("groupEntitiesByContext", () => {
	describe("empty input", () => {
		it("returns empty results for empty input", () => {
			const result = run(groupEntitiesByContext([]))

			expect(result.blocks).toEqual([])
			expect(result.dynamicGroups.size).toBe(0)
			expect(result.groupKeys).toEqual([])
		})
	})

	describe("blocks grouping (static key)", () => {
		it("groups entities with BLOCKS context into blocks array", () => {
			const entities = [
				makeEntity(nuuid("block-1"), norm(SystemIds.BLOCKS)),
				makeEntity(nuuid("block-2"), norm(SystemIds.BLOCKS)),
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.blocks).toEqual([nuuid("block-1"), nuuid("block-2")])
			expect(result.dynamicGroups.size).toBe(0)
			expect(result.groupKeys).toEqual([])
		})

		it("groups entities with null context (relation fallback) into blocks", () => {
			const entities = [makeEntity(nuuid("block-1"), null), makeEntity(nuuid("block-2"), null)]

			const result = run(groupEntitiesByContext(entities))

			expect(result.blocks).toEqual([nuuid("block-1"), nuuid("block-2")])
			expect(result.dynamicGroups.size).toBe(0)
		})

		it("uses custom fallback type when provided", () => {
			const customType = nuuid("custom-type-id")
			const entities = [makeEntity(nuuid("entity-1"), null)]

			const result = run(groupEntitiesByContext(entities, customType))

			// null contextEdgeTypeId falls back to blocksTypeId, which is customType here,
			// so the entity lands in blocks (the static bucket for the provided blocksTypeId)
			expect(result.blocks).toEqual([nuuid("entity-1")])
			expect(result.dynamicGroups.size).toBe(0)
			expect(result.groupKeys).toEqual([])
		})
	})

	describe("dynamic grouping", () => {
		it("groups entities with non-BLOCKS context into dynamic groups", () => {
			const customTypeA = nuuid("type-a")
			const customTypeB = nuuid("type-b")
			const entities = [
				makeEntity(nuuid("entity-1"), customTypeA),
				makeEntity(nuuid("entity-2"), customTypeB),
				makeEntity(nuuid("entity-3"), customTypeA),
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.blocks).toEqual([])
			expect(result.dynamicGroups.get(customTypeA)).toEqual([nuuid("entity-1"), nuuid("entity-3")])
			expect(result.dynamicGroups.get(customTypeB)).toEqual([nuuid("entity-2")])
			expect(result.groupKeys).toEqual([customTypeA, customTypeB].sort())
		})

		it("returns sorted groupKeys for discoverability", () => {
			const entities = [
				makeEntity(nuuid("e1"), nuuid("zzz-type")),
				makeEntity(nuuid("e2"), nuuid("aaa-type")),
				makeEntity(nuuid("e3"), nuuid("mmm-type")),
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.groupKeys).toEqual([nuuid("aaa-type"), nuuid("mmm-type"), nuuid("zzz-type")])
		})
	})

	describe("hybrid mode (blocks + dynamic)", () => {
		it("groups blocks and dynamic keys together", () => {
			const customType = nuuid("custom-type")
			const entities = [
				makeEntity(nuuid("block-1"), norm(SystemIds.BLOCKS)),
				makeEntity(nuuid("custom-1"), customType),
				makeEntity(nuuid("block-2"), norm(SystemIds.BLOCKS)),
				makeEntity(nuuid("custom-2"), customType),
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.blocks).toEqual([nuuid("block-1"), nuuid("block-2")])
			expect(result.dynamicGroups.get(customType)).toEqual([nuuid("custom-1"), nuuid("custom-2")])
			expect(result.groupKeys).toEqual([customType])
		})

		it("handles mix of context-based and fallback entities", () => {
			const customType = nuuid("custom-type")
			const entities = [
				makeEntity(nuuid("block-context"), norm(SystemIds.BLOCKS)),
				makeEntity(nuuid("block-fallback"), null), // null = fallback to BLOCKS
				makeEntity(nuuid("custom-1"), customType),
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.blocks).toContain(nuuid("block-context"))
			expect(result.blocks).toContain(nuuid("block-fallback"))
			expect(result.dynamicGroups.get(customType)).toEqual([nuuid("custom-1")])
		})
	})

	describe("deduplication", () => {
		it("deduplicates entities with same ID", () => {
			const entities = [
				makeEntity(nuuid("entity-1"), norm(SystemIds.BLOCKS)),
				makeEntity(nuuid("entity-1"), norm(SystemIds.BLOCKS)), // duplicate
				makeEntity(nuuid("entity-1"), null), // duplicate via fallback
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.blocks).toEqual([nuuid("entity-1")])
		})

		it("keeps first occurrence when deduplicating", () => {
			const customType = nuuid("custom-type")
			const entities = [
				makeEntity(nuuid("entity-1"), norm(SystemIds.BLOCKS), "a"),
				makeEntity(nuuid("entity-1"), customType, "b"), // same ID, different type
			]

			const result = run(groupEntitiesByContext(entities))

			// First occurrence wins (BLOCKS)
			expect(result.blocks).toEqual([nuuid("entity-1")])
			expect(result.dynamicGroups.size).toBe(0)
		})

		it("context discovery wins over BLOCKS-relation fallback (fallback first)", () => {
			const customType = nuuid("custom-type")
			const entities = [
				// Relation fallback arrives first with a position
				makeEntity(nuuid("entity-1"), null, "pos-a"),
				// Then context metadata pins it to a dynamic type
				makeEntity(nuuid("entity-1"), customType, null),
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.blocks).toEqual([])
			expect(result.dynamicGroups.get(customType)).toEqual([nuuid("entity-1")])
		})

		it("context discovery wins over BLOCKS-relation fallback (context first)", () => {
			const customType = nuuid("custom-type")
			const entities = [
				makeEntity(nuuid("entity-1"), customType, null),
				makeEntity(nuuid("entity-1"), null, "pos-a"),
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.dynamicGroups.get(customType)).toEqual([nuuid("entity-1")])
		})

		it("inherits position from relation entry when context entry has none", () => {
			const customTypeA = nuuid("type-a")
			const entities = [
				makeEntity(nuuid("e1"), customTypeA, null),
				// Same ID with a position
				makeEntity(nuuid("e1"), null, "pos-1"),
				// Another in the same group sorts after via position
				makeEntity(nuuid("e2"), customTypeA, "pos-2"),
			]

			const result = run(groupEntitiesByContext(entities))

			// e1 inherited pos-1 so it comes before e2 (pos-2)
			expect(result.dynamicGroups.get(customTypeA)).toEqual([nuuid("e1"), nuuid("e2")])
		})
	})

	describe("position-based ordering", () => {
		it("sorts entities by position", () => {
			const entities = [
				makeEntity(nuuid("block-c"), norm(SystemIds.BLOCKS), "c"),
				makeEntity(nuuid("block-a"), norm(SystemIds.BLOCKS), "a"),
				makeEntity(nuuid("block-b"), norm(SystemIds.BLOCKS), "b"),
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.blocks).toEqual([nuuid("block-a"), nuuid("block-b"), nuuid("block-c")])
		})

		it("puts null positions last", () => {
			const entities = [
				makeEntity(nuuid("block-null"), norm(SystemIds.BLOCKS), null),
				makeEntity(nuuid("block-a"), norm(SystemIds.BLOCKS), "a"),
				makeEntity(nuuid("block-b"), norm(SystemIds.BLOCKS), "b"),
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.blocks).toEqual([nuuid("block-a"), nuuid("block-b"), nuuid("block-null")])
		})

		it("maintains order within dynamic groups", () => {
			const customType = nuuid("custom-type")
			const entities = [
				makeEntity(nuuid("e-c"), customType, "c"),
				makeEntity(nuuid("e-a"), customType, "a"),
				makeEntity(nuuid("e-b"), customType, "b"),
			]

			const result = run(groupEntitiesByContext(entities))

			expect(result.dynamicGroups.get(customType)).toEqual([nuuid("e-a"), nuuid("e-b"), nuuid("e-c")])
		})
	})
})

// =============================================================================
// mergeDiscoveryResults Tests
// =============================================================================

describe("mergeDiscoveryResults", () => {
	it("returns empty array for empty inputs", () => {
		const result = run(mergeDiscoveryResults([], [], norm(SystemIds.BLOCKS)))
		expect(result).toEqual([])
	})

	it("passes through context entities unchanged", () => {
		const contextEntities = [
			makeEntity(nuuid("e1"), nuuid("type-a"), "pos-1"),
			makeEntity(nuuid("e2"), nuuid("type-b"), "pos-2"),
		]

		const result = run(mergeDiscoveryResults(contextEntities, [], norm(SystemIds.BLOCKS)))

		expect(result).toEqual(contextEntities)
	})

	it("converts relation entities to discovered entities with null context", () => {
		const relationEntities = [
			{entityId: nuuid("e1"), position: "pos-1"},
			{entityId: nuuid("e2"), position: null},
		]

		const result = run(mergeDiscoveryResults([], relationEntities, norm(SystemIds.BLOCKS)))

		expect(result).toEqual([
			{entityId: nuuid("e1"), contextEdgeTypeId: null, position: "pos-1"},
			{entityId: nuuid("e2"), contextEdgeTypeId: null, position: null},
		])
	})

	it("merges context and relation entities", () => {
		const contextEntities = [makeEntity(nuuid("context-1"), nuuid("type-a"))]
		const relationEntities = [{entityId: nuuid("relation-1"), position: "pos-1"}]

		const result = run(mergeDiscoveryResults(contextEntities, relationEntities, norm(SystemIds.BLOCKS)))

		expect(result).toHaveLength(2)
		expect(result[0]).toEqual(contextEntities[0])
		expect(result[1]).toEqual({
			entityId: nuuid("relation-1"),
			contextEdgeTypeId: null,
			position: "pos-1",
		})
	})
})
