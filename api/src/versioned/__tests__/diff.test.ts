import { Effect } from "effect";
import { describe, expect, it } from "vitest";
import {
	diffValues,
	diffRelations,
	diffBlocks,
	diffEntitySnapshots,
	diffGroupedEntitySnapshots,
} from "../diff";
import type {
	VersionedValue,
	VersionedRelation,
	BlockSnapshot,
	EntitySnapshot,
	GroupedEntitySnapshot,
	TextValueChange,
	SimpleValueChange,
} from "../types";
import { SystemIds } from "@graphprotocol/grc-20";

// =============================================================================
// Test Helpers
// =============================================================================

/** Run an Effect synchronously for testing */
const run = <A>(effect: Effect.Effect<A, never, never>): A =>
	Effect.runSync(effect);

function makeTextValue(
	propertyId: string,
	text: string,
	spaceId = "space-1"
): VersionedValue {
	return { propertyId, spaceId, text };
}

function makeIntegerValue(
	propertyId: string,
	integer: number,
	spaceId = "space-1"
): VersionedValue {
	return { propertyId, spaceId, integer };
}

function makeBooleanValue(
	propertyId: string,
	boolean: boolean,
	spaceId = "space-1"
): VersionedValue {
	return { propertyId, spaceId, boolean };
}

function makeRelation(
	relationId: string,
	typeId: string,
	toEntityId: string,
	opts: Partial<VersionedRelation> = {}
): VersionedRelation {
	return {
		relationId,
		typeId,
		fromEntityId: "from-entity",
		toEntityId,
		spaceId: "space-1",
		...opts,
	};
}

function makeTextBlock(id: string, markdownContent: string): BlockSnapshot {
	return {
		id,
		values: [
			makeTextValue(SystemIds.MARKDOWN_CONTENT, markdownContent),
		],
		relations: [
			makeRelation(`${id}-type`, SystemIds.TYPES_PROPERTY, SystemIds.TEXT_BLOCK),
		],
	};
}

function makeImageBlock(id: string, imageUrl: string | null): BlockSnapshot {
	return {
		id,
		values: imageUrl
			? [makeTextValue(SystemIds.IMAGE_URL_PROPERTY, imageUrl)]
			: [],
		relations: [
			makeRelation(`${id}-type`, SystemIds.TYPES_PROPERTY, SystemIds.IMAGE_BLOCK),
		],
	};
}

function makeDataBlock(id: string, name: string | null): BlockSnapshot {
	return {
		id,
		values: name
			? [makeTextValue(SystemIds.NAME_PROPERTY, name)]
			: [],
		relations: [
			makeRelation(`${id}-type`, SystemIds.TYPES_PROPERTY, SystemIds.DATA_BLOCK),
		],
	};
}

// =============================================================================
// diffValues Tests
// =============================================================================

