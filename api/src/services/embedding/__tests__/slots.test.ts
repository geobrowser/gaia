import {describe, expect, it, vi} from "vitest"
import {SearchError, SearchErrorType} from "../../search/types"
import {canonicalJson, SlotRegistry} from "../slots"

const descriptor = (dims: number, extra: Record<string, unknown> = {}) => ({
	provider: "onnx-local",
	model_id: "BAAI/bge-small-en-v1.5",
	dimensions: dims,
	pooling: "cls",
	score_floor: 0.85,
	document_prompt: "",
	query_prompt: "",
	...extra,
})

function registry(opts: {
	meta?: Record<string, unknown>
	slots?: Record<string, {descriptor: Record<string, unknown>; descriptor_hash: string; vector_field: string}>
	infoFails?: boolean
}) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const service = {
		info: vi.fn().mockImplementation(async () => {
			if (opts.infoFails) throw new Error("connect ECONNREFUSED")
			return {slots: opts.slots ?? {}}
		}),
	}
	return {reg: new SlotRegistry({readMeta: async () => opts.meta, service, log}), log, service}
}

describe("canonicalJson", () => {
	it("sorts keys at every level and strips whitespace", () => {
		expect(canonicalJson({b: {z: 1, a: [{y: 2, x: 1}]}, a: true})).toBe(
			'{"a":true,"b":{"a":[{"x":1,"y":2}],"z":1}}',
		)
		expect(canonicalJson({k: 0.85})).toBe('{"k":0.85}')
	})
})

describe("SlotRegistry", () => {
	const a = descriptor(384)
	const aHash = "a".repeat(64)

	it("marks a slot ready only when index and service agree on the descriptor", async () => {
		const {reg} = registry({
			meta: {embedding_slots: {s1: a}, embedding_default_slot: "s1"},
			slots: {s1: {descriptor: {...a}, descriptor_hash: aHash, vector_field: "emb_s1"}},
		})
		await reg.refresh()
		const slot = reg.resolve()
		expect(slot.id).toBe("s1")
		expect(slot.vectorField).toBe("emb_s1")
		expect(slot.descriptorHash).toBe(aHash)
		expect(slot.scoreFloor).toBe(0.85)
		expect(slot.dimensions).toBe(384)
		// key order on either side does not matter
		expect(reg.status().slots).toEqual([{id: "s1", state: "ready"}])
	})

	it("reports slots the service does not load or loads differently, and never serves them", async () => {
		const {reg} = registry({
			meta: {embedding_slots: {s1: a, s2: descriptor(768)}, embedding_default_slot: "s2"},
			slots: {
				s1: {
					descriptor: {...a, query_prompt: "query: "},
					descriptor_hash: "x".repeat(64),
					vector_field: "emb_s1",
				},
			},
		})
		await reg.refresh()
		expect(reg.readySlots()).toEqual([])
		expect(reg.status().slots.map((s) => [s.id, s.state])).toEqual([
			["s1", "descriptor_mismatch"],
			["s2", "not_loaded"],
		])
		expect(() => reg.resolve()).toThrowError(SearchError)
		try {
			reg.resolve("s2")
		} catch (e) {
			expect((e as SearchError).type).toBe(SearchErrorType.Unavailable)
		}
	})

	it("distinguishes an unknown slot (client error) from an unavailable one", async () => {
		const {reg} = registry({
			meta: {embedding_slots: {s1: a}, embedding_default_slot: "s1"},
			slots: {s1: {descriptor: {...a}, descriptor_hash: aHash, vector_field: "emb_s1"}},
		})
		await reg.refresh()
		try {
			reg.resolve("nope000000")
			throw new Error("should have thrown")
		} catch (e) {
			expect((e as SearchError).type).toBe(SearchErrorType.ValidationError)
		}
	})

	it("keeps the previous registry when a refresh fails, and says so", async () => {
		const {reg, log} = registry({
			meta: {embedding_slots: {s1: a}, embedding_default_slot: "s1"},
			slots: {s1: {descriptor: {...a}, descriptor_hash: aHash, vector_field: "emb_s1"}},
		})
		await reg.refresh()
		expect(reg.resolve().id).toBe("s1")
		// now the service goes away
		const failing = registry({meta: {embedding_slots: {s1: a}, embedding_default_slot: "s1"}, infoFails: true})
		await failing.reg.refresh()
		expect(failing.reg.lastError).toContain("ECONNREFUSED")
		expect(failing.log.warn).toHaveBeenCalled()
		try {
			failing.reg.resolve()
		} catch (e) {
			expect((e as SearchError).type).toBe(SearchErrorType.Unavailable)
			expect((e as SearchError).message).toContain("last refresh failed")
		}
		expect(log.warn).not.toHaveBeenCalled()
	})

	it("with no default set, an explicit slot works and the implicit one is unavailable", async () => {
		const {reg} = registry({
			meta: {embedding_slots: {s1: a}},
			slots: {s1: {descriptor: {...a}, descriptor_hash: aHash, vector_field: "emb_s1"}},
		})
		await reg.refresh()
		expect(reg.resolve("s1").id).toBe("s1")
		expect(() => reg.resolve()).toThrowError(/no default/)
	})
})
