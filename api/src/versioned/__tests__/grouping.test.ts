import { describe, expect, it } from "vitest";
import { Effect } from "effect";
import { SystemIds } from "@graphprotocol/grc-20";
import {
	groupEntitiesByContext,
	mergeDiscoveryResults,
	type DiscoveredEntity,
} from "../grouping";

// =============================================================================
// Test Helpers
// =============================================================================

/**
 * Helper to run Effects synchronously in tests.
 */
const run = <A>(effect: Effect.Effect<A, never, never>): A =>
	Effect.runSync(effect);

function makeEntity(
	entityId: string,
	contextEdgeTypeId: string | null = null,
	position: string | null = null
): DiscoveredEntity {
	return { entityId, contextEdgeTypeId, position };
}

// =============================================================================
// groupEntitiesByContext Tests
// =============================================================================

describe("groupEntitiesByContext", () => {
	describe("empty input", () => {
		it("returns empty results for empty input", () => {
			const result = run(groupEntitiesByContext([]));

			expect(result.blocks).toEqual([]);
			expect(result.dynamicGroups.size).toBe(0);
			expect(result.groupKeys).toEqual([]);
		});
	});

	describe("blocks grouping (static key)", () => {
		it("groups entities with BLOCKS context into blocks array", () => {
			const entities = [
				makeEntity("block-1", SystemIds.BLOCKS),
				makeEntity("block-2", SystemIds.BLOCKS),
			];

			const result = run(groupEntitiesByContext(entities));

			expect(result.blocks).toEqual(["block-1", "block-2"]);
			expect(result.dynamicGroups.size).toBe(0);
			expect(result.groupKeys).toEqual([]);
		});

		it("groups entities with null context (relation fallback) into blocks", () => {
			const entities = [
				makeEntity("block-1", null),
				makeEntity("block-2", null),
			];

			const result = run(groupEntitiesByContext(entities));

			expect(result.blocks).toEqual(["block-1", "block-2"]);
			expect(result.dynamicGroups.size).toBe(0);
		});

		it("uses custom fallback type when provided", () => {
			const customType = "custom-type-id";
			const entities = [makeEntity("entity-1", null)];

			const result = run(groupEntitiesByContext(entities, customType));

			// Should go into dynamic groups, not blocks
			expect(result.blocks).toEqual([]);
			expect(result.dynamicGroups.get(customType)).toEqual(["entity-1"]);
			expect(result.groupKeys).toEqual([customType]);
		});
	});

	describe("dynamic grouping", () => {
		it("groups entities with non-BLOCKS context into dynamic groups", () => {
			const customTypeA = "type-a";
			const customTypeB = "type-b";
			const entities = [
				makeEntity("entity-1", customTypeA),
				makeEntity("entity-2", customTypeB),
				makeEntity("entity-3", customTypeA),
			];

			const result = run(groupEntitiesByContext(entities));

			expect(result.blocks).toEqual([]);
			expect(result.dynamicGroups.get(customTypeA)).toEqual([
				"entity-1",
				"entity-3",
			]);
			expect(result.dynamicGroups.get(customTypeB)).toEqual(["entity-2"]);
			expect(result.groupKeys).toEqual([customTypeA, customTypeB].sort());
		});

		it("returns sorted groupKeys for discoverability", () => {
			const entities = [
				makeEntity("e1", "zzz-type"),
				makeEntity("e2", "aaa-type"),
				makeEntity("e3", "mmm-type"),
			];

			const result = run(groupEntitiesByContext(entities));

			expect(result.groupKeys).toEqual(["aaa-type", "mmm-type", "zzz-type"]);
		});
	});

	describe("hybrid mode (blocks + dynamic)", () => {
		it("groups blocks and dynamic keys together", () => {
			const customType = "custom-type";
			const entities = [
				makeEntity("block-1", SystemIds.BLOCKS),
				makeEntity("custom-1", customType),
				makeEntity("block-2", SystemIds.BLOCKS),
				makeEntity("custom-2", customType),
			];

			const result = run(groupEntitiesByContext(entities));

			expect(result.blocks).toEqual(["block-1", "block-2"]);
			expect(result.dynamicGroups.get(customType)).toEqual([
				"custom-1",
				"custom-2",
			]);
			expect(result.groupKeys).toEqual([customType]);
		});

		it("handles mix of context-based and fallback entities", () => {
			const customType = "custom-type";
			const entities = [
				makeEntity("block-context", SystemIds.BLOCKS),
				makeEntity("block-fallback", null), // null = fallback to BLOCKS
				makeEntity("custom-1", customType),
			];

			const result = run(groupEntitiesByContext(entities));

			expect(result.blocks).toContain("block-context");
			expect(result.blocks).toContain("block-fallback");
			expect(result.dynamicGroups.get(customType)).toEqual(["custom-1"]);
		});
	});

	describe("deduplication", () => {
		it("deduplicates entities with same ID", () => {
			const entities = [
				makeEntity("entity-1", SystemIds.BLOCKS),
				makeEntity("entity-1", SystemIds.BLOCKS), // duplicate
				makeEntity("entity-1", null), // duplicate via fallback
			];

			const result = run(groupEntitiesByContext(entities));

			expect(result.blocks).toEqual(["entity-1"]);
		});

		it("keeps first occurrence when deduplicating", () => {
			const customType = "custom-type";
			const entities = [
				makeEntity("entity-1", SystemIds.BLOCKS, "a"),
				makeEntity("entity-1", customType, "b"), // same ID, different type
			];

			const result = run(groupEntitiesByContext(entities));

			// First occurrence wins (BLOCKS)
			expect(result.blocks).toEqual(["entity-1"]);
			expect(result.dynamicGroups.size).toBe(0);
		});
	});

	describe("position-based ordering", () => {
		it("sorts entities by position", () => {
			const entities = [
				makeEntity("block-c", SystemIds.BLOCKS, "c"),
				makeEntity("block-a", SystemIds.BLOCKS, "a"),
				makeEntity("block-b", SystemIds.BLOCKS, "b"),
			];

			const result = run(groupEntitiesByContext(entities));

			expect(result.blocks).toEqual(["block-a", "block-b", "block-c"]);
		});

		it("puts null positions last", () => {
			const entities = [
				makeEntity("block-null", SystemIds.BLOCKS, null),
				makeEntity("block-a", SystemIds.BLOCKS, "a"),
				makeEntity("block-b", SystemIds.BLOCKS, "b"),
			];

			const result = run(groupEntitiesByContext(entities));

			expect(result.blocks).toEqual(["block-a", "block-b", "block-null"]);
		});

		it("maintains order within dynamic groups", () => {
			const customType = "custom-type";
			const entities = [
				makeEntity("e-c", customType, "c"),
				makeEntity("e-a", customType, "a"),
				makeEntity("e-b", customType, "b"),
			];

			const result = run(groupEntitiesByContext(entities));

			expect(result.dynamicGroups.get(customType)).toEqual([
				"e-a",
				"e-b",
				"e-c",
			]);
		});
	});
});

