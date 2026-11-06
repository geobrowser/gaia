import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {proposalActionsBatchHandler} from "../proposalActionsConsumer.js"
import {
	backfillProposalActions,
	handleProposalActionMessage,
	type ProposalAction,
	type ProposalActionRow,
	parseProposalActions,
	proposalActionFromRow,
	proposalActionIntents,
	proposalActionRecordId,
	proposalActionsKeyedEnabled,
	proposalActionsOffsetsDir,
} from "../proposalActionsKeyed.js"
import {idPart, intPart, recordEntityId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {SystemSpaceCapError} from "../systemSpaces.js"
import {deadLetters, fixture, ModelStore} from "./keyedHarness.js"

const KIND = RECORD_KINDS.proposalAction.id
const COL = RECORD_COLUMNS.proposalAction
const PROPOSAL = "7e1c345ab9866679b001d7d2138d88b1"
const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const PROPOSER = "b29c345ab9866679b001d7d2138d88a2"
const TARGET = "c39c345ab9866679b001d7d2138d88a3"

const pfixture = (o: Parameters<typeof fixture>[0] = {}) => fixture({...o, offsetsDir: proposalActionsOffsetsDir})

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

/** One ProposalAction message with the oneof member `member` holding `body`. */
const action = (member: number, body: Buffer) => len(member, body)

const message = (o: {actions?: Buffer[]; block?: bigint | null; settings?: boolean; proposal?: string | null} = {}): Buffer => {
	const parts: Buffer[] = [len(1, hexb(SPACE)), len(2, hexb(PROPOSER))]
	if (o.proposal !== null) parts.push(len(3, hexb(o.proposal ?? PROPOSAL)))
	for (const a of o.actions ?? []) parts.push(len(5, a))
	if (o.settings !== false) parts.push(len(6, vi(7, 1n)))
	const block = o.block === undefined ? 123n : o.block
	if (block !== null) parts.push(len(10, Buffer.concat([vi(1, 1_700_000_000n), vi(3, block)])))
	return Buffer.concat(parts)
}

const addMember = action(10, len(1, hexb(TARGET)))
const publish = action(15, Buffer.concat([len(1, Buffer.from("ipfs://Qm1")), len(2, Buffer.from([1, 2, 255]))]))
const flag = action(16, len(1, Buffer.from([0xab, 0xcd])))
const voting = action(18, Buffer.concat([vi(5, 500_000n), vi(6, 750_000n), vi(7, 5n), vi(8, 10n), vi(9, 86_400n), vi(10, 1n), vi(11, 3600n)]))
const badTarget = action(10, len(1, Buffer.from([1, 2, 3])))
const none = Buffer.alloc(0)

const msg = (value: Uint8Array | null, offset = 0, eventType: string | null = "PROPOSAL_CREATED") => ({
	offset: String(offset),
	value,
	headers: eventType === null ? {} : {"event-type": Buffer.from(eventType)},
})

describe("parseProposalActions (kg-indexer's map_proposal_action)", () => {
	it("maps each action kind to the columns Postgres holds", () => {
		const a = parseProposalActions(message({actions: [addMember, publish, flag, voting, badTarget, none]}))
		expect(a.map((x) => [x.index, x.actionType])).toEqual([
			[0, "AddMember"],
			[1, "Publish"],
			[2, "Flag"],
			[3, "UpdateVotingSettings"],
			[4, "Unknown"],
			[5, "Unknown"],
		])
		expect(a[0]).toMatchObject({proposal: PROPOSAL, block: 123n, target: TARGET})
		expect(a[1]).toMatchObject({contentUri: "ipfs://Qm1", metadata: "0102ff"})
		expect(a[2]).toMatchObject({contentId: "abcd"})
		expect(a[3]).toMatchObject({partial: 500_000n, universal: 750_000n, flat: 5n, quorum: 10n, duration: 86_400n, grace: 3600n, disableFastPath: true})
		expect(a[4]).toEqual({actionType: "Unknown", proposal: PROPOSAL, block: 123n, index: 4})
	})

	it("omits an empty Publish metadata, as storage.rs does", () => {
		const [p] = parseProposalActions(message({actions: [action(15, len(1, Buffer.from("x")))]}))
		expect(p).not.toHaveProperty("metadata")
	})

	it("reads missing metadata as block 0 and keeps a u64 above i64::MAX as its i64 bit pattern", () => {
		expect(parseProposalActions(message({actions: [addMember], block: null}))[0]?.block).toBe(0n)
		const [v] = parseProposalActions(message({actions: [action(18, vi(5, 0xffff_ffff_ffff_ffffn))]}))
		expect(v?.partial).toBe(-1n)
	})

	it("refuses what kg-indexer errors on: a bad id, no settings; and a wrong wire type", () => {
		expect(() => parseProposalActions(message({proposal: null}))).toThrow(/proposal_id/)
		expect(() => parseProposalActions(message({settings: false}))).toThrow(/settings/)
		expect(() => parseProposalActions(message({actions: [vi(10, 1n)]}))).toThrow(/wire type/)
	})
})

describe("record key", () => {
	const a: ProposalAction = {proposal: PROPOSAL, block: 123n, index: 2, actionType: "AddMember"}
	it("is recordEntityId(proposalAction, [proposal, block, index]) and pinned", () => {
		expect(proposalActionRecordId(a)).toBe(recordEntityId(KIND, [idPart(PROPOSAL), intPart(123), intPart(2)]))
		expect(proposalActionRecordId(a)).toBe(PINNED)
		expect(RECORD_KINDS.proposalAction.family).toBe("governance")
	})
	it("moves with each key part and with nothing else", () => {
		const id = proposalActionRecordId(a)
		expect(proposalActionRecordId({...a, actionType: "Flag", contentId: "ab"} as ProposalAction)).toBe(id)
		const moved = [{...a, proposal: TARGET}, {...a, block: 124n}, {...a, index: 3}].map(proposalActionRecordId)
		expect(new Set([id, ...moved]).size).toBe(4)
	})
})
const PINNED = proposalActionRecordId({proposal: PROPOSAL, block: 123n, index: 2})

describe("proposalActionIntents", () => {
	it("writes three key columns once and puts only the columns the action has", () => {
		const [a] = parseProposalActions(message({actions: [addMember]}))
		const intents = proposalActionIntents(a as ProposalAction)
		expect(intents[0]).toBe(`.{ .create_entity = "${proposalActionRecordId(a as ProposalAction)}" }`)
		expect(intents.filter((i) => i.includes(".add = ")).length).toBe(3)
		expect(intents.filter((i) => i.includes(".set_active = ")).length).toBe(3)
		expect(intents.filter((i) => i.includes(".put = ")).length).toBe(2)
		const all = intents.join("\n")
		expect(all).toContain(`.property = "${COL.actionType}", .value = .{ .text = "AddMember" }`)
		expect(all).toContain(`.property = "${COL.target}", .value = .{ .reference = "${cid("entity", TARGET)}" }`)
	})
	it("writes every voting-settings column", () => {
		const [a] = parseProposalActions(message({actions: [voting]}))
		const all = proposalActionIntents(a as ProposalAction).join("\n")
		for (const [c, v] of [[COL.partial, "500000"], [COL.universal, "750000"], [COL.flat, "5"], [COL.quorum, "10"], [COL.duration, "86400"], [COL.grace, "3600"]] as const) {
			expect(all).toContain(`.property = "${c}", .value = .{ .number = ${v} }`)
		}
		expect(all).toContain(`.property = "${COL.disableFastPath}", .value = .{ .boolean = true }`)
	})
})

describe("rows and events agree", () => {
	const rowOf = (a: ProposalAction): ProposalActionRow => ({
		proposal_id: `${a.proposal.slice(0, 8)}-${a.proposal.slice(8, 12)}-${a.proposal.slice(12, 16)}-${a.proposal.slice(16, 20)}-${a.proposal.slice(20)}`.toUpperCase(),
		index: a.index,
		block: String(BigInt.asIntN(64, a.block)),
		action_type: a.actionType,
		target_id: a.target ?? null,
		content_uri: a.contentUri ?? null,
		metadata: a.metadata ?? null,
		content_id: a.contentId ?? null,
		quorum: a.quorum?.toString() ?? null,
		duration: a.duration?.toString() ?? null,
		partial_percentage_support_threshold: a.partial?.toString() ?? null,
		universal_percentage_support_threshold: a.universal?.toString() ?? null,
		flat_support_threshold: a.flat?.toString() ?? null,
		disable_fast_path_access_for_new_members: a.disableFastPath ?? null,
		execution_grace_period: a.grace?.toString() ?? null,
	})
	it("a backfill row and the event that wrote it send the same intents", () => {
		for (const block of [123n, 0n, 0xffff_ffff_ffff_fff0n]) {
			for (const a of parseProposalActions(message({actions: [addMember, publish, flag, voting, badTarget, none], block}))) {
				expect(proposalActionIntents(proposalActionFromRow(rowOf(a)))).toEqual(proposalActionIntents(a))
			}
		}
	})
	it("refuses a row no event could have written", () => {
		const [a] = parseProposalActions(message({actions: [addMember]}))
		expect(() => proposalActionFromRow({...rowOf(a as ProposalAction), action_type: "Bogus"})).toThrow(/action_type/)
		expect(() => proposalActionFromRow({...rowOf(a as ProposalAction), quorum: "1.5"})).toThrow(/quorum/)
	})
})

describe("handleProposalActionMessage", () => {
	it("applies CREATED and UPDATED, ignores everything else", async () => {
		const f = pfixture()
		const v = message({actions: [addMember]})
		expect(await handleProposalActionMessage(f.deps, 0, msg(v, 0, "PROPOSAL_CREATED"))).toBe("applied")
		expect(await handleProposalActionMessage(f.deps, 0, msg(v, 1, "PROPOSAL_UPDATED"))).toBe("applied")
		for (const t of ["PROPOSAL_VOTED", "PROPOSAL_EXECUTED", null]) expect(await handleProposalActionMessage(f.deps, 0, msg(v, 2, t))).toBe("ignored")
		expect(await handleProposalActionMessage(f.deps, 0, msg(null, 3))).toBe("ignored")
		expect(await handleProposalActionMessage(f.deps, 0, msg(message({actions: []}), 4))).toBe("ignored")
	})
	it("dead-letters what kg-indexer rejects, writing nothing", async () => {
		const f = pfixture()
		expect(await handleProposalActionMessage(f.deps, 0, msg(message({settings: false, actions: [addMember]}), 7))).toBe("dead_letter")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toHaveLength(1)
	})
	it("keeps versions apart: an UPDATE at another block adds records and never changes the CREATE's; a replay is a no-op", async () => {
		const model = new ModelStore()
		const f = pfixture({model})
		const create = message({actions: [addMember, flag], block: 10n})
		const update = message({actions: [flag], block: 20n})
		await handleProposalActionMessage(f.deps, 0, msg(create, 0))
		await handleProposalActionMessage(f.deps, 0, msg(update, 1, "PROPOSAL_UPDATED"))
		expect(model.entities.size).toBe(3)
		const before = model.snapshot()
		await handleProposalActionMessage(f.deps, 0, msg(create, 0))
		await handleProposalActionMessage(f.deps, 0, msg(update, 1, "PROPOSAL_UPDATED"))
		expect(model.snapshot()).toBe(before)
	})
	it("a founding cap error throws, and the backfill aborts on it", async () => {
		const f = pfixture({foundFailsOnce: "error: ResourceExhausted (space cap)"})
		await expect(handleProposalActionMessage(f.deps, 0, msg(message({actions: [addMember]}), 0))).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(deadLetters(f.store)).toEqual([])
		const g = pfixture({foundFails: "error: ResourceExhausted (space cap)"})
		const row = {proposal_id: PROPOSAL, index: 0, block: "1", action_type: "Unknown"} as ProposalActionRow
		await expect(backfillProposalActions(g.deps, [{...row, target_id: null, content_uri: null, metadata: null, content_id: null, quorum: null, duration: null, partial_percentage_support_threshold: null, universal_percentage_support_threshold: null, flat_support_threshold: null, disable_fast_path_access_for_new_members: null, execution_grace_period: null}])).rejects.toBeInstanceOf(SystemSpaceCapError)
	})
})

describe("consumer wiring", () => {
	it("is off by default, needs GEODB_RECORD_KEYS, and has its own offsets directory", () => {
		expect(proposalActionsKeyedEnabled({recordKeys: true, proposalActions: {enabled: false}})).toBe(false)
		expect(proposalActionsKeyedEnabled({recordKeys: false, proposalActions: {enabled: true}})).toBe(false)
		expect(proposalActionsKeyedEnabled({recordKeys: true, proposalActions: {enabled: true}})).toBe(true)
		expect(proposalActionsOffsetsDir("/d")).toBe("/d/feeder/proposal-actions-keyed")
		expect(typeof proposalActionsBatchHandler).toBe("function")
	})
})
