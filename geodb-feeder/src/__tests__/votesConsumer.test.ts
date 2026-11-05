import {describe, expect, it} from "vitest"
import {createMetrics} from "../metrics.js"
import {votesBatchHandler, type VotesBatch} from "../votesConsumer.js"
import type {KeyedVotesDeps} from "../votesKeyed.js"

/** In-memory KeyedOffsets that records every markDone. */
const memOffsets = () => {
	const done = new Map<number, number>()
	const marks: [number, number][] = []
	return {
		marks,
		hadOffsets: false,
		lastDone: (p: number) => done.get(p) ?? null,
		markDone: (p: number, o: number) => {
			done.set(p, o)
			marks.push([p, o])
		},
	}
}

const harness = (handle: (offset: string) => Promise<"applied">) => {
	const offsets = memOffsets()
	const deps = {metrics: createMetrics(), offsets} as unknown as KeyedVotesDeps
	const abort = new AbortController()
	const events: string[] = []
	const committed: number[] = []
	const run = votesBatchHandler({
		deps,
		abort,
		commits: {done: async (_p, o) => void committed.push(o), flush: async () => {}},
		sleep: async (ms) => void events.push(`sleep ${ms}`),
		retryMs: 5,
		handle: async (_d, _p, m) => {
			events.push(`try ${m.offset}`)
			return handle(m.offset)
		},
	})
	const resolved: string[] = []
	const batch = (offsets: string[]): VotesBatch => ({
		batch: {partition: 0, messages: offsets.map((offset) => ({offset, value: Buffer.from([1]), timestamp: "0"}))},
		resolveOffset: (o) => void resolved.push(o),
		heartbeat: async () => {},
	})
	return {run, batch, offsets, abort, events, resolved, committed}
}

describe("votes consumer batch loop: stall, never skip", () => {
	it("retries a failing message in place and only marks it done once it succeeds, before touching the next", async () => {
		let failures = 2
		const h = harness(async (offset) => {
			if (offset === "10" && failures-- > 0) throw new Error("ResourceExhausted")
			return "applied"
		})
		await h.run(h.batch(["10", "11"]))
		expect(h.events).toEqual(["try 10", "sleep 5", "try 10", "sleep 5", "try 10", "try 11"])
		expect(h.offsets.marks).toEqual([
			[0, 10],
			[0, 11],
		])
		expect(h.resolved).toEqual(["10", "11"])
		expect(h.committed).toEqual([10, 11])
	})

	it("on shutdown mid-retry, returns without marking, resolving or committing the failing message", async () => {
		const h = harness(async () => {
			h.abort.abort()
			throw new Error("cap")
		})
		await h.run(h.batch(["10", "11"]))
		expect(h.events).toEqual(["try 10"])
		expect(h.offsets.marks).toEqual([])
		expect(h.resolved).toEqual([])
		expect(h.committed).toEqual([])
	})

	it("resolves an offset already marked done without applying it again", async () => {
		const h = harness(async () => "applied")
		h.offsets.markDone(0, 10)
		h.offsets.marks.length = 0
		await h.run(h.batch(["9", "10", "11"]))
		expect(h.events).toEqual(["try 11"])
		expect(h.resolved).toEqual(["9", "10", "11"])
		expect(h.offsets.marks).toEqual([[0, 11]])
	})
})
