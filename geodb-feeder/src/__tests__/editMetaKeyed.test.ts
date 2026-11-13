import {describe, expect, it} from "vitest"
import {feederGroupIds, loadConfig} from "../config.js"
import {
	cutEditName,
	editMetaIntents,
	editMetaKeyedEnabled,
	editMetaOffsetsDir,
	editMetaRecordId,
	handleEditMetaMessage,
} from "../editMetaKeyed.js"
import {parseHermesEdit} from "../hermes.js"
import {RECORD_COLUMNS} from "../recordKinds.js"
import {SystemSpaceCapError, systemSpaceNonce} from "../systemSpaces.js"
import {deadLetters, fixture, ModelStore} from "./keyedHarness.js"

const COL = RECORD_COLUMNS.editMeta
const EDIT = "b29c345ab9866679b001d7d2138d88a2"
const EDIT2 = "c39c345ab9866679b001d7d2138d88a3"
const efixture = (o: Parameters<typeof fixture>[0] = {}) => fixture({...o, offsetsDir: editMetaOffsetsDir})

const varint = (n: number) => {
	const out: number[] = []
	let v = n
	do {
		let b = v & 0x7f
		v = Math.floor(v / 128)
		if (v) b |= 0x80
		out.push(b)
	} while (v)
	return Buffer.from(out)
}
const bytes = (n: number, b: Buffer) => Buffer.concat([varint((n << 3) | 2), varint(b.length), b])
const num = (n: number, v: number) => Buffer.concat([varint(n << 3), varint(v)])

/** HermesEdit bytes (knowledge.proto): id (1), name (2), meta (8: created_at 1, block_number 3, sequence 5). */
function hermesEdit(o: {id?: string | null; name?: string | null; meta?: {block?: number; sequence?: number} | null} = {}): Uint8Array {
	const parts: Buffer[] = []
	const id = o.id === undefined ? EDIT : o.id
	if (id !== null) parts.push(bytes(1, Buffer.from(id, "hex")))
	if (o.name) parts.push(bytes(2, Buffer.from(o.name, "utf8")))
	parts.push(bytes(3, Buffer.from([1, 2, 3])))
	const meta = o.meta === undefined ? {block: 123, sequence: 4} : o.meta
	if (meta !== null) {
		const m = [num(1, 1_700_000_000)]
		if (meta.block) m.push(num(3, meta.block))
		if (meta.sequence) m.push(num(5, meta.sequence))
		parts.push(bytes(8, Buffer.concat(m)))
	}
	return Buffer.concat(parts)
}
const msg = (value: Uint8Array | null, offset = 0) => ({offset: String(offset), value})

describe("parseHermesEdit: sequence and meta", () => {
	it("reads meta.sequence and notes whether meta was there", () => {
		expect(parseHermesEdit(hermesEdit())).toMatchObject({blockNumber: 123, sequence: 4, hasMeta: true})
		expect(parseHermesEdit(hermesEdit({meta: null}))).toMatchObject({blockNumber: null, sequence: null, hasMeta: false})
	})
})

describe("cutEditName", () => {
	it("empty is no name, short is kept, long is cut to 256 bytes on a char boundary", () => {
		expect(cutEditName(undefined)).toBeNull()
		expect(cutEditName("")).toBeNull()
		expect(cutEditName("hello")).toBe("hello")
		expect(cutEditName("a".repeat(300))).toBe("a".repeat(256))
		const cut = cutEditName("é".repeat(200)) as string
		expect(Buffer.byteLength(cut)).toBe(256)
		expect(cut).toBe("é".repeat(128))
		const odd = cutEditName(`a${"é".repeat(200)}`) as string
		expect(Buffer.byteLength(odd)).toBe(255)
	})
})

describe("edit meta record", () => {
	it("is keyed by the edit id alone", () => {
		expect(editMetaRecordId(EDIT)).toMatch(/^[0-9a-f]{32}$/)
		expect(editMetaRecordId(EDIT)).not.toBe(editMetaRecordId(EDIT2))
		expect(editMetaRecordId(EDIT)).toBe(editMetaRecordId(EDIT))
	})

	it("intents are a pure function of the edit, name optional", () => {
		const a = editMetaIntents({editId: EDIT, block: 5, sequence: 2, name: "n"})
		expect(editMetaIntents({editId: EDIT, block: 5, sequence: 2, name: "n"})).toEqual(a)
		expect(a[0]).toBe(`.{ .create_entity = "${editMetaRecordId(EDIT)}" }`)
		expect(a.filter((i) => i.includes(".put ="))).toHaveLength(3)
		expect(editMetaIntents({editId: EDIT, block: 5, sequence: 2, name: null}).filter((i) => i.includes(".put ="))).toHaveLength(2)
	})
})

