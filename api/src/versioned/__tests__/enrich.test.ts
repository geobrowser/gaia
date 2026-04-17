import {Effect} from "effect"
import {describe, expect, it, vi} from "vitest"
import type {NormalizedUuid} from "../../utils/uuid"
import {enrichEntityDiffs} from "../enrich"
import type {EntityDiff} from "../types"

function createMockDb() {
	return {
		execute: vi.fn(),
	}
}

function makeDiff(overrides: Partial<EntityDiff> = {}): EntityDiff {
	return {
		entityId: "aaaa0000aaaa0000aaaa0000aaaa0001" as NormalizedUuid,
		name: null,
		values: [],
		relations: [],
		blocks: [],
		...overrides,
	}
}

describe("enrichEntityDiffs", () => {
	it("returns empty array for empty diffs", async () => {
		const db = createMockDb()
		const result = await Effect.runPromise(enrichEntityDiffs(db as any, []))
		expect(result).toEqual([])
		expect(db.execute).not.toHaveBeenCalled()
	})

	it("resolves entity name when diff.name is null", async () => {
		const db = createMockDb()
		const entityId = "aaaa0000aaaa0000aaaa0000aaaa0001" as NormalizedUuid

		db.execute.mockResolvedValueOnce({
			rows: [{entity_id: entityId, text: "My Entity"}],
		})

		const diffs = [makeDiff({entityId, name: null})]
		const result = await Effect.runPromise(enrichEntityDiffs(db as any, diffs))

		expect(result[0]?.name).toBe("My Entity")
	})

	it("preserves existing entity name when already set", async () => {
		const db = createMockDb()
		const entityId = "aaaa0000aaaa0000aaaa0000aaaa0001" as NormalizedUuid

		db.execute.mockResolvedValueOnce({
			rows: [{entity_id: entityId, text: "DB Name"}],
		})

		const diffs = [makeDiff({entityId, name: "Existing Name"})]
		const result = await Effect.runPromise(enrichEntityDiffs(db as any, diffs))

		expect(result[0]?.name).toBe("Existing Name")
	})

	it("resolves propertyName on value changes", async () => {
		const db = createMockDb()
		const propertyId = "bbbb0000bbbb0000bbbb0000bbbb0001" as NormalizedUuid

		db.execute.mockResolvedValueOnce({
			rows: [{entity_id: propertyId, text: "Description"}],
		})

		const diffs = [
			makeDiff({
				values: [
					{
						propertyId,
						spaceId: "cccc0000cccc0000cccc0000cccc0001" as NormalizedUuid,
						type: "TEXT" as const,
						before: null,
						after: "hello",
						diff: [{value: "hello", added: true}],
					},
				],
			}),
		]

		const result = await Effect.runPromise(enrichEntityDiffs(db as any, diffs))
		expect(result[0]?.values[0]?.propertyName).toBe("Description")
	})

	it("resolves typeName and toEntityName on relation changes", async () => {
		const db = createMockDb()
		const typeId = "dddd0000dddd0000dddd0000dddd0001" as NormalizedUuid
		const toEntityId = "eeee0000eeee0000eeee0000eeee0001" as NormalizedUuid

		db.execute.mockResolvedValueOnce({
			rows: [
				{entity_id: typeId, text: "Member Of"},
				{entity_id: toEntityId, text: "Geo DAO"},
			],
		})

		const diffs = [
			makeDiff({
				relations: [
					{
						relationId: "ffff0000ffff0000ffff0000ffff0001" as NormalizedUuid,
						typeId,
						spaceId: "cccc0000cccc0000cccc0000cccc0001" as NormalizedUuid,
						changeType: "ADD" as const,
						before: null,
						after: {
							toEntityId,
							toSpaceId: null,
							position: null,
						},
					},
				],
			}),
		]

		const result = await Effect.runPromise(enrichEntityDiffs(db as any, diffs))
		const rel = result[0]?.relations[0]
		expect(rel?.typeName).toBe("Member Of")
		expect(rel?.after?.toEntityName).toBe("Geo DAO")
	})

	it("sets null for names that cannot be resolved", async () => {
		const db = createMockDb()

		// Return empty — no names found
		db.execute.mockResolvedValueOnce({rows: []})

		const diffs = [
			makeDiff({
				values: [
					{
						propertyId: "bbbb0000bbbb0000bbbb0000bbbb0001" as NormalizedUuid,
						spaceId: "cccc0000cccc0000cccc0000cccc0001" as NormalizedUuid,
						type: "TEXT" as const,
						before: "old",
						after: "new",
						diff: [
							{value: "old", removed: true},
							{value: "new", added: true},
						],
					},
				],
			}),
		]

		const result = await Effect.runPromise(enrichEntityDiffs(db as any, diffs))
		expect(result[0]?.values[0]?.propertyName).toBeNull()
		expect(result[0]?.name).toBeNull()
	})

	it("deduplicates IDs across multiple diffs", async () => {
		const db = createMockDb()
		const sharedPropertyId = "bbbb0000bbbb0000bbbb0000bbbb0001" as NormalizedUuid

		db.execute.mockResolvedValueOnce({
			rows: [{entity_id: sharedPropertyId, text: "Name"}],
		})

		const diffs = [
			makeDiff({
				entityId: "aaaa0000aaaa0000aaaa0000aaaa0001" as NormalizedUuid,
				values: [
					{
						propertyId: sharedPropertyId,
						spaceId: "cccc0000cccc0000cccc0000cccc0001" as NormalizedUuid,
						type: "TEXT" as const,
						before: null,
						after: "a",
						diff: [{value: "a", added: true}],
					},
				],
			}),
			makeDiff({
				entityId: "aaaa0000aaaa0000aaaa0000aaaa0002" as NormalizedUuid,
				values: [
					{
						propertyId: sharedPropertyId,
						spaceId: "cccc0000cccc0000cccc0000cccc0001" as NormalizedUuid,
						type: "TEXT" as const,
						before: null,
						after: "b",
						diff: [{value: "b", added: true}],
					},
				],
			}),
		]

		const result = await Effect.runPromise(enrichEntityDiffs(db as any, diffs))

		// Both diffs should have the same propertyName resolved from a single query
		expect(result[0]?.values[0]?.propertyName).toBe("Name")
		expect(result[1]?.values[0]?.propertyName).toBe("Name")

		// Only one DB call (batch query)
		expect(db.execute).toHaveBeenCalledTimes(1)
	})

	it("handles before-side relation names", async () => {
		const db = createMockDb()
		const toEntityId = "eeee0000eeee0000eeee0000eeee0001" as NormalizedUuid

		db.execute.mockResolvedValueOnce({
			rows: [{entity_id: toEntityId, text: "Removed Entity"}],
		})

		const diffs = [
			makeDiff({
				relations: [
					{
						relationId: "ffff0000ffff0000ffff0000ffff0001" as NormalizedUuid,
						typeId: "dddd0000dddd0000dddd0000dddd0001" as NormalizedUuid,
						spaceId: "cccc0000cccc0000cccc0000cccc0001" as NormalizedUuid,
						changeType: "REMOVE" as const,
						before: {
							toEntityId,
							toSpaceId: null,
							position: null,
						},
						after: null,
					},
				],
			}),
		]

		const result = await Effect.runPromise(enrichEntityDiffs(db as any, diffs))
		expect(result[0]?.relations[0]?.before?.toEntityName).toBe("Removed Entity")
		expect(result[0]?.relations[0]?.after).toBeNull()
	})
})
