import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {proposalVersionsBatchHandler} from "../proposalVersionsConsumer.js"
import {
	applyProposalVersion,
	backfillProposalVersions,
	deriveProposalName,
	handleProposalVersionMessage,
	type ProposalVersion,
	type ProposalVersionRow,
	parseProposalVersion,
	proposalVersionFromRow,
	proposalVersionIntents,
	proposalVersionRecordId,
	proposalVersionsKeyedEnabled,
	proposalVersionsOffsetsDir,
} from "../proposalVersionsKeyed.js"
import {idPart, intPart, recordEntityId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {SystemSpaceCapError} from "../systemSpaces.js"
import {deadLetters, fixture, ModelStore} from "./keyedHarness.js"

const KIND = RECORD_KINDS.proposalVersion.id
const COL = RECORD_COLUMNS.proposalVersion
const PROPOSAL = "7e1c345ab9866679b001d7d2138d88b1"
const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const PROPOSER = "b29c345ab9866679b001d7d2138d88a2"
const TARGET = "c39c345ab9866679b001d7d2138d88a3"

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

const addMember = len(10, len(1, hexb(TARGET)))
const flag = len(16, len(1, Buffer.from([0xab])))
const publish = (name?: string) => len(15, Buffer.concat([len(1, Buffer.from("ipfs://Qm1")), ...(name === undefined ? [] : [len(3, Buffer.from(name))])]))
const none = Buffer.alloc(0)

type Opts = {
	actions?: Buffer[]
	block?: bigint | null
	settings?: boolean
	proposal?: string | null
	mode?: bigint | null
	executeBy?: bigint
	createdAt?: bigint
}

const message = (o: Opts = {}): Buffer => {
	const parts: Buffer[] = [len(1, hexb(SPACE)), len(2, hexb(PROPOSER))]
	if (o.proposal !== null) parts.push(len(3, hexb(o.proposal ?? PROPOSAL)))
	if (o.mode !== null) parts.push(vi(4, o.mode ?? 1n))
	for (const a of o.actions ?? []) parts.push(len(5, a))
	if (o.settings !== false) {
		parts.push(len(6, Buffer.concat([vi(8, 500_000n), vi(9, 750_000n), vi(10, 5n), vi(11, 10n), vi(12, 1000n), vi(13, 2000n), vi(14, o.executeBy ?? 0n)])))
	}
	const block = o.block === undefined ? 123n : o.block
	if (block !== null) parts.push(len(10, Buffer.concat([vi(1, o.createdAt ?? 1_700_000_000n), vi(3, block)])))
	return Buffer.concat(parts)
}

const msg = (value: Uint8Array | null, offset = 0, eventType: string | null = "PROPOSAL_CREATED") => ({
	offset: String(offset),
	value,
	headers: eventType === null ? {} : {"event-type": Buffer.from(eventType)},
})

describe("parseProposalVersion (kg-indexer's map_proposal_message)", () => {
	it("reads the settings, mode, block and stamp", () => {
		expect(parseProposalVersion(message({mode: 0n, executeBy: 99n, actions: [addMember]}))).toEqual({
			proposal: PROPOSAL,
			block: 123n,
			votingMode: "Fast",
			startTime: 1000n,
			endTime: 2000n,
			quorum: 10n,
			partial: 500_000n,
			universal: 750_000n,
			flat: 5n,
			executeBy: 99n,
			name: "Add Member",
			createdAt: 1_700_000_000n,
		})
	})
	it("reads any mode but 0 as Slow, as kg-indexer maps it", () => {
		for (const mode of [1n, 7n, null]) expect(parseProposalVersion(message({mode})).votingMode).toBe(mode === null ? "Fast" : "Slow")
	})
	it("leaves execute_by 0 and an action-less message without those columns", () => {
		const v = parseProposalVersion(message({actions: []}))
		expect(v).not.toHaveProperty("executeBy")
		expect(v).not.toHaveProperty("name")
	})
	it("reads missing metadata as block 0 and keeps a u64 block as its bit pattern", () => {
		expect(parseProposalVersion(message({block: null})).block).toBe(0n)
		expect(parseProposalVersion(message({block: 0xffff_ffff_ffff_fff0n})).block).toBe(0xffff_ffff_ffff_fff0n)
	})
	it("refuses what kg-indexer errors on: a bad id, no settings; and a wrong wire type", () => {
		expect(() => parseProposalVersion(message({proposal: null}))).toThrow(/proposal_id/)
		expect(() => parseProposalVersion(message({settings: false}))).toThrow(/settings/)
		expect(() => parseProposalVersion(message({actions: [vi(10, 1n)]}))).toThrow(/wire type/)
	})
})

describe("deriveProposalName", () => {
	it("labels each action, a Publish by its name, and joins with a comma", () => {
		expect(deriveProposalName([addMember, flag, publish("My edit"), publish(), none])).toBe("Add Member, Flag, My edit, Publish, Unknown Action")
		expect(deriveProposalName([])).toBeUndefined()
	})
	it("truncates at 500 bytes to the last comma, then adds an ellipsis", () => {
		const long = Array.from({length: 60}, (_, i) => publish(`action-${String(i).padStart(3, "0")}`))
		const n = deriveProposalName(long) as string
		expect(Buffer.byteLength(n)).toBeLessThanOrEqual(500)
		expect(n.endsWith("...")).toBe(true)
		expect(n.slice(0, -3).endsWith(",")).toBe(false)
		expect(n.startsWith("action-000, action-001")).toBe(true)
	})
	it("never cuts inside a multi-byte character", () => {
		const n = deriveProposalName([publish("é".repeat(400))]) as string
		expect(n.endsWith("...")).toBe(true)
		expect(n).not.toContain("�")
		expect(Buffer.byteLength(n)).toBeLessThanOrEqual(500)
	})
})

describe("record key", () => {
	const v = parseProposalVersion(message())
	it("is recordEntityId(proposalVersion, [proposal, block]) and pinned", () => {
		expect(proposalVersionRecordId(v)).toBe(recordEntityId(KIND, [idPart(PROPOSAL), intPart(123)]))
		expect(proposalVersionRecordId(v)).toBe(PINNED)
		expect(RECORD_KINDS.proposalVersion.family).toBe("governance")
	})
	it("moves with each key part and with nothing else", () => {
		const id = proposalVersionRecordId(v)
		expect(proposalVersionRecordId({...v, quorum: 99n, name: "x", votingMode: "Fast"} as ProposalVersion)).toBe(id)
		const moved = [{...v, proposal: TARGET}, {...v, block: 124n}].map(proposalVersionRecordId)
		expect(new Set([id, ...moved]).size).toBe(3)
	})
})
const PINNED = "95702cf8f5fd4de9837c1a06d4e2ece8"

describe("proposalVersionIntents", () => {
	it("writes two key columns once and puts every other column the version has", () => {
		const intents = proposalVersionIntents(parseProposalVersion(message({mode: 0n, executeBy: 9n, actions: [flag]})))
		expect(intents[0]).toBe(`.{ .create_entity = "${proposalVersionRecordId(parseProposalVersion(message()))}" }`)
		expect(intents.filter((i) => i.includes(".add = ")).length).toBe(2)
		expect(intents.filter((i) => i.includes(".set_active = ")).length).toBe(2)
		expect(intents.filter((i) => i.includes(".put = ")).length).toBe(10)
		const all = intents.join("\n")
		expect(all).toContain(`.property = "${COL.proposal}", .value = .{ .reference = "${cid("entity", PROPOSAL)}" }`)
		expect(all).toContain(`.property = "${COL.block}", .value = .{ .number = 123 }`)
		expect(all).toContain(`.property = "${COL.votingMode}", .value = .{ .text = "Fast" }`)
		expect(all).toContain(`.property = "${COL.name}", .value = .{ .text = "Flag" }`)
		for (const [c, x] of [[COL.startTime, "1000"], [COL.endTime, "2000"], [COL.quorum, "10"], [COL.partial, "500000"], [COL.universal, "750000"], [COL.flat, "5"], [COL.executeBy, "9"], [COL.createdAt, "1700000000"]] as const) {
			expect(all).toContain(`.property = "${c}", .value = .{ .number = ${x} }`)
		}
	})
	it("omits execute_by and name when the version has none", () => {
		const all = proposalVersionIntents(parseProposalVersion(message({actions: []}))).join("\n")
		expect(all).not.toContain(COL.executeBy)
		expect(all).not.toContain(COL.name)
	})
	it("never writes the dropped columns: the ordinal, the threshold, the counts", () => {
		expect(Object.keys(COL).sort()).toEqual(["block", "createdAt", "endTime", "executeBy", "flat", "name", "partial", "proposal", "quorum", "startTime", "universal", "votingMode"])
	})
})

describe("rows and events agree", () => {
	const rowOf = (v: ProposalVersion): ProposalVersionRow => ({
		proposal_id: `${v.proposal.slice(0, 8)}-${v.proposal.slice(8, 12)}-${v.proposal.slice(12, 16)}-${v.proposal.slice(16, 20)}-${v.proposal.slice(20)}`.toUpperCase(),
		block: String(BigInt.asIntN(64, v.block)),
		voting_mode: v.votingMode,
		start_time: v.startTime.toString(),
		end_time: v.endTime.toString(),
		quorum: v.quorum.toString(),
		partial_percentage_support_threshold: v.partial.toString(),
		universal_percentage_support_threshold: v.universal.toString(),
		flat_support_threshold: v.flat.toString(),
		execute_by: v.executeBy?.toString() ?? null,
		name: v.name ?? null,
		version_created_at: v.createdAt.toString(),
	})
	it("a backfill row and the event that wrote it send the same intents", () => {
		for (const block of [123n, 0n, 0xffff_ffff_ffff_fff0n]) {
			for (const o of [{mode: 0n, executeBy: 5n, actions: [addMember, publish("x")]}, {mode: 1n, actions: []}]) {
				const v = parseProposalVersion(message({...o, block}))
				expect(proposalVersionIntents(proposalVersionFromRow(rowOf(v)))).toEqual(proposalVersionIntents(v))
			}
		}
	})
	it("refuses a row no event could have written", () => {
		const r = rowOf(parseProposalVersion(message()))
		expect(() => proposalVersionFromRow({...r, voting_mode: "Medium"})).toThrow(/voting_mode/)
		expect(() => proposalVersionFromRow({...r, quorum: "1.5"})).toThrow(/quorum/)
	})
})

describe("handleProposalVersionMessage", () => {
	it("applies CREATED and UPDATED, ignores everything else", async () => {
		const f = pfixture()
		const v = message({actions: [addMember]})
		expect(await handleProposalVersionMessage(f.deps, 0, msg(v, 0, "PROPOSAL_CREATED"))).toBe("applied")
		expect(await handleProposalVersionMessage(f.deps, 0, msg(v, 1, "PROPOSAL_UPDATED"))).toBe("applied")
		for (const t of ["PROPOSAL_VOTED", "PROPOSAL_EXECUTED", null]) expect(await handleProposalVersionMessage(f.deps, 0, msg(v, 2, t))).toBe("ignored")
		expect(await handleProposalVersionMessage(f.deps, 0, msg(null, 3))).toBe("ignored")
	})
	it("dead-letters what kg-indexer rejects, writing nothing", async () => {
		const f = pfixture()
		expect(await handleProposalVersionMessage(f.deps, 0, msg(message({settings: false}), 7))).toBe("dead_letter")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toHaveLength(1)
	})
	it("keeps versions apart: an UPDATE at another block adds a record and never changes the CREATE's; a replay is a no-op", async () => {
		const model = new ModelStore()
		const f = pfixture({model})
		const create = message({actions: [addMember], block: 10n})
		const update = message({actions: [flag], block: 20n})
		await handleProposalVersionMessage(f.deps, 0, msg(create, 0))
		await handleProposalVersionMessage(f.deps, 0, msg(update, 1, "PROPOSAL_UPDATED"))
		expect(model.entities.size).toBe(2)
		const before = model.snapshot()
		await handleProposalVersionMessage(f.deps, 0, msg(create, 0))
		await handleProposalVersionMessage(f.deps, 0, msg(update, 1, "PROPOSAL_UPDATED"))
		expect(model.snapshot()).toBe(before)
	})
	it("a founding cap error throws, and the backfill aborts on it", async () => {
		const f = pfixture({foundFailsOnce: "error: ResourceExhausted (space cap)"})
		await expect(handleProposalVersionMessage(f.deps, 0, msg(message(), 0))).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(deadLetters(f.store)).toEqual([])
		const g = pfixture({foundFails: "error: ResourceExhausted (space cap)"})
		const row: ProposalVersionRow = {
			proposal_id: PROPOSAL, block: "1", voting_mode: "Slow", start_time: "1", end_time: "2", quorum: "3",
			partial_percentage_support_threshold: "4", universal_percentage_support_threshold: "5", flat_support_threshold: "6",
			execute_by: null, name: null, version_created_at: "7",
		}
		await expect(backfillProposalVersions(g.deps, [row])).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(typeof applyProposalVersion).toBe("function")
	})
})

describe("consumer wiring", () => {
	it("is off by default, needs GEODB_RECORD_KEYS, and has its own offsets directory", () => {
		expect(proposalVersionsKeyedEnabled({recordKeys: true, proposalVersions: {enabled: false}})).toBe(false)
		expect(proposalVersionsKeyedEnabled({recordKeys: false, proposalVersions: {enabled: true}})).toBe(false)
		expect(proposalVersionsKeyedEnabled({recordKeys: true, proposalVersions: {enabled: true}})).toBe(true)
		expect(proposalVersionsOffsetsDir("/d")).toBe("/d/feeder/proposal-versions-keyed")
		expect(typeof proposalVersionsBatchHandler).toBe("function")
	})
})
