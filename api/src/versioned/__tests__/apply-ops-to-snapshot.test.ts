/**
 * Unit tests for `applyOpsToSnapshot` — the pure op-replay function that builds an
 * entity's proposed (after) state from a base snapshot + GRC-20 ops.
 *
 * No DB. This is the op-application core shared by the proposal diff and the new
 * review endpoint (`computeEnrichedOpsDiff`), so it gets direct coverage of every
 * op type the editor can emit, the BLOCKS-relation special-casing, multi-op
 * sequencing, the documented restore no-ops, and the no-mutation guarantee —
 * independent of the DB-gated integration suites.
 */

import {EditBuilder, type Id, type Op, parseId} from "@geoprotocol/grc-20"
import {SystemIds} from "@graphprotocol/grc-20"
import {describe, expect, it} from "vitest"
import {normalizeUuid} from "../../utils/uuid"
import {applyOpsToSnapshot} from "../proposal-diff"
import type {EntitySnapshot, VersionedRelation} from "../types"

const n = normalizeUuid
const uuidToId = (u: string): Id => {
	const id = parseId(u)
	if (!id) throw new Error(`invalid uuid: ${u}`)
	return id
}

// Fixture uuids (10000000-* prefix; arbitrary, just need to be valid + distinct).
const SPACE = "10000000-0000-4000-8000-000000000001"
const EDIT = "10000000-0000-4000-8000-0000000000ed"
const ENTITY = "10000000-0000-4000-8000-000000000010"
const PROP_A = "10000000-0000-4000-8000-0000000000a1"
const PROP_B = "10000000-0000-4000-8000-0000000000a2"
const REL = "10000000-0000-4000-8000-0000000000b1"
const REL_TYPE = "10000000-0000-4000-8000-0000000000c1"
const TARGET = "10000000-0000-4000-8000-0000000000d1"
const BLOCK = "10000000-0000-4000-8000-0000000000e1"

/** Build a real GRC-20 Op[] via the SDK builder (same shape decodeEditAuto yields). */
function ops(build: (e: EditBuilder) => EditBuilder): Op[] {
	return build(new EditBuilder(uuidToId(EDIT)).setName("test")).build().ops
}

const empty = (id: string): EntitySnapshot => ({id: n(id), values: [], relations: [], blocks: []})
const rel = (overrides: Partial<VersionedRelation> = {}): VersionedRelation => ({
	relationId: n(REL),
	typeId: n(REL_TYPE),
	fromEntityId: n(ENTITY),
	toEntityId: n(TARGET),
	spaceId: n(SPACE),
	...overrides,
})

const apply = (base: EntitySnapshot, opList: Op[], blocksMap = new Map<string, string>()) =>
	applyOpsToSnapshot(base, opList, n(ENTITY), n(SPACE), blocksMap as never)