describe("diffValues", () => {
	it("returns empty array when both inputs are empty", () => {
		const result = run(diffValues([], []));
		expect(result).toEqual([]);
	});

	it("returns empty array when values are identical", () => {
		const values = [makeTextValue("prop-1", "hello")];
		const result = run(diffValues(values, values));
		expect(result).toEqual([]);
	});

	it("detects added text value", () => {
		const from: VersionedValue[] = [];
		const to = [makeTextValue("prop-1", "new text")];

		const result = run(diffValues(from, to));

		expect(result).toHaveLength(1);
		const change = result[0] as TextValueChange;
		expect(change.propertyId).toBe("prop-1");
		expect(change.type).toBe("TEXT");
		expect(change.before).toBeNull();
		expect(change.after).toBe("new text");
		expect(change.diff).toContainEqual({ value: "new text", added: true });
	});

	it("detects removed text value", () => {
		const from = [makeTextValue("prop-1", "old text")];
		const to: VersionedValue[] = [];

		const result = run(diffValues(from, to));

		expect(result).toHaveLength(1);
		const change = result[0] as TextValueChange;
		expect(change.propertyId).toBe("prop-1");
		expect(change.type).toBe("TEXT");
		expect(change.before).toBe("old text");
		expect(change.after).toBeNull();
		expect(change.diff).toContainEqual({ value: "old text", removed: true });
	});

	it("detects changed text value with word-level diff", () => {
		const from = [makeTextValue("prop-1", "hello world")];
		const to = [makeTextValue("prop-1", "hello universe")];

		const result = run(diffValues(from, to));

		expect(result).toHaveLength(1);
		const change = result[0] as TextValueChange;
		expect(change.type).toBe("TEXT");
		expect(change.before).toBe("hello world");
		expect(change.after).toBe("hello universe");
		expect(change.diff).toContainEqual({ value: "world", removed: true });
		expect(change.diff).toContainEqual({ value: "universe", added: true });
	});

	it("detects added integer value", () => {
		const from: VersionedValue[] = [];
		const to = [makeIntegerValue("prop-1", 42)];

		const result = run(diffValues(from, to));

		expect(result).toHaveLength(1);
		const change = result[0] as SimpleValueChange;
		expect(change.type).toBe("INT64");
		expect(change.before).toBeNull();
		expect(change.after).toBe("42");
	});

	it("detects changed integer value", () => {
		const from = [makeIntegerValue("prop-1", 10)];
		const to = [makeIntegerValue("prop-1", 20)];

		const result = run(diffValues(from, to));

		expect(result).toHaveLength(1);
		const change = result[0] as SimpleValueChange;
		expect(change.type).toBe("INT64");
		expect(change.before).toBe("10");
		expect(change.after).toBe("20");
	});

	it("detects changed boolean value", () => {
		const from = [makeBooleanValue("prop-1", false)];
		const to = [makeBooleanValue("prop-1", true)];

		const result = run(diffValues(from, to));

		expect(result).toHaveLength(1);
		const change = result[0] as SimpleValueChange;
		expect(change.type).toBe("BOOL");
		expect(change.before).toBe("false");
		expect(change.after).toBe("true");
	});

	it("handles multiple changes in single diff", () => {
		const from = [
			makeTextValue("prop-1", "old"),
			makeIntegerValue("prop-2", 1),
		];
		const to = [
			makeTextValue("prop-1", "new"),
			makeTextValue("prop-3", "added"),
		];

		const result = run(diffValues(from, to));

		expect(result).toHaveLength(3); // changed, removed, added
	});

	it("distinguishes values by space", () => {
		const from = [makeTextValue("prop-1", "space-a-value", "space-a")];
		const to = [
			makeTextValue("prop-1", "space-a-value", "space-a"),
			makeTextValue("prop-1", "space-b-value", "space-b"),
		];

		const result = run(diffValues(from, to));

		// Should only show space-b as added
		expect(result).toHaveLength(1);
		const change = result[0] as TextValueChange;
		expect(change.spaceId).toBe("space-b");
	});
});

// =============================================================================
// diffRelations Tests
// =============================================================================

describe("diffRelations", () => {
	it("returns empty array when both inputs are empty", () => {
		const result = run(diffRelations([], []));
		expect(result).toEqual([]);
	});

	it("returns empty array when relations are identical", () => {
		const relations = [makeRelation("rel-1", "type-1", "to-1")];
		const result = run(diffRelations(relations, relations));
		expect(result).toEqual([]);
	});

	it("detects added relation", () => {
		const from: VersionedRelation[] = [];
		const to = [makeRelation("rel-1", "type-1", "to-1")];

		const result = run(diffRelations(from, to));

		expect(result).toHaveLength(1);
		const change = result[0]!;
		expect(change.relationId).toBe("rel-1");
		expect(change.changeType).toBe("ADD");
		expect(change.before).toBeNull();
		expect(change.after?.toEntityId).toBe("to-1");
	});

	it("detects removed relation", () => {
		const from = [makeRelation("rel-1", "type-1", "to-1")];
		const to: VersionedRelation[] = [];

		const result = run(diffRelations(from, to));

		expect(result).toHaveLength(1);
		const change = result[0]!;
		expect(change.relationId).toBe("rel-1");
		expect(change.changeType).toBe("REMOVE");
		expect(change.before?.toEntityId).toBe("to-1");
		expect(change.after).toBeNull();
	});

	it("detects changed relation target", () => {
		const from = [makeRelation("rel-1", "type-1", "old-target")];
		const to = [makeRelation("rel-1", "type-1", "new-target")];

		const result = run(diffRelations(from, to));

		expect(result).toHaveLength(1);
		const change = result[0]!;
		expect(change.changeType).toBe("UPDATE");
		expect(change.before?.toEntityId).toBe("old-target");
		expect(change.after?.toEntityId).toBe("new-target");
	});

	it("detects changed relation position", () => {
		const from = [makeRelation("rel-1", "type-1", "to-1", { position: "a" })];
		const to = [makeRelation("rel-1", "type-1", "to-1", { position: "b" })];

		const result = run(diffRelations(from, to));

		expect(result).toHaveLength(1);
		const change = result[0]!;
		expect(change.changeType).toBe("UPDATE");
		expect(change.before?.position).toBe("a");
		expect(change.after?.position).toBe("b");
	});

	it("handles multiple relation changes", () => {
		const from = [
			makeRelation("rel-1", "type-1", "to-1"),
			makeRelation("rel-2", "type-1", "to-2"),
		];
		const to = [
			makeRelation("rel-1", "type-1", "to-1-changed"),
			makeRelation("rel-3", "type-1", "to-3"),
		];

		const result = run(diffRelations(from, to));

		expect(result).toHaveLength(3); // updated, removed, added
	});
});

