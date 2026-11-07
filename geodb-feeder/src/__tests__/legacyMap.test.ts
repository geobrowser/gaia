import {readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"
import {describe, expect, it} from "vitest"
import {conformingId} from "../ids.js"
import {LEGACY_ENTITY_KIND, LEGACY_ID_PROPERTY, legacyMapIntent, legacyStatementId} from "../legacyMap.js"
import {recordEntityId} from "../recordKey.js"
import {type GrcOp, type TranslateContext, translateOps} from "../translate.js"

const here = dirname(fileURLToPath(import.meta.url))
const vectors = JSON.parse(readFileSync(join(here, "vectors/legacy-map.json"), "utf8")) as {
	atoms: Record<string, string>
	cases: {source: "upstream" | "gaia"; original: string; keyed: string; statement: string}[]
}

const ctxOf = () => {
	const e = new Set<string>()
	const s = new Set<string>()
	const ctx: TranslateContext = {
		knownEntity: (x) => e.has(x),
		markEntity: (x) => void e.add(x),
		knownStmt: (x) => s.has(x),
		markStmt: (x) => void s.add(x),
	}
	return ctx
}
const NON_V4 = "0123456789abcdef0123456789abcdef"
const V4 = "3f2a1b0c9d8e4f70a1b2c3d4e5f60718"

describe("legacy map statement ids (byte for byte against geo-db's derivation)", () => {
	it("pins the atoms", () => {
		expect(LEGACY_ENTITY_KIND).toBe(vectors.atoms["sys.legacy.Entity"])
		expect(LEGACY_ID_PROPERTY).toBe(vectors.atoms["sys.legacy.Id"])
	})
	for (const c of vectors.cases) {
		it(`${c.source} ${c.original}`, () => {
			expect(legacyStatementId(c.keyed, c.original)).toBe(c.statement)
		})
	}
	it("the gaia cases' keyed ids are the feeder's own conformingId", () => {
		for (const c of vectors.cases.filter((x) => x.source === "gaia")) expect(conformingId("entity", c.original)).toBe(c.keyed)
	})
	it("the upstream cases' keyed ids are the record key of the kind over the original", () => {
		for (const c of vectors.cases.filter((x) => x.source === "upstream")) {
			expect(recordEntityId(LEGACY_ENTITY_KIND, [{id: c.original}])).toBe(c.keyed)
		}
	})
	it("takes a dashed original as its compact bytes", () => {
		expect(legacyStatementId("a7f79f68ecd64763867f42bfc4387c95", "01234567-89ab-cdef-0123-456789abcdef")).toBe("8d01169123404f6486a4d969ba49c828")
	})
	it("refuses an id that is not 16 bytes", () => {
		expect(() => legacyStatementId("a7f79f68ecd64763867f42bfc4387c95", "abc")).toThrow()
	})
})

describe("legacyMapIntent", () => {
	it("writes the Bytes value as 16 raw bytes (32 hex digits), never text of the id", () => {
		const keyed = conformingId("entity", NON_V4)
		const r = legacyMapIntent(keyed, NON_V4)
		expect(r).toEqual({
			intent: `.{ .add = .{ .id = "8d01169123404f6486a4d969ba49c828", .entity = "${keyed}", .property = "${LEGACY_ID_PROPERTY}", .value = .{ .bytes = "${NON_V4}" } } }`,
		})
	})
	it("writes nothing for a v4 id or a malformed one", () => {
		expect(legacyMapIntent(V4, V4)).toEqual({skip: "v4"})
		expect(legacyMapIntent(V4, "xyz")).toEqual({skip: "malformed"})
	})
})

describe("translateOps legacyMap flag", () => {
	const bytesOf = (h: string) => Uint8Array.from(Buffer.from(h, "hex"))
	const ops: GrcOp[] = [
		{type: "createEntity", id: bytesOf(NON_V4), values: []},
		{type: "createEntity", id: bytesOf(V4), values: []},
		{type: "updateEntity", id: bytesOf("00000000000000000000000000000001"), set: [], unset: []},
		{type: "createRelation", id: bytesOf(V4), from: bytesOf("ffffffffffffffffffffffffffffffff"), to: bytesOf(V4), relationType: bytesOf(V4)},
	]
	const mapLines = (t: {intents: string[]}) => t.intents.filter((i) => i.includes(LEGACY_ID_PROPERTY))

	it("is off by default: the intents are the same as without the option", () => {
		const off = translateOps(ops, ctxOf())
		expect(mapLines(off)).toEqual([])
		expect(off.stats["legacy-map-written"]).toBeUndefined()
		expect(translateOps(ops, ctxOf(), {legacyMap: false}).intents).toEqual(off.intents)
	})

	it("on, writes one map statement after each non-v4 entity's create_entity, and none for a v4 one", () => {
		const on = translateOps(ops, ctxOf(), {legacyMap: true})
		const maps = mapLines(on)
		expect(maps).toHaveLength(3) // createEntity, updateEntity (predates window), createRelation's from
		expect(on.stats["legacy-map-written"]).toBe(3)
		for (const m of maps) {
			const entity = /\.entity = "([0-9a-f]{32})"/.exec(m)?.[1]
			const at = on.intents.indexOf(m)
			expect(on.intents[at - 1]).toBe(`.{ .create_entity = "${entity}" }`)
		}
		expect(maps[0]).toContain(`.id = "8d01169123404f6486a4d969ba49c828"`)
		expect(maps[1]).toContain(`.id = "88710883e1af42c1ab486d8100468aaa"`)
		expect(maps[2]).toContain(`.id = "8350d213995b47e199ce5d07ef97941d"`)
		// the edit's other intents are unchanged
		expect(on.intents.filter((i) => !i.includes(LEGACY_ID_PROPERTY))).toEqual(translateOps(ops, ctxOf()).intents)
	})

	it("writes the statement once: an entity already known gets none, and a replay is idempotent", () => {
		const ctx = ctxOf()
		const first = translateOps(ops.slice(0, 1), ctx, {legacyMap: true})
		const again = translateOps(ops.slice(0, 1), ctx, {legacyMap: true})
		expect(mapLines(first)).toHaveLength(1)
		expect(mapLines(again)).toHaveLength(0)
		expect(mapLines(translateOps(ops.slice(0, 1), ctxOf(), {legacyMap: true}))).toEqual(mapLines(first))
	})

	it("counts and skips an original that is not 16 bytes of hex", () => {
		const t = translateOps([{type: "createEntity", id: "not-an-id", values: []}], ctxOf(), {legacyMap: true})
		expect(mapLines(t)).toEqual([])
		expect(t.stats["legacy-map-skipped-malformed"]).toBe(1)
	})
})
