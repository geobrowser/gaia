import {mkdtempSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {blockInstant, CREATED_AT_PROPERTY, UPDATED_AT_PROPERTY} from "../entityTimes.js"
import {type Decode, handleMessage} from "../handle.js"
import {conformingId} from "../ids.js"
import {createMetrics} from "../metrics.js"
import {Store} from "../state.js"
import {type GrcOp, type TranslateContext, translateOps} from "../translate.js"

const id = (n: number) => new Uint8Array(Array.from({length: 16}, (_, i) => (i === 0 ? n : i === 6 ? 0x40 : i === 8 ? 0x80 : i)))
const hex = (n: number) => Buffer.from(id(n)).toString("hex")
const SECONDS = 1_700_000_000
const MICROS = "1700000000000000"

function context(known: string[] = []) {
	const entities = new Set<string>(known)
	const statements = new Set<string>()
	const ctx: TranslateContext = {
		knownEntity: (e) => entities.has(e),
		markEntity: (e) => void entities.add(e),
		knownStmt: (s) => statements.has(s),
		markStmt: (s) => void statements.add(s),
	}
	return {ctx, entities, statements}
}

const times = (intents: string[], property: string) => intents.filter((i) => i.includes(`.property = "${property}"`))
const entityOf = (intent: string) => /\.entity = "([0-9a-f]+)"/.exec(intent)?.[1]
const value = (n: number): GrcOp["values"] => [{property: id(n), value: {type: "integer", value: 1}}]

describe("reserved time properties", () => {
	it("pins the ids the gateway mirrors", () => {
		expect(CREATED_AT_PROPERTY).toBe(conformingId("reserved-property", "createdAt"))
		expect(UPDATED_AT_PROPERTY).toBe(conformingId("reserved-property", "updatedAt"))
		expect(CREATED_AT_PROPERTY).not.toBe(UPDATED_AT_PROPERTY)
		expect(CREATED_AT_PROPERTY).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
		// Literals; api/src/geodb/card.ts carries the same two (ENTITY_TIME_PROPERTIES).
		expect(CREATED_AT_PROPERTY).toBe("74d9c34835444345969a3c9d70362ea5")
		expect(UPDATED_AT_PROPERTY).toBe("c5f5b46a774442acb138c3cba948acec")
	})

	it("turns block seconds into microseconds, and refuses nonsense", () => {
		expect(blockInstant(SECONDS)).toBe(MICROS)
		expect(blockInstant(0)).toBe("0")
		expect(blockInstant(null)).toBeNull()
		expect(blockInstant(undefined)).toBeNull()
		expect(blockInstant(-1)).toBeNull()
		expect(blockInstant(1.5)).toBeNull()
		expect(blockInstant(Number.NaN)).toBeNull()
	})
})

describe("translateOps writes the times", () => {
	it("writes nothing when the option is absent", () => {
		const {ctx} = context()
		const r = translateOps([{type: "createEntity", id: id(1), values: value(2)}], ctx)
		expect(times(r.intents, UPDATED_AT_PROPERTY)).toHaveLength(0)
		expect(times(r.intents, CREATED_AT_PROPERTY)).toHaveLength(0)
	})

	it("a new entity gets createdAt and updatedAt after its create and values, both as the block time", () => {
		const {ctx} = context()
		const r = translateOps([{type: "createEntity", id: id(1), values: value(2)}], ctx, {blockSeconds: SECONDS})
		const e = conformingId("entity", hex(1))
		expect(r.intents.slice(-2)).toEqual([
			`.{ .set = .{ .entity = "${e}", .property = "${CREATED_AT_PROPERTY}", .value = .{ .instant = "${MICROS}" } } }`,
			`.{ .set = .{ .entity = "${e}", .property = "${UPDATED_AT_PROPERTY}", .value = .{ .instant = "${MICROS}" } } }`,
		])
		expect(r.stats["times-created-at"]).toBe(1)
		expect(r.stats["times-updated-at"]).toBe(1)
	})

	it("a known entity's value edit moves updatedAt only, never createdAt", () => {
		const e = conformingId("entity", hex(1))
		const {ctx} = context([e])
		const r = translateOps([{type: "updateEntity", id: id(1), set: value(2)}], ctx, {blockSeconds: SECONDS})
		expect(times(r.intents, CREATED_AT_PROPERTY)).toHaveLength(0)
		expect(times(r.intents, UPDATED_AT_PROPERTY)).toHaveLength(1)
	})

	it("an unset counts as a change", () => {
		const e = conformingId("entity", hex(1))
		const {ctx} = context([e])
		const r = translateOps([{type: "updateEntity", id: id(1), unset: [id(2)]}], ctx, {blockSeconds: SECONDS})
		expect(times(r.intents, UPDATED_AT_PROPERTY)).toHaveLength(1)
	})

	it("one updatedAt per entity however many ops touched it", () => {
		const {ctx} = context()
		const r = translateOps(
			[
				{type: "createEntity", id: id(1), values: value(2)},
				{type: "updateEntity", id: id(1), set: value(3)},
				{type: "createRelation", id: id(9), from: id(1), to: id(4), relationType: id(5)},
			],
			ctx,
			{blockSeconds: SECONDS},
		)
		expect(times(r.intents, UPDATED_AT_PROPERTY)).toHaveLength(1)
		expect(times(r.intents, CREATED_AT_PROPERTY)).toHaveLength(1)
	})

	it("a relation touches its from-entity only, not to, type or the relation's own entity", () => {
		const from = conformingId("entity", hex(1))
		const {ctx} = context([from])
		const r = translateOps(
			[{type: "createRelation", id: id(9), from: id(1), to: id(4), relationType: id(5), entity: id(6)}],
			ctx,
			{blockSeconds: SECONDS},
		)
		const stamped = times(r.intents, UPDATED_AT_PROPERTY).map(entityOf)
		expect(stamped).toEqual([from])
		expect(times(r.intents, CREATED_AT_PROPERTY)).toHaveLength(0)
	})

	it("a relation from an entity new to the Space also creates it with a createdAt", () => {
		const {ctx} = context()
		const r = translateOps([{type: "createRelation", id: id(9), from: id(1), to: id(4), relationType: id(5)}], ctx, {blockSeconds: SECONDS})
		expect(times(r.intents, CREATED_AT_PROPERTY).map(entityOf)).toEqual([conformingId("entity", hex(1))])
	})

	it("a relation delete changes no time", () => {
		const {ctx, statements} = context()
		statements.add(conformingId("statement", hex(9)))
		const r = translateOps([{type: "deleteRelation", id: id(9)}], ctx, {blockSeconds: SECONDS})
		expect(times(r.intents, UPDATED_AT_PROPERTY)).toHaveLength(0)
	})

	it("an entity delete leaves the entity with no times, even after a change in the same edit", () => {
		const e = conformingId("entity", hex(1))
		const {ctx} = context([e])
		const r = translateOps(
			[
				{type: "updateEntity", id: id(1), set: value(2)},
				{type: "deleteEntity", id: id(1)},
			],
			ctx,
			{blockSeconds: SECONDS},
		)
		expect(times(r.intents, UPDATED_AT_PROPERTY)).toHaveLength(0)
	})

	it("an edit with no block time writes no times and counts the skip", () => {
		const {ctx} = context()
		const r = translateOps([{type: "createEntity", id: id(1), values: value(2)}], ctx, {blockSeconds: null})
		expect(times(r.intents, UPDATED_AT_PROPERTY)).toHaveLength(0)
		expect(r.stats["times-skipped-no-block-time"]).toBe(1)
	})

	it("is deterministic, so a replay at the stored progress sees the same intents", () => {
		const ops: GrcOp[] = [
			{type: "createEntity", id: id(1), values: value(2)},
			{type: "createEntity", id: id(3), values: value(2)},
		]
		const a = translateOps(ops, context().ctx, {blockSeconds: SECONDS}).intents
		const b = translateOps(ops, context().ctx, {blockSeconds: SECONDS}).intents
		expect(a).toEqual(b)
	})
})

describe("handleMessage carries the block time", () => {
	const varint = (n: number) => {
		const out: number[] = []
		let v = n
		while (v > 0x7f) {
			out.push((v & 0x7f) | 0x80)
			v >>>= 7
		}
		out.push(v)
		return out
	}
	const GAIA = "a19c345ab9866679b001d7d2138d88a1"
	const GEO = "daf0bc83ca744e1a9ee0377967846846"
	const edit = (seconds: number | null) =>
		Uint8Array.from([
			0x1a,
			1,
			1,
			0x32,
			16,
			...Buffer.from(GAIA, "hex"),
			...(seconds === null ? [] : [0x42, ...varint(1 + varint(seconds).length), 0x08, ...varint(seconds)]),
		])
	const decode: Decode = async () => ({ops: [{type: "createEntity", id: id(1), values: value(2)}]})

	async function run(seconds: number | null, entityTimes: boolean) {
		const store = new Store(mkdtempSync(join(tmpdir(), "feeder-times-")))
		const applied: string[] = []
		const spaces = new Set<string>()
		const deps = {
			db: {
				apply: async (zon: string) => {
					applied.push(zon)
					return {rc: 0, out: "", err: ""}
				},
				waitReady: async () => [...spaces],
				found: async () => {
					spaces.add(GEO)
					return GEO
				},
			},
			sealer: async () => "5".repeat(64),
			store,
			metrics: createMetrics(),
			beat: async () => {},
			sleep: async () => {},
			publish: () => {},
			decode,
			chunk: 200,
			signal: undefined,
			entityTimes,
		}
		await handleMessage(deps, 0, {offset: "1", value: edit(seconds)})
		return applied.join("\n")
	}

	it("flag on: the entity's createdAt and updatedAt are the edit's block time", async () => {
		const zon = await run(SECONDS, true)
		expect(zon).toContain(`.property = "${CREATED_AT_PROPERTY}", .value = .{ .instant = "${MICROS}" }`)
		expect(zon).toContain(`.property = "${UPDATED_AT_PROPERTY}", .value = .{ .instant = "${MICROS}" }`)
	})

	it("flag off: no time statements", async () => {
		const zon = await run(SECONDS, false)
		expect(zon).not.toContain(CREATED_AT_PROPERTY)
		expect(zon).not.toContain(UPDATED_AT_PROPERTY)
	})

	it("flag on but the edit has no block time: no time statements", async () => {
		const zon = await run(null, true)
		expect(zon).not.toContain(UPDATED_AT_PROPERTY)
	})
})