describe("handleEditMetaMessage", () => {
	it("founds the governance Space once and writes block, sequence and name", async () => {
		const model = new ModelStore()
		const f = efixture({model})
		expect(await handleEditMetaMessage(f.deps, 0, msg(hermesEdit({name: "Fix typo"}), 0))).toBe("applied")
		expect(await handleEditMetaMessage(f.deps, 0, msg(hermesEdit({id: EDIT2, meta: {block: 9, sequence: 1}}), 1))).toBe("applied")
		expect(f.founds).toEqual([systemSpaceNonce("governance")])
		const r = editMetaRecordId(EDIT)
		expect(model.cell(r, COL.block)).toEqual([".{ .number = 123 }"])
		expect(model.cell(r, COL.sequence)).toEqual([".{ .number = 4 }"])
		expect(model.cell(r, COL.name)).toEqual(['.{ .text = "Fix typo" }'])
		expect(model.cell(r, COL.editId)).toEqual([`.{ .text = "${EDIT}" }`])
		expect(model.cell(editMetaRecordId(EDIT2), COL.name)).toEqual([])
	})

	it("a replay or a redelivery converges to the same state", async () => {
		const once = new ModelStore()
		const f1 = efixture({model: once})
		await handleEditMetaMessage(f1.deps, 0, msg(hermesEdit({name: "x"}), 0))
		const twice = new ModelStore()
		const f2 = efixture({model: twice})
		for (const o of [0, 0, 0]) await handleEditMetaMessage(f2.deps, 0, msg(hermesEdit({name: "x"}), o))
		expect(twice.snapshot()).toBe(once.snapshot())
	})

	it("ignores an empty value and an edit with no meta, as kg-indexer writes no edit_versions row", async () => {
		const f = efixture()
		expect(await handleEditMetaMessage(f.deps, 0, msg(null))).toBe("ignored")
		expect(await handleEditMetaMessage(f.deps, 0, msg(hermesEdit({meta: null})))).toBe("ignored")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toEqual([])
	})

	it("dead-letters an edit with no id or a truncated envelope, writing nothing", async () => {
		const f = efixture()
		expect(await handleEditMetaMessage(f.deps, 0, msg(hermesEdit({id: null})))).toBe("dead_letter")
		expect(await handleEditMetaMessage(f.deps, 0, msg(Buffer.from([0x0a, 0x40, 1])))).toBe("dead_letter")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toHaveLength(2)
	})

	it("a ResourceExhausted founding throws (the consumer retries in place) and dead-letters nothing", async () => {
		const f = efixture({foundFailsOnce: "error: ResourceExhausted (space cap)"})
		await expect(handleEditMetaMessage(f.deps, 0, msg(hermesEdit()))).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(deadLetters(f.store)).toEqual([])
	})
})

describe("GEODB_EDIT_META", () => {
	const env = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b", ENVIRONMENT: "t"}
	it("is off by default, needs GEODB_RECORD_KEYS, and has its own group", () => {
		expect(editMetaKeyedEnabled(loadConfig({...env, GEODB_RECORD_KEYS: "1"}))).toBe(false)
		expect(editMetaKeyedEnabled(loadConfig({...env, GEODB_EDIT_META: "1"}))).toBe(false)
		const c = loadConfig({...env, GEODB_EDIT_META: "1", GEODB_RECORD_KEYS: "1"})
		expect(editMetaKeyedEnabled(c)).toBe(true)
		expect(c.editMeta.topic).toBe("t.knowledge.edits")
		expect(c.editMeta.groupId).toBe("geodb-feeder-edit-meta-keyed-v1-t")
		expect(feederGroupIds(c)).toContain(c.editMeta.groupId)
		expect(editMetaOffsetsDir("/d")).toBe("/d/feeder/edit-meta-keyed")
	})
})