describe("applyOpsToSnapshot", () => {
	describe("values", () => {
		it("createEntity sets values", () => {
			const out = apply(
				empty(ENTITY),
				ops((e) => e.createEntity(uuidToId(ENTITY), (b) => b.text(uuidToId(PROP_A), "hello"))),
			)
			expect(out.values).toHaveLength(1)
			expect(out.values[0]?.propertyId).toBe(n(PROP_A))
			expect(out.values[0]?.text).toBe("hello")
		})

		it("updateEntity set replaces an existing value", () => {
			const base: EntitySnapshot = {
				id: n(ENTITY),
				values: [{propertyId: n(PROP_A), spaceId: n(SPACE), text: "old"}],
				relations: [],
				blocks: [],
			}
			const out = apply(
				base,
				ops((e) => e.updateEntity(uuidToId(ENTITY), (u) => u.setText(uuidToId(PROP_A), "new"))),
			)
			expect(out.values).toHaveLength(1)
			expect(out.values[0]?.text).toBe("new")
		})

		it("updateEntity set adds a new property", () => {
			const base: EntitySnapshot = {
				id: n(ENTITY),
				values: [{propertyId: n(PROP_A), spaceId: n(SPACE), text: "a"}],
				relations: [],
				blocks: [],
			}
			const out = apply(
				base,
				ops((e) => e.updateEntity(uuidToId(ENTITY), (u) => u.setText(uuidToId(PROP_B), "b"))),
			)
			expect(out.values.map((v) => v.propertyId).sort()).toEqual([n(PROP_A), n(PROP_B)].sort())
		})

		it("updateEntity unset removes a value", () => {
			const base: EntitySnapshot = {
				id: n(ENTITY),
				values: [{propertyId: n(PROP_A), spaceId: n(SPACE), text: "old"}],
				relations: [],
				blocks: [],
			}
			const out = apply(
				base,
				ops((e) => e.updateEntity(uuidToId(ENTITY), (u) => u.unsetAll(uuidToId(PROP_A)))),
			)
			expect(out.values).toHaveLength(0)
		})

		it("ignores ops targeting a different entity", () => {
			const out = apply(
				empty(ENTITY),
				ops((e) => e.createEntity(uuidToId(TARGET), (b) => b.text(uuidToId(PROP_A), "not mine"))),
			)
			expect(out.values).toHaveLength(0)
		})
	})

	describe("relations", () => {
		it("createRelation (non-BLOCKS) adds a relation", () => {
			const out = apply(
				empty(ENTITY),
				ops((e) =>
					e.createRelationSimple(uuidToId(REL), uuidToId(ENTITY), uuidToId(TARGET), uuidToId(REL_TYPE)),
				),
			)
			expect(out.relations).toHaveLength(1)
			expect(out.relations[0]?.toEntityId).toBe(n(TARGET))
			expect(out.relations[0]?.typeId).toBe(n(REL_TYPE))
		})

		it("deleteRelation removes a relation", () => {
			const base: EntitySnapshot = {id: n(ENTITY), values: [], relations: [rel()], blocks: []}
			const out = apply(
				base,
				ops((e) => e.deleteRelation(uuidToId(REL))),
			)
			expect(out.relations).toHaveLength(0)
		})

		it("updateRelation changes position", () => {
			const base: EntitySnapshot = {
				id: n(ENTITY),
				values: [],
				relations: [rel({position: "a0"})],
				blocks: [],
			}
			const out = apply(
				base,
				ops((e) => e.updateRelation(uuidToId(REL), (u) => u.setPosition("zz"))),
			)
			expect(out.relations[0]?.position).toBe("zz")
		})
	})

	describe("BLOCKS relations", () => {
		it("createRelation BLOCKS adds a block and tracks the reified relation id", () => {
			const map = new Map<string, string>()
			const out = apply(
				empty(ENTITY),
				ops((e) =>
					e.createRelationSimple(
						uuidToId(REL),
						uuidToId(ENTITY),
						uuidToId(BLOCK),
						uuidToId(SystemIds.BLOCKS),
					),
				),
				map,
			)
			expect(out.blocks.map((b) => b.id)).toContain(n(BLOCK))
			// the BLOCKS relation does NOT leak into relations[]
			expect(out.relations).toHaveLength(0)
			expect(map.get(n(REL))).toBe(n(BLOCK))
		})

		it("deleteRelation on a BLOCKS relation removes the block", () => {
			const base: EntitySnapshot = {
				id: n(ENTITY),
				values: [],
				relations: [],
				blocks: [{id: n(BLOCK), values: [], relations: []}],
			}
			const map = new Map<string, string>([[n(REL), n(BLOCK)]])
			const out = apply(
				base,
				ops((e) => e.deleteRelation(uuidToId(REL))),
				map,
			)
			expect(out.blocks).toHaveLength(0)
		})
	})

	describe("deleteEntity", () => {
		it("clears all values, relations, and blocks", () => {
			const base: EntitySnapshot = {
				id: n(ENTITY),
				values: [{propertyId: n(PROP_A), spaceId: n(SPACE), text: "x"}],
				relations: [rel()],
				blocks: [{id: n(BLOCK), values: [], relations: []}],
			}
			const out = apply(
				base,
				ops((e) => e.deleteEntity(uuidToId(ENTITY))),
			)
			expect(out.values).toHaveLength(0)
			expect(out.relations).toHaveLength(0)
			expect(out.blocks).toHaveLength(0)
		})
	})

	describe("documented no-ops (need historical state — see KNOWN LIMITATIONS)", () => {
		it("restoreEntity leaves the snapshot unchanged", () => {
			const base: EntitySnapshot = {
				id: n(ENTITY),
				values: [{propertyId: n(PROP_A), spaceId: n(SPACE), text: "keep"}],
				relations: [],
				blocks: [],
			}
			const out = apply(
				base,
				ops((e) => e.restoreEntity(uuidToId(ENTITY))),
			)
			expect(out.values).toHaveLength(1)
			expect(out.values[0]?.text).toBe("keep")
		})

		it("restoreRelation leaves the snapshot unchanged", () => {
			const base: EntitySnapshot = {id: n(ENTITY), values: [], relations: [rel()], blocks: []}
			const out = apply(
				base,
				ops((e) => e.restoreRelation(uuidToId(REL))),
			)
			expect(out.relations).toHaveLength(1)
		})
	})

	describe("sequencing & immutability", () => {
		it("applies a later unset op after an earlier set op on the same entity", () => {
			// Two separate ops: op1 sets A and B, op2 unsets A → only B survives.
			// (Within a *single* updateEntity op, set-after-unset means set wins, since
			// applyOpsToSnapshot applies that op's unsets before its sets.)
			const out = apply(
				empty(ENTITY),
				ops((e) =>
					e
						.updateEntity(uuidToId(ENTITY), (u) =>
							u.setText(uuidToId(PROP_A), "x").setText(uuidToId(PROP_B), "y"),
						)
						.updateEntity(uuidToId(ENTITY), (u) => u.unsetAll(uuidToId(PROP_A))),
				),
			)
			expect(out.values).toHaveLength(1)
			expect(out.values[0]?.propertyId).toBe(n(PROP_B))
			expect(out.values[0]?.text).toBe("y")
		})

		it("does not mutate the base snapshot", () => {
			const base: EntitySnapshot = {
				id: n(ENTITY),
				values: [{propertyId: n(PROP_A), spaceId: n(SPACE), text: "old"}],
				relations: [rel()],
				blocks: [],
			}
			apply(
				base,
				ops((e) =>
					e
						.updateEntity(uuidToId(ENTITY), (u) => u.setText(uuidToId(PROP_A), "new"))
						.deleteRelation(uuidToId(REL)),
				),
			)
			expect(base.values[0]?.text).toBe("old")
			expect(base.relations).toHaveLength(1)
		})
	})
})
