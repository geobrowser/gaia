import {describe, expect, it} from "vitest"
import type {DerivedReader, DerivedRow} from "../../geodb/derived"
import {createFastPathFacts, FASTPATH_EXECUTIONS_COLLECTION, proposalFactKey} from "../fastPathFact"

const id = "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9"
const key = "0a1b2c3d4e5f60718293a4b5c6d7e8f9"

function value(version: number, seconds: bigint): Buffer {
	const b = Buffer.alloc(12)
	b.writeUInt32LE(version, 0)
	b.writeBigInt64LE(seconds, 4)
	return b
}

function reader(rows: Record<string, Buffer>, fail = false): DerivedReader & {reads: string[]} {
	const reads: string[] = []
	return {
		reads,
		async get(collection, keyHex): Promise<DerivedRow | null> {
			expect(collection).toBe(FASTPATH_EXECUTIONS_COLLECTION)
			reads.push(keyHex)
			if (fail) throw new Error("pipe down")
			const held = rows[keyHex]
			return held ? {group: "0".repeat(32), score: 0, value: held} : null
		},
		close() {},
	}
}

const fast = (executedAt: bigint | null, pid = id) => ({id: pid, votingMode: "Fast", executedAt})

describe("fast-path fact reader", () => {
	it("keys a proposal by its sixteen id bytes in hex", () => {
		expect(proposalFactKey(id.toUpperCase())).toBe(key)
		expect(proposalFactKey("not-a-uuid")).toBeNull()
	})

	it("fills a missing executedAt from the fact", async () => {
		const facts = createFastPathFacts(reader({[key]: value(2, 1_700_000_000n)}))
		const [p] = await facts.apply([fast(null)])
		expect(p?.executedAt).toBe(1_700_000_000n)
	})

	it("never overrides a stored executedAt and does not read for it", async () => {
		const r = reader({[key]: value(1, 5n)})
		const [p] = await createFastPathFacts(r).apply([fast(9n)])
		expect(p?.executedAt).toBe(9n)
		expect(r.reads).toEqual([])
	})

	it("skips Slow proposals and keeps null when there is no fact", async () => {
		const r = reader({})
		const out = await createFastPathFacts(r).apply([
			{id, votingMode: "Slow", executedAt: null},
			fast(null, "ffffffff-ffff-ffff-ffff-ffffffffffff"),
		])
		expect(out.map((p) => p.executedAt)).toEqual([null, null])
		expect(r.reads).toEqual(["ffffffffffffffffffffffffffffffff"])
	})

	it("fails open when the read throws", async () => {
		const warnings: string[] = []
		const facts = createFastPathFacts(reader({[key]: value(1, 5n)}, true), {warn: (m) => warnings.push(m)})
		const [p] = await facts.apply([fast(null)])
		expect(p?.executedAt).toBeNull()
		expect(warnings).toHaveLength(1)
	})

	it("ignores a value of the wrong length", async () => {
		const [p] = await createFastPathFacts(reader({[key]: Buffer.alloc(5)})).apply([fast(null)])
		expect(p?.executedAt).toBeNull()
	})
})
