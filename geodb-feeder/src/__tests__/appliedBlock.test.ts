import {describe, expect, it} from "vitest"
import {APPLIED_BLOCK_KEY, AppliedBlockTracker, createAppliedBlockWriter} from "../appliedBlock.js"
import type {RunResult} from "../geodb.js"

const hexOf = (s: string) => Buffer.from(s).toString("hex")

describe("applied block watermark", () => {
	it("is empty until an edit with a block is applied", () => {
		const t = new AppliedBlockTracker()
		expect(t.watermark()).toBeNull()
		t.observe(0, null)
		expect(t.watermark()).toBeNull()
		expect(t.value()).toBeNull()
	})

	it("follows one partition and moves back on a replay", () => {
		const t = new AppliedBlockTracker()
		t.observe(0, 100)
		t.observe(0, 105)
		expect(t.watermark()).toBe(105)
		t.observe(0, 90) // a replay from earlier offsets: the row says what the store holds
		expect(t.watermark()).toBe(90)
	})

	it("is the lowest partition's block, not the highest", () => {
		const t = new AppliedBlockTracker()
		t.observe(0, 500)
		t.observe(1, 300)
		expect(t.watermark()).toBe(300)
		t.observe(1, 600)
		expect(t.watermark()).toBe(500)
	})

	it("ignores an edit with no block and keeps the last value", () => {
		const t = new AppliedBlockTracker()
		t.observe(0, 7)
		t.observe(0, null)
		expect(t.watermark()).toBe(7)
	})

	it("renders `<block>:<per-partition json>`, the shape every cursors row has", () => {
		const t = new AppliedBlockTracker()
		t.observe(1, 300)
		t.observe(0, 500)
		expect(t.value()).toBe('300:{"0":500,"1":300}')
	})
})

describe("applied block writer", () => {
	const setup = (opts: {fail?: boolean} = {}) => {
		let now = 1_000_000
		const calls: string[][] = []
		const results: boolean[] = []
		const w = createAppliedBlockWriter({
			run: async (args): Promise<RunResult> => {
				calls.push(args)
				return opts.fail ? {rc: 1, out: "", err: "SessionExpired"} : {rc: 0, out: "applied: 1\n", err: ""}
			},
			minIntervalMs: 5000,
			now: () => now,
			onResult: (ok) => results.push(ok),
		})
		return {w, calls, results, advance: (ms: number) => (now += ms)}
	}

	it("puts one cursors row keyed by the writer id, after an edit", async () => {
		const {w, calls} = setup()
		await w.note(0, 42)
		expect(calls).toEqual([["derived", "put", "10", "0", "0", APPLIED_BLOCK_KEY, `hex:${hexOf('42:{"0":42}')}`]])
	})

	it("is throttled per process, and flush sends what the throttle held back", async () => {
		const {w, calls, advance} = setup()
		await w.note(0, 1)
		await w.note(0, 2)
		await w.note(0, 3)
		expect(calls).toHaveLength(1)
		await w.flush()
		expect(calls).toHaveLength(2)
		expect(calls[1]?.[6]).toBe(`hex:${hexOf('3:{"0":3}')}`)
		await w.flush() // nothing new
		expect(calls).toHaveLength(2)
		advance(6000)
		await w.note(0, 4)
		expect(calls).toHaveLength(3)
	})

	it("never throws on a failed put, counts it, and retries the same value later", async () => {
		const {w, calls, results, advance} = setup({fail: true})
		await expect(w.note(0, 9)).resolves.toBeUndefined()
		expect(results).toEqual([false])
		advance(6000)
		await w.flush()
		expect(calls).toHaveLength(2)
		expect(calls[1]?.[6]).toBe(`hex:${hexOf('9:{"0":9}')}`)
	})

	it("sends nothing for an edit with no block", async () => {
		const {w, calls} = setup()
		await w.note(0, null)
		await w.flush()
		expect(calls).toHaveLength(0)
	})
})
