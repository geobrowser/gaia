import {describe, expect, it} from "vitest"
import {
	parseProposalEscalation,
	proposalEscalationIntents,
	proposalEscalationRecordId,
	resolveEscalations,
} from "../proposalEscalationKeyed.js"
import {handleProposalVersionMessage, proposalVersionsOffsetsDir} from "../proposalVersionsKeyed.js"
import {deadLetters, fixture, ModelStore} from "./keyedHarness.js"

const PROPOSAL = "7e1c345ab9866679b001d7d2138d88b1"
const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const pfixture = (o: Parameters<typeof fixture>[0] = {}) => fixture({...o, offsetsDir: proposalVersionsOffsetsDir})

const varint = (n: bigint) => {
	const out: number[] = []
	let v = BigInt.asUintN(64, n)
	do {
		let b = Number(v & 0x7fn)
		v >>= 7n
		if (v) b |= 0x80
		out.push(b)
	} while (v)
	return Buffer.from(out)
}
const len = (n: number, b: Buffer) => Buffer.concat([varint(BigInt((n << 3) | 2)), varint(BigInt(b.length)), b])
const vi = (n: number, v: bigint) => Buffer.concat([varint(BigInt(n << 3)), varint(v)])
const hexb = (h: string) => Buffer.from(h, "hex")

type Opts = {mode?: bigint; executeBy?: bigint; block?: bigint | null; settings?: boolean; proposal?: string}
const message = (o: Opts = {}): Buffer => {
	const parts = [len(1, hexb(SPACE)), len(2, hexb(o.proposal ?? PROPOSAL))]
	if (o.settings !== false)
		parts.push(len(3, Buffer.concat([vi(7, o.mode ?? 1n), vi(8, 500_000n), vi(9, 750_000n), vi(10, 5n), vi(11, 10n), vi(12, 1000n), vi(13, 2000n), vi(14, o.executeBy ?? 0n)])))
	const block = o.block === undefined ? 200n : o.block
	if (block !== null) parts.push(len(4, Buffer.concat([vi(1, 1_700_000_000n), vi(3, block)])))
	return Buffer.concat(parts)
}
const msg = (value: Uint8Array | null, offset = 0) => ({offset: String(offset), value, headers: {"event-type": Buffer.from("PROPOSAL_SETTINGS_UPDATED")}})

describe("parseProposalEscalation", () => {
	it("reads the settings and the event's block; execute_by 0 is none; any non-Fast mode is Slow", () => {
		const e = parseProposalEscalation(message({mode: 0n, executeBy: 0n}))
		expect(e).toEqual({proposal: PROPOSAL, block: 200n, votingMode: "Fast", startTime: 1000n, endTime: 2000n, quorum: 10n, partial: 500_000n, universal: 750_000n, flat: 5n})
		expect(parseProposalEscalation(message({mode: 7n, executeBy: 9n}))).toMatchObject({votingMode: "Slow", executeBy: 9n})
	})
	it("throws where kg-indexer errors", () => {
		expect(() => parseProposalEscalation(message({settings: false}))).toThrow(/settings/)
		expect(() => parseProposalEscalation(message({proposal: "abcd"}))).toThrow()
	})
	it("intents depend on the event alone and the record id on (proposal, block)", () => {
		const a = parseProposalEscalation(message())
		expect(proposalEscalationIntents(a)).toEqual(proposalEscalationIntents(parseProposalEscalation(message())))
		expect(proposalEscalationRecordId(a)).not.toBe(proposalEscalationRecordId({...a, block: 201n}))
		expect(proposalEscalationIntents({...a, executeBy: 3n}).length).toBe(proposalEscalationIntents(a).length + 1)
	})
})

describe("handleProposalVersionMessage, PROPOSAL_SETTINGS_UPDATED", () => {
	it("applies, and a replay is a no-op", async () => {
		const model = new ModelStore()
		const f = pfixture({model})
		expect(await handleProposalVersionMessage(f.deps, 0, msg(message()))).toBe("applied")
		expect(model.entities.size).toBe(1)
		const before = model.snapshot()
		await handleProposalVersionMessage(f.deps, 0, msg(message()))
		expect(model.snapshot()).toBe(before)
	})
	it("a second escalation at another block is a second record", async () => {
		const model = new ModelStore()
		const f = pfixture({model})
		await handleProposalVersionMessage(f.deps, 0, msg(message({block: 200n})))
		await handleProposalVersionMessage(f.deps, 0, msg(message({block: 300n, mode: 0n})))
		expect(model.entities.size).toBe(2)
	})
	it("dead-letters a message kg-indexer rejects, writing nothing", async () => {
		const f = pfixture()
		expect(await handleProposalVersionMessage(f.deps, 0, msg(message({settings: false})))).toBe("dead_letter")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toHaveLength(1)
	})
})

describe("resolveEscalations", () => {
	const v = (block: bigint) => ({block})
	const e = (block: bigint, id: string) => ({block, id})
	it("hits the version with the greatest block at or before the escalation", () => {
		const [v1, v2] = [v(10n), v(20n)]
		const e1 = e(15n, "a")
		const r = resolveEscalations([v2, v1], [e1])
		expect(r.applied.get(v1)).toBe(e1)
		expect(r.applied.has(v2)).toBe(false)
		expect(r.orphans).toEqual([])
	})
	it("an escalation in the version's own block hits it; one before every version is an orphan", () => {
		const v1 = v(10n)
		const same = e(10n, "a")
		const early = e(9n, "b")
		const r = resolveEscalations([v1], [same, early])
		expect(r.applied.get(v1)).toBe(same)
		expect(r.orphans).toEqual([early])
	})
	it("the latest escalation wins, a tie in one block by record id, whatever the input order", () => {
		const v1 = v(10n)
		const [a, b, c] = [e(12n, "a"), e(12n, "b"), e(11n, "z")]
		expect(resolveEscalations([v1], [c, b, a]).applied.get(v1)).toBe(b)
		expect(resolveEscalations([v1], [a, c, b]).applied.get(v1)).toBe(b)
	})
	it("compares blocks as unsigned", () => {
		const big = v(0xffff_ffff_ffff_fff0n)
		const r = resolveEscalations([v(5n), big], [e(0xffff_ffff_ffff_fff1n, "a")])
		expect(r.applied.has(big)).toBe(true)
	})
})
