import {describe, expect, it} from "vitest"
import {feederGroupIds, loadConfig} from "../config.js"
import {cid} from "../ids.js"
import {keyedBackfillGate} from "../membershipKeyed.js"
import {createMetrics} from "../metrics.js"
import {proposalVotesBatchHandler} from "../proposalVotesConsumer.js"
import {
	backfillProposalVotes,
	handleProposalVoteMessage,
	type KeyedProposalVotesDeps,
	normalizeProposalVersion,
	type ProposalVote,
	type ProposalVoteRow,
	parseProposalVoted,
	proposalVersionKey,
	proposalVoteFromRow,
	proposalVoteIntents,
	proposalVoteRecordId,
	proposalVotesKeyedEnabled,
	proposalVotesOffsetsDir,
} from "../proposalVotesKeyed.js"
import {ProposalOffsets} from "../proposals.js"
import {idPart, intPart, recordEntityId, recordStatementId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {SystemSpaceCapError, systemSpaceNonce} from "../systemSpaces.js"
import {votesOffsetsDir} from "../votesKeyed.js"
import {deadLetters, fixture, ModelStore, tmp, VOTES_SPACE} from "./keyedHarness.js"

const KIND = RECORD_KINDS.proposalVote.id
const COL = RECORD_COLUMNS.proposalVote
const PROPOSAL = "7e1c345ab9866679b001d7d2138d88b1"
const VOTER = "b29c345ab9866679b001d7d2138d88a2"
const VOTER2 = "c39c345ab9866679b001d7d2138d88a3"
const SPACE = "a19c345ab9866679b001d7d2138d88a1"

const pfixture = (o: Parameters<typeof fixture>[0] = {}) => fixture({...o, offsetsDir: proposalVotesOffsetsDir})

const pvote = (o: Partial<ProposalVote> = {}): ProposalVote => ({
	proposal: PROPOSAL,
	version: 1,
	voter: VOTER,
	space: SPACE,
	vote: 1,
	createdAtSeconds: 1_700_000_000n,
	createdAtBlock: 123n,
	...o,
})

/** HermesProposalVoted bytes (hermes-schema/proto/governance.proto). Omits fields the caller sets to null. */
function voted(o: {
	voter?: string | null
	space?: string | null
	proposal?: string | null
	vote?: bigint
	meta?: {createdAt: bigint; block: bigint} | null
	version?: bigint
} = {}): Uint8Array {
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
	const bytes = (n: number, b: Buffer) => Buffer.concat([varint(BigInt((n << 3) | 2)), varint(BigInt(b.length)), b])
	const num = (n: number, v: bigint) => Buffer.concat([varint(BigInt(n << 3)), varint(v)])
	const parts: Buffer[] = []
	const id = (n: number, v: string | null | undefined, d: string) => {
		const x = v === undefined ? d : v
		if (x !== null) parts.push(bytes(n, Buffer.from(x, "hex")))
	}
	id(1, o.voter, VOTER)
	id(2, o.space, SPACE)
	id(3, o.proposal, PROPOSAL)
	const vote = o.vote ?? 1n
	if (vote) parts.push(num(4, vote))
	const meta = o.meta === undefined ? {createdAt: 1_700_000_000n, block: 123n} : o.meta
	if (meta !== null) parts.push(bytes(5, Buffer.concat([num(1, meta.createdAt), num(3, meta.block)])))
	const version = o.version ?? 1n
	if (version) parts.push(num(6, version))
	return Buffer.concat(parts)
}

const msg = (value: Uint8Array, offset = 0, eventType: string | null = "PROPOSAL_VOTED") => ({
	offset: String(offset),
	value,
	headers: eventType === null ? {} : {"event-type": Buffer.from(eventType)},
})

const votedOf = (v: ProposalVote) => ({
	voter: v.voter,
	space: v.space,
	proposal: v.proposal,
	vote: BigInt(v.vote),
	meta: {createdAt: v.createdAtSeconds, block: v.createdAtBlock},
	version: BigInt(v.version),
})

const rowOf = (v: ProposalVote): ProposalVoteRow => ({
	proposal_id: `${v.proposal.slice(0, 8)}-${v.proposal.slice(8, 12)}-${v.proposal.slice(12, 16)}-${v.proposal.slice(16, 20)}-${v.proposal.slice(20)}`,
	proposal_version: v.version,
	voter_id: v.voter.toUpperCase(),
	space_id: v.space,
	vote: ({1: "Yes", 2: "No", 3: "Abstain"} as const)[v.vote],
	created_at: String(v.createdAtSeconds),
	created_at_block: String(v.createdAtBlock),
})

describe("proposal vote record key (K2)", () => {
	it("is recordEntityId(proposalVote, [proposal, version, voter]) over the raw gaia ids: proposal_votes' primary key", () => {
		expect(proposalVoteRecordId(pvote())).toBe(recordEntityId(KIND, [idPart(PROPOSAL), intPart(1), idPart(VOTER)]))
	})

	it("is pinned (a change re-keys every proposal vote)", () => {
		expect(proposalVoteRecordId(pvote())).toBe("1d9b034885ab49eb8224bc75d645ac4d")
		expect(KIND).toBe("ca89a877762c4883a3640b5455a8f0e2")
		expect(RECORD_KINDS.proposalVote.family).toBe("votes")
	})

	it("is the key alone: vote, time, block and owning Space do not move it; every key part does", () => {
		const id = proposalVoteRecordId(pvote())
		expect(proposalVoteRecordId(pvote({vote: 2, createdAtSeconds: 5n, createdAtBlock: 9n, space: VOTER2}))).toBe(id)
		const moved = [pvote({proposal: VOTER2}), pvote({version: 2}), pvote({voter: VOTER2})].map(proposalVoteRecordId)
		expect(new Set([id, ...moved]).size).toBe(4)
	})

	it("keys a version as its u32 bit pattern: 0 is read as 1 and a u32 above i32::MAX wraps, as kg-indexer stores it", () => {
		expect(normalizeProposalVersion(0n)).toBe(1)
		expect(normalizeProposalVersion(7n)).toBe(7)
		expect(normalizeProposalVersion(0x8000_0000n)).toBe(-(2 ** 31))
		expect(proposalVersionKey(-(2 ** 31))).toBe(0x8000_0000n)
		expect(proposalVersionKey(3)).toBe(3n)
		expect(() => proposalVersionKey(2 ** 31)).toThrow(/i32/)
	})
})

describe("proposalVoteIntents", () => {
	it("creates the record, writes each key column once (add + set_active) and the mutable columns with put", () => {
		const intents = proposalVoteIntents(pvote())
		const record = proposalVoteRecordId(pvote())
		expect(intents[0]).toBe(`.{ .create_entity = "${record}" }`)
		expect(intents.filter((i) => i.includes(".add = ")).length).toBe(4)
		expect(intents.filter((i) => i.includes(".set_active = ")).length).toBe(4)
		expect(intents.filter((i) => i.includes(".put = ")).length).toBe(3)
		const all = intents.join("\n")
		for (const [p, t] of [
			[COL.proposal, cid("entity", PROPOSAL)],
			[COL.voter, cid("entity", VOTER)],
			[COL.space, cid("entity", SPACE)],
		] as const) {
			const sid = recordStatementId(KIND, [idPart(record), idPart(p), idPart(t)])
			expect(all).toContain(`.add = .{ .id = "${sid}", .entity = "${record}", .property = "${p}", .value = .{ .reference = "${t}" } }`)
		}
		expect(all).toContain(`.property = "${COL.version}", .value = .{ .number = 1 }`)
		expect(all).toContain(`.property = "${COL.vote}", .value = .{ .number = 1 }`)
		expect(all).toContain(`.property = "${COL.createdAt}", .value = .{ .instant = "1700000000000000" }`)
		expect(all).toContain(`.property = "${COL.createdAtBlock}", .value = .{ .number = 123 }`)
	})

	it("depends on the vote alone", () => {
		expect(proposalVoteIntents(pvote())).toEqual(proposalVoteIntents(pvote()))
	})
})

describe("replace on change (put)", () => {
	const run = async (votes: ProposalVote[]) => {
		const model = new ModelStore()
		const f = pfixture({model})
		for (const [i, v] of votes.entries()) expect(await handleProposalVoteMessage(f.deps, 0, msg(voted(votedOf(v)), i))).toBe("applied")
		return {model, f}
	}

	it("a changed vote leaves exactly one active value per mutable column: the last, as ON CONFLICT DO UPDATE does", async () => {
		const {model} = await run([pvote({vote: 1, createdAtSeconds: 10n, createdAtBlock: 1n}), pvote({vote: 2, createdAtSeconds: 20n, createdAtBlock: 2n})])
		const record = proposalVoteRecordId(pvote())
		expect(model.cell(record, COL.vote)).toEqual([".{ .number = 2 }"])
		expect(model.cell(record, COL.createdAt)).toEqual(['.{ .instant = "20000000" }'])
		expect(model.cell(record, COL.createdAtBlock)).toEqual([".{ .number = 2 }"])
		expect(model.cell(record, COL.voter)).toEqual([`.{ .reference = "${cid("entity", VOTER)}" }`])
	})

	it("A -> B -> A ends on A (the value-keyed id reopens)", async () => {
		const {model} = await run([pvote({vote: 1}), pvote({vote: 3}), pvote({vote: 1})])
		expect(model.cell(proposalVoteRecordId(pvote()), COL.vote)).toEqual([".{ .number = 1 }"])
	})

	it("keys stay apart: two versions and two voters on one proposal are four records", async () => {
		const vs = [pvote(), pvote({version: 2}), pvote({voter: VOTER2}), pvote({version: 2, voter: VOTER2, vote: 2})]
		const {model} = await run(vs)
		expect(model.entities.size).toBe(4)
		expect(model.cell(proposalVoteRecordId(vs[3] as ProposalVote), COL.vote)).toEqual([".{ .number = 2 }"])
	})

	it("a replay from zero over a store that holds the stream converges on the same state", async () => {
		const stream = [pvote({vote: 1}), pvote({voter: VOTER2}), pvote({vote: 2, createdAtSeconds: 30n})]
		const {model, f} = await run(stream)
		const before = model.snapshot()
		for (const [i, v] of stream.entries()) await handleProposalVoteMessage(f.deps, 0, msg(voted(votedOf(v)), i))
		expect(model.snapshot()).toBe(before)
	})

	it("a backfill row and the event that wrote it send the same intents", async () => {
		for (const v of [pvote(), pvote({vote: 3, version: 4, createdAtSeconds: 0n, createdAtBlock: 0n}), pvote({version: -(2 ** 31)})]) {
			expect(proposalVoteIntents(proposalVoteFromRow(rowOf(v)))).toEqual(proposalVoteIntents(parseProposalVoted(voted(votedOf(v)))))
		}
	})
})

describe("parseProposalVoted (kg-indexer's handle_proposal_voted)", () => {
	it("decodes a vote", () => {
		expect(parseProposalVoted(voted({vote: 2n, version: 3n}))).toEqual(pvote({vote: 2, version: 3}))
	})

	it("writes none (0) and an unknown option as abstain", () => {
		expect(parseProposalVoted(voted({vote: 0n})).vote).toBe(3)
		expect(parseProposalVoted(voted({vote: 9n})).vote).toBe(3)
		expect(parseProposalVoted(voted({vote: -1n})).vote).toBe(3)
	})

	it("reads an unset version as 1, and missing metadata as time 0, block 0", () => {
		expect(parseProposalVoted(voted({version: 0n, meta: null}))).toEqual(pvote({version: 1, createdAtSeconds: 0n, createdAtBlock: 0n}))
	})

	it("refuses an id that is not 16 bytes, as Uuid::from_slice does", () => {
		expect(() => parseProposalVoted(voted({proposal: null}))).toThrow(/proposal_id/)
		expect(() => parseProposalVoted(voted({voter: "abcd"}))).toThrow(/voter_id/)
		expect(() => parseProposalVoted(voted({space: null}))).toThrow(/space_id/)
	})
})

describe("handleProposalVoteMessage", () => {
	it("founds the votes Space once with the votes nonce and writes every proposal vote there", async () => {
		const f = pfixture()
		expect(await handleProposalVoteMessage(f.deps, 0, msg(voted(), 0))).toBe("applied")
		expect(await handleProposalVoteMessage(f.deps, 1, msg(voted({voter: VOTER2}), 1))).toBe("applied")
		expect(f.founds).toEqual([systemSpaceNonce("votes")])
		expect(f.applied.every((z) => z.includes(VOTES_SPACE))).toBe(true)
		expect(JSON.parse(f.published.at(-1) as string)).toMatchObject({votes: {geo: VOTES_SPACE}})
	})

	it("ignores every other event type on the topic, and a message with no header, as kg-indexer routes by it", async () => {
		const f = pfixture()
		for (const t of ["PROPOSAL_CREATED", "PROPOSAL_EXECUTED", "PROPOSAL_UPDATED", null]) {
			expect(await handleProposalVoteMessage(f.deps, 0, msg(voted(), 0, t))).toBe("ignored")
		}
		expect(await handleProposalVoteMessage(f.deps, 0, {offset: "0", value: null, headers: {"event-type": "PROPOSAL_VOTED"}})).toBe("ignored")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toEqual([])
	})

	it("reads the header as kafkajs hands it: a Buffer, a string or an array", async () => {
		const f = pfixture()
		for (const h of [Buffer.from("PROPOSAL_VOTED"), "PROPOSAL_VOTED", [Buffer.from("PROPOSAL_VOTED")]]) {
			expect(await handleProposalVoteMessage(f.deps, 0, {offset: "0", value: voted(), headers: {"event-type": h}})).toBe("applied")
		}
	})

	it("dead-letters what kg-indexer rejects, writing nothing", async () => {
		const f = pfixture()
		expect(await handleProposalVoteMessage(f.deps, 0, msg(voted({space: "ab"}), 7))).toBe("dead_letter")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toHaveLength(1)
	})
})

describe("stall, never skip", () => {
	it("a ResourceExhausted founding throws (the consumer retries in place) and dead-letters nothing", async () => {
		const f = pfixture({foundFailsOnce: "error: ResourceExhausted (space cap)"})
		await expect(handleProposalVoteMessage(f.deps, 0, msg(voted(), 0))).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(deadLetters(f.store)).toEqual([])
		expect(await handleProposalVoteMessage(f.deps, 0, msg(voted(), 0))).toBe("applied")
	})

	it("the backfill aborts on the first cap error, writing nothing", async () => {
		const f = pfixture({foundFails: "error: ResourceExhausted (space cap)"})
		await expect(backfillProposalVotes(f.deps, [rowOf(pvote())])).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(f.applied).toEqual([])
	})

	it("refuses to send put while not allowed", async () => {
		const f = pfixture()
		await expect(handleProposalVoteMessage({...f.deps, allowPut: false}, 0, msg(voted(), 0))).rejects.toThrow(/GEODB_RECORD_KEYS is off/)
	})

	it("the batch loop retries a failing message in place and counts outcomes on the proposal-votes series", async () => {
		const done = new Map<number, number>()
		const deps = {
			metrics: createMetrics(),
			offsets: {hadOffsets: false, lastDone: (p: number) => done.get(p) ?? null, markDone: (p: number, o: number) => void done.set(p, o)},
		} as unknown as KeyedProposalVotesDeps
		const tries: string[] = []
		let failures = 1
		const run = proposalVotesBatchHandler({
			deps,
			abort: new AbortController(),
			commits: {done: async () => {}, flush: async () => {}},
			sleep: async () => {},
			handle: async (_d, _p, m) => {
				tries.push(m.offset)
				if (m.offset === "5" && failures-- > 0) throw new Error("ResourceExhausted")
				return m.offset === "6" ? "ignored" : "applied"
			},
		})
		await run({
			batch: {partition: 0, messages: ["5", "6"].map((offset) => ({offset, value: Buffer.from([1]), timestamp: "0"}))},
			resolveOffset: () => {},
			heartbeat: async () => {},
		})
		expect(tries).toEqual(["5", "5", "6"])
		expect(done.get(0)).toBe(6)
		expect(deps.metrics.proposalVoteEvents.values).toEqual(new Map([['outcome="applied"', 1], ['outcome="ignored"', 1]]))
		expect(deps.metrics.voteEvents.values.size).toBe(0)
	})
})

describe("backfillProposalVotes", () => {
	it("writes each row as its record, from an async page source", async () => {
		const model = new ModelStore()
		const f = pfixture({model})
		async function* rows() {
			yield rowOf(pvote())
			yield rowOf(pvote({voter: VOTER2, vote: 2}))
		}
		expect(await backfillProposalVotes(f.deps, rows())).toEqual({rows: 2, applied: 2, deadLetter: 0})
		expect(model.cell(proposalVoteRecordId(pvote({voter: VOTER2})), COL.vote)).toEqual([".{ .number = 2 }"])
	})

	it("dead-letters a row no event could have written, and counts it", async () => {
		const f = pfixture()
		const bad = [
			{...rowOf(pvote()), vote: "Maybe"},
			{...rowOf(pvote()), proposal_version: 0},
			{...rowOf(pvote()), created_at: "1.5"},
			{...rowOf(pvote()), created_at_block: "99999999999999999999"},
		]
		expect(await backfillProposalVotes(f.deps, [...bad, rowOf(pvote())])).toEqual({rows: 5, applied: 1, deadLetter: 4})
	})

	it("stops when aborted", async () => {
		const f = pfixture()
		const abort = new AbortController()
		abort.abort()
		await expect(backfillProposalVotes({...f.deps, signal: abort.signal}, [rowOf(pvote())])).rejects.toThrow(/aborted|shutting down/)
	})
})

describe("flag and gate", () => {
	const base = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}
	it("is off by default, needs GEODB_RECORD_KEYS, and GEODB_VOTES does not turn it on", () => {
		const off = loadConfig(base)
		expect(off.proposalVotes).toEqual({
			enabled: false,
			topic: "testnet.space.governance",
			groupId: "geodb-feeder-proposal-votes-keyed-v1-testnet",
		})
		expect(proposalVotesKeyedEnabled(off)).toBe(false)
		expect(proposalVotesKeyedEnabled(loadConfig({...base, GEODB_PROPOSAL_VOTES: "1"}))).toBe(false)
		expect(proposalVotesKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_VOTES: "1"}))).toBe(false)
		expect(proposalVotesKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_PROPOSAL_VOTES: "1"}))).toBe(true)
	})

	it("has its own group, apart from the proposals consumer's on the same topic, and is in the backfill group check", () => {
		const c = loadConfig(base)
		expect(c.proposalVotes.groupId).not.toBe(c.proposals.groupId)
		expect(feederGroupIds(c)).toContain(c.proposalVotes.groupId)
	})

	it("the backfill gate refuses once the consumer has offsets, unless told to reset them", () => {
		const dir = proposalVotesOffsetsDir(tmp())
		expect(keyedBackfillGate(dir, false)).toBe("fresh")
		new ProposalOffsets(dir).markDone(0, 4)
		expect(() => keyedBackfillGate(dir, false)).toThrow(/BACKFILL_KEYED_RESET_CONSUMER/)
		expect(keyedBackfillGate(dir, true)).toBe("reset")
	})

	it("keeps its offsets apart from the user-votes consumer's", () => {
		expect(proposalVotesOffsetsDir("/data")).toBe("/data/feeder/proposal-votes-keyed")
		expect(proposalVotesOffsetsDir("/data")).not.toBe(votesOffsetsDir("/data"))
	})
})