// =============================================================================
// diffBlocks Tests
// =============================================================================

describe("diffBlocks", () => {
	it("returns empty array when both inputs are empty", () => {
		const result = run(diffBlocks([], []));
		expect(result).toEqual([]);
	});

	it("returns empty array when blocks are identical", () => {
		const blocks = [makeTextBlock("block-1", "content")];
		const result = run(diffBlocks(blocks, blocks));
		expect(result).toEqual([]);
	});

	it("detects added text block", () => {
		const from: BlockSnapshot[] = [];
		const to = [makeTextBlock("block-1", "new content")];

		const result = run(diffBlocks(from, to));

		expect(result).toHaveLength(1);
		const change = result[0]!;
		expect(change.id).toBe("block-1");
		expect(change.type).toBe("textBlock");
		if (change.type === "textBlock") {
			expect(change.before).toBeNull();
			expect(change.after).toBe("new content");
			expect(change.diff).toContainEqual({ value: "new content", added: true });
		}
	});

	it("detects removed text block", () => {
		const from = [makeTextBlock("block-1", "old content")];
		const to: BlockSnapshot[] = [];

		const result = run(diffBlocks(from, to));

		expect(result).toHaveLength(1);
		const change = result[0]!;
		expect(change.type).toBe("textBlock");
		if (change.type === "textBlock") {
			expect(change.before).toBe("old content");
			expect(change.after).toBeNull();
			expect(change.diff).toContainEqual({ value: "old content", removed: true });
		}
	});

	it("detects changed text block content", () => {
		const from = [makeTextBlock("block-1", "old content")];
		const to = [makeTextBlock("block-1", "new content")];

		const result = run(diffBlocks(from, to));

		expect(result).toHaveLength(1);
		const change = result[0]!;
		expect(change.type).toBe("textBlock");
		if (change.type === "textBlock") {
			expect(change.before).toBe("old content");
			expect(change.after).toBe("new content");
			expect(change.diff).toContainEqual({ value: "old", removed: true });
			expect(change.diff).toContainEqual({ value: "new", added: true });
		}
	});

	it("detects added image block", () => {
		const from: BlockSnapshot[] = [];
		const to = [makeImageBlock("block-1", "https://example.com/image.png")];

		const result = run(diffBlocks(from, to));

		expect(result).toHaveLength(1);
		const change = result[0]!;
		expect(change.type).toBe("imageBlock");
		if (change.type === "imageBlock") {
			expect(change.before).toBeNull();
			expect(change.after).toBe("https://example.com/image.png");
		}
	});

	it("detects changed image block URL", () => {
		const from = [makeImageBlock("block-1", "https://old.com/image.png")];
		const to = [makeImageBlock("block-1", "https://new.com/image.png")];

		const result = run(diffBlocks(from, to));

		expect(result).toHaveLength(1);
		const change = result[0]!;
		expect(change.type).toBe("imageBlock");
		if (change.type === "imageBlock") {
			expect(change.before).toBe("https://old.com/image.png");
			expect(change.after).toBe("https://new.com/image.png");
		}
	});

	it("detects added data block", () => {
		const from: BlockSnapshot[] = [];
		const to = [makeDataBlock("block-1", "My Data Block")];

		const result = run(diffBlocks(from, to));

		expect(result).toHaveLength(1);
		const change = result[0]!;
		expect(change.type).toBe("dataBlock");
		if (change.type === "dataBlock") {
			expect(change.before).toBeNull();
			expect(change.after).toBe("My Data Block");
		}
	});

	it("handles mixed block types", () => {
		const from = [
			makeTextBlock("text-1", "old text"),
			makeImageBlock("img-1", "https://old.com/img.png"),
		];
		const to = [
			makeTextBlock("text-1", "new text"),
			makeDataBlock("data-1", "New Data"),
		];

		const result = run(diffBlocks(from, to));

		expect(result).toHaveLength(3); // text changed, image removed, data added
	});
});