// =============================================================================
// mergeDiscoveryResults Tests
// =============================================================================

describe("mergeDiscoveryResults", () => {
	it("returns empty array for empty inputs", () => {
		const result = run(mergeDiscoveryResults([], [], SystemIds.BLOCKS));
		expect(result).toEqual([]);
	});

	it("passes through context entities unchanged", () => {
		const contextEntities = [
			makeEntity("e1", "type-a", "pos-1"),
			makeEntity("e2", "type-b", "pos-2"),
		];

		const result = run(mergeDiscoveryResults(contextEntities, [], SystemIds.BLOCKS));

		expect(result).toEqual(contextEntities);
	});

	it("converts relation entities to discovered entities with null context", () => {
		const relationEntities = [
			{ entityId: "e1", position: "pos-1" },
			{ entityId: "e2", position: null },
		];

		const result = run(mergeDiscoveryResults([], relationEntities, SystemIds.BLOCKS));

		expect(result).toEqual([
			{ entityId: "e1", contextEdgeTypeId: null, position: "pos-1" },
			{ entityId: "e2", contextEdgeTypeId: null, position: null },
		]);
	});

	it("merges context and relation entities", () => {
		const contextEntities = [makeEntity("context-1", "type-a")];
		const relationEntities = [{ entityId: "relation-1", position: "pos-1" }];

		const result = run(mergeDiscoveryResults(
			contextEntities,
			relationEntities,
			SystemIds.BLOCKS
		));

		expect(result).toHaveLength(2);
		expect(result[0]).toEqual(contextEntities[0]);
		expect(result[1]).toEqual({
			entityId: "relation-1",
			contextEdgeTypeId: null,
			position: "pos-1",
		});
	});
});