// =============================================================================
// diffEntitySnapshots Tests
// =============================================================================

describe("diffEntitySnapshots", () => {
	it("returns diff with entity ID and name", () => {
		const from: EntitySnapshot = {
			id: "entity-1",
			values: [makeTextValue(SystemIds.NAME_PROPERTY, "Old Name")],
			relations: [],
			blocks: [],
		};
		const to: EntitySnapshot = {
			id: "entity-1",
			values: [makeTextValue(SystemIds.NAME_PROPERTY, "New Name")],
			relations: [],
			blocks: [],
		};

		const result = run(diffEntitySnapshots("entity-1", from, to));

		expect(result.entityId).toBe("entity-1");
		expect(result.name).toBe("New Name");
	});

	it("computes value, relation, and block diffs together", () => {
		const from: EntitySnapshot = {
			id: "entity-1",
			values: [makeTextValue("prop-1", "old value")],
			relations: [makeRelation("rel-1", "type-1", "target-1")],
			blocks: [makeTextBlock("block-1", "old block")],
		};
		const to: EntitySnapshot = {
			id: "entity-1",
			values: [makeTextValue("prop-1", "new value")],
			relations: [makeRelation("rel-1", "type-1", "target-2")],
			blocks: [makeTextBlock("block-1", "new block")],
		};

		const result = run(diffEntitySnapshots("entity-1", from, to));

		expect(result.values).toHaveLength(1);
		expect(result.relations).toHaveLength(1);
		expect(result.blocks).toHaveLength(1);
	});

	it("uses name from 'to' snapshot, falling back to 'from'", () => {
		const from: EntitySnapshot = {
			id: "entity-1",
			values: [makeTextValue(SystemIds.NAME_PROPERTY, "From Name")],
			relations: [],
			blocks: [],
		};
		const to: EntitySnapshot = {
			id: "entity-1",
			values: [],
			relations: [],
			blocks: [],
		};

		const result = run(diffEntitySnapshots("entity-1", from, to));

		expect(result.name).toBe("From Name");
	});
});

// =============================================================================
// diffGroupedEntitySnapshots Tests
// =============================================================================

function makeGroupedSnapshot(
	id: string,
	opts: Partial<GroupedEntitySnapshot> = {}
): GroupedEntitySnapshot {
	return {
		id,
		values: [],
		relations: [],
		blocks: [],
		groupKeys: [],
		groups: {},
		...opts,
	};
}

describe("diffGroupedEntitySnapshots", () => {
	it("returns correct response shape with all fields", () => {
		const from = makeGroupedSnapshot("entity-1");
		const to = makeGroupedSnapshot("entity-1");

		const result = run(diffGroupedEntitySnapshots("entity-1", from, to));

		// Verify response shape
		expect(result).toHaveProperty("entityId");
		expect(result).toHaveProperty("name");
		expect(result).toHaveProperty("values");
		expect(result).toHaveProperty("relations");
		expect(result).toHaveProperty("blocks");
		expect(result).toHaveProperty("groupKeys");
		expect(result).toHaveProperty("groups");
	});

	it("returns empty arrays when snapshots are identical", () => {
		const snapshot = makeGroupedSnapshot("entity-1", {
			blocks: [makeTextBlock("block-1", "content")],
		});

		const result = run(diffGroupedEntitySnapshots("entity-1", snapshot, snapshot));

		expect(result.values).toEqual([]);
		expect(result.relations).toEqual([]);
		expect(result.blocks).toEqual([]);
		expect(result.groupKeys).toEqual([]);
		expect(result.groups).toEqual({});
	});

	it("computes block diffs in static blocks array", () => {
		const from = makeGroupedSnapshot("entity-1", {
			blocks: [makeTextBlock("block-1", "old content")],
		});
		const to = makeGroupedSnapshot("entity-1", {
			blocks: [makeTextBlock("block-1", "new content")],
		});

		const result = run(diffGroupedEntitySnapshots("entity-1", from, to));

		expect(result.blocks).toHaveLength(1);
		expect(result.blocks[0]!.type).toBe("textBlock");
	});

	it("computes dynamic group diffs", () => {
		const customType = "custom-relation-type";
		const from = makeGroupedSnapshot("entity-1", {
			groupKeys: [customType],
			groups: {
				[customType]: [makeTextBlock("child-1", "old content")],
			},
		});
		const to = makeGroupedSnapshot("entity-1", {
			groupKeys: [customType],
			groups: {
				[customType]: [makeTextBlock("child-1", "new content")],
			},
		});

		const result = run(diffGroupedEntitySnapshots("entity-1", from, to));

		expect(result.groupKeys).toContain(customType);
		expect(result.groups[customType]).toHaveLength(1);
	});

	it("includes groupKeys only for groups with changes", () => {
		const typeA = "type-a";
		const typeB = "type-b";
		const from = makeGroupedSnapshot("entity-1", {
			groupKeys: [typeA, typeB],
			groups: {
				[typeA]: [makeTextBlock("a-1", "unchanged")],
				[typeB]: [makeTextBlock("b-1", "old")],
			},
		});
		const to = makeGroupedSnapshot("entity-1", {
			groupKeys: [typeA, typeB],
			groups: {
				[typeA]: [makeTextBlock("a-1", "unchanged")],
				[typeB]: [makeTextBlock("b-1", "new")],
			},
		});

		const result = run(diffGroupedEntitySnapshots("entity-1", from, to));

		// Only typeB has changes
		expect(result.groupKeys).toEqual([typeB]);
		expect(result.groups[typeA]).toBeUndefined();
		expect(result.groups[typeB]).toHaveLength(1);
	});

	it("handles added dynamic group", () => {
		const customType = "custom-type";
		const from = makeGroupedSnapshot("entity-1");
		const to = makeGroupedSnapshot("entity-1", {
			groupKeys: [customType],
			groups: {
				[customType]: [makeTextBlock("child-1", "new content")],
			},
		});

		const result = run(diffGroupedEntitySnapshots("entity-1", from, to));

		expect(result.groupKeys).toContain(customType);
		expect(result.groups[customType]).toHaveLength(1);
	});

	it("handles removed dynamic group", () => {
		const customType = "custom-type";
		const from = makeGroupedSnapshot("entity-1", {
			groupKeys: [customType],
			groups: {
				[customType]: [makeTextBlock("child-1", "old content")],
			},
		});
		const to = makeGroupedSnapshot("entity-1");

		const result = run(diffGroupedEntitySnapshots("entity-1", from, to));

		expect(result.groupKeys).toContain(customType);
		expect(result.groups[customType]).toHaveLength(1);
	});

	it("handles hybrid mode with blocks and dynamic groups", () => {
		const customType = "custom-type";
		const from = makeGroupedSnapshot("entity-1", {
			blocks: [makeTextBlock("block-1", "old block")],
			groupKeys: [customType],
			groups: {
				[customType]: [makeTextBlock("child-1", "old child")],
			},
		});
		const to = makeGroupedSnapshot("entity-1", {
			blocks: [makeTextBlock("block-1", "new block")],
			groupKeys: [customType],
			groups: {
				[customType]: [makeTextBlock("child-1", "new child")],
			},
		});

		const result = run(diffGroupedEntitySnapshots("entity-1", from, to));

		// Both blocks and dynamic groups have changes
		expect(result.blocks).toHaveLength(1);
		expect(result.groupKeys).toContain(customType);
		expect(result.groups[customType]).toHaveLength(1);
	});

	it("sorts groupKeys alphabetically", () => {
		const from = makeGroupedSnapshot("entity-1", {
			groupKeys: ["zzz-type", "aaa-type"],
			groups: {
				"zzz-type": [makeTextBlock("z-1", "old")],
				"aaa-type": [makeTextBlock("a-1", "old")],
			},
		});
		const to = makeGroupedSnapshot("entity-1", {
			groupKeys: ["zzz-type", "aaa-type"],
			groups: {
				"zzz-type": [makeTextBlock("z-1", "new")],
				"aaa-type": [makeTextBlock("a-1", "new")],
			},
		});

		const result = run(diffGroupedEntitySnapshots("entity-1", from, to));

		expect(result.groupKeys).toEqual(["aaa-type", "zzz-type"]);
	});

	it("uses name from to snapshot, falling back to from", () => {
		const from = makeGroupedSnapshot("entity-1", {
			values: [makeTextValue(SystemIds.NAME_PROPERTY, "From Name")],
		});
		const to = makeGroupedSnapshot("entity-1");

		const result = run(diffGroupedEntitySnapshots("entity-1", from, to));

		expect(result.name).toBe("From Name");
	});
});
