import {describe, expect, it} from "vitest"
import {feederGroupIds, loadConfig} from "../config.js"
import {cid} from "../ids.js"
import {keyedBackfillGate} from "../membershipKeyed.js"
import {ProposalOffsets} from "../proposals.js"
import {idPart, intPart, recordEntityId, recordStatementId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {SystemSpaceCapError, systemSpaceNonce} from "../systemSpaces.js"
import {deadLetters, fixture, ModelStore, tmp, VOTES_SPACE} from "./keyedHarness.js"
import {
	backfillUserVotes,
	handleVoteMessage,
	parseVoteCast,
	type UserVote,
	type UserVoteRow,
	userVoteFromRow,
	userVoteIntents,
	userVoteRecordId,
	voteKindKey,
	voteObjectRef,
	votesKeyedEnabled,
	votesOffsetsDir,
} from "../votesKeyed.js"

const vfixture = (o: Parameters<typeof fixture>[0] = {}) => fixture({...o, offsetsDir: votesOffsetsDir})

const KIND = RECORD_KINDS.userVote.id
const COL = RECORD_COLUMNS.userVote
const USER = "b29c345ab9866679b001d7d2138d88a2"
const USER2 = "c39c345ab9866679b001d7d2138d88a3"
const OBJECT = "0d9c345ab9866679b001d7d2138d88a4"
const SPACE = "a19c345ab9866679b001d7d2138d88a1"

const vote = (o: Partial<UserVote> = {}): UserVote => ({
	user: USER,
	object: OBJECT,
	objectType: 0,
	space: SPACE,
	kind: 0,
	direction: 0,
	votedAtSeconds: 1_700_000_000n,
	...o,
})

/** HermesVoteCast bytes (hermes-schema/proto/voting.proto). Omits fields the caller sets to null. */
function voteCast(o: {
	voter?: string | null
	objectType?: string | null
	object?: string | null
	direction?: number
	space?: string | null
	createdAt?: number | null
	kind?: number
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
	const num = (n: number, v: number) => Buffer.concat([varint(BigInt(n << 3)), varint(BigInt(v))])
	const parts: Buffer[] = []
	const voter = o.voter === undefined ? USER : o.voter
	if (voter !== null) parts.push(bytes(1, Buffer.from(voter, "hex")))
	const ot = o.objectType === undefined ? "00000000" : o.objectType
	if (ot !== null) parts.push(bytes(2, Buffer.from(ot, "hex")))
	const obj = o.object === undefined ? OBJECT : o.object
	if (obj !== null) parts.push(bytes(3, Buffer.from(obj, "hex")))
	if (o.direction) parts.push(num(4, o.direction))
	const space = o.space === undefined ? SPACE : o.space
	if (space !== null) parts.push(bytes(7, Buffer.from(space, "hex")))
	const created = o.createdAt === undefined ? 1_700_000_000 : o.createdAt
	if (created !== null) parts.push(bytes(8, Buffer.concat([num(1, created), num(3, 123)])))
	if (o.kind) parts.push(num(9, o.kind))
	return Buffer.concat(parts)
}

const msg = (value: Uint8Array, offset = 0, eventType: string | null = "VOTE_CAST") => ({
	offset: String(offset),
	value,
	headers: eventType === null ? {} : {"event-type": Buffer.from(eventType)},
})

describe("user vote record key (K2)", () => {
	it("is recordEntityId(userVote, [user, object, object type, space, kind]) over the raw gaia ids", () => {
		expect(userVoteRecordId(vote())).toBe(
			recordEntityId(KIND, [idPart(USER), idPart(OBJECT), intPart(0), idPart(SPACE), intPart(0)]),
		)
	})

	it("is pinned (a change re-keys every vote; recordKey.test.ts holds the derivation to geo-db's vectors)", () => {
		expect(userVoteRecordId(vote())).toBe("820ee681699b4d52a3d6f839fc3ed8db")
		expect(userVoteRecordId(vote({objectType: 1, kind: 3}))).toBe("b0c1a8d8f7c44b5798322ea9efaf3174")
	})

	it("is the key alone: direction and time do not move it; every key part does", () => {
		const base = userVoteRecordId(vote())
		expect(userVoteRecordId(vote({direction: 1, votedAtSeconds: 1n}))).toBe(base)
		const others = [
			vote({user: USER2}),
			vote({object: USER2}),
			vote({objectType: 1}),
			vote({space: USER2}),
			vote({kind: 1}),
			vote({kind: 7}),
		].map(userVoteRecordId)
		expect(new Set([base, ...others]).size).toBe(others.length + 1)
	})

	it("keys a negative i16 kind by its u16 bit pattern, injectively, and stores the signed value", () => {
		expect(voteKindKey(3)).toBe(3n)
		expect(voteKindKey(-1)).toBe(65535n)
		expect(voteKindKey(-32768)).toBe(32768n)
		expect(() => voteKindKey(32768)).toThrow()
		const neg = vote({kind: -1})
		expect(userVoteRecordId(neg)).toBe(
			recordEntityId(KIND, [idPart(USER), idPart(OBJECT), intPart(0), idPart(SPACE), intPart(65535)]),
		)
		const ids = [-32768, -2, -1, 0, 1, 3, 32767].map((kind) => userVoteRecordId(vote({kind})))
		expect(new Set(ids).size).toBe(ids.length)
		const record = userVoteRecordId(neg)
		const intents = userVoteIntents(neg).join("\n")
		expect(intents).toContain(
			`.{ .add = .{ .id = "${recordStatementId(KIND, [idPart(record), idPart(COL.kind), intPart(65535)])}", .entity = "${record}", .property = "${COL.kind}", .value = .{ .number = -1 } } }`,
		)
	})

	it("uses the kind's statement role for column ids, so no column id equals a record id", () => {
		const record = userVoteRecordId(vote())
		const ids = userVoteIntents(vote()).flatMap((i) => [...i.matchAll(/\.id = "([0-9a-f]{32})"/g)].map((m) => m[1]))
		expect(ids).not.toContain(record)
		expect(ids).toContain(recordStatementId(KIND, [idPart(record), idPart(COL.direction), intPart(0)]))
	})
})

describe("userVoteIntents", () => {
	it("creates the record, writes each key column once (add + set_active) and the mutable columns with put", () => {
		const record = userVoteRecordId(vote())
		const intents = userVoteIntents(vote({direction: 1}))
		expect(intents[0]).toBe(`.{ .create_entity = "${record}" }`)
		expect(intents.filter((i) => i.startsWith(".{ .add ="))).toHaveLength(5)
		expect(intents.filter((i) => i.startsWith(".{ .set_active ="))).toHaveLength(5)
		const puts = intents.filter((i) => i.startsWith(".{ .put ="))
		expect(puts).toEqual([
			`.{ .put = .{ .id = "${recordStatementId(KIND, [idPart(record), idPart(COL.direction), intPart(1)])}", .entity = "${record}", .property = "${COL.direction}", .value = .{ .number = 1 } } }`,
			`.{ .put = .{ .id = "${recordStatementId(KIND, [idPart(record), idPart(COL.votedAt), intPart(1_700_000_000n)])}", .entity = "${record}", .property = "${COL.votedAt}", .value = .{ .instant = "1700000000000000" } } }`,
		])
		expect(intents.join("\n")).toContain(`.value = .{ .reference = "${cid("entity", USER)}" }`)
		expect(intents.join("\n")).toContain(`.value = .{ .reference = "${cid("entity", SPACE)}" }`)
	})

	it("references a relation's geo-db statement id, an entity's entity id", () => {
		expect(voteObjectRef(OBJECT, 0)).toBe(cid("entity", OBJECT))
		expect(voteObjectRef(OBJECT, 1)).toBe(cid("statement", OBJECT))
		expect(userVoteIntents(vote({objectType: 1})).join("\n")).toContain(`.{ .reference = "${cid("statement", OBJECT)}" }`)
	})

	it("depends on the vote alone", () => {
		expect(userVoteIntents(vote())).toEqual(userVoteIntents(vote()))
	})
})

describe("replace on change (put)", () => {
	const run = async (votes: UserVote[]) => {
		const model = new ModelStore()
		const f = vfixture({model})
		for (const [i, v] of votes.entries()) await handleVoteMessage(f.deps, 0, msg(voteCast(castOf(v)), i))
		return {model, f}
	}
	const castOf = (v: UserVote) => ({
		direction: v.direction,
		kind: v.kind,
		createdAt: Number(v.votedAtSeconds),
		objectType: v.objectType === 1 ? "00000001" : "00000000",
		voter: v.user,
	})

	it("a changed vote leaves exactly one active value per mutable column: the last", async () => {
		const {model} = await run([vote({direction: 0, votedAtSeconds: 10n}), vote({direction: 1, votedAtSeconds: 20n})])
		const record = userVoteRecordId(vote())
		expect(model.cell(record, COL.direction)).toEqual([".{ .number = 1 }"])
		expect(model.cell(record, COL.votedAt)).toEqual(['.{ .instant = "20000000" }'])
		expect(model.cell(record, COL.user)).toEqual([`.{ .reference = "${cid("entity", USER)}" }`])
	})

	it("A -> B -> A ends on A (the value-keyed id reopens)", async () => {
		const {model} = await run([vote({direction: 0}), vote({direction: 1}), vote({direction: 0})])
		expect(model.cell(userVoteRecordId(vote()), COL.direction)).toEqual([".{ .number = 0 }"])
	})

	it("a removal keeps the record with direction 2, as vote-indexer keeps the row", async () => {
		const {model} = await run([vote({direction: 0}), vote({direction: 2})])
		const record = userVoteRecordId(vote())
		expect(model.entities.has(record)).toBe(true)
		expect(model.cell(record, COL.direction)).toEqual([".{ .number = 2 }"])
	})

	it("keys stay apart: two kinds and two voters on one object are four records", async () => {
		const {model} = await run([vote(), vote({kind: 1, direction: 1}), vote({user: USER2}), vote({user: USER2, kind: 3})])
		expect(model.entities.size).toBe(4)
		expect(model.cell(userVoteRecordId(vote({kind: 1})), COL.direction)).toEqual([".{ .number = 1 }"])
		expect(model.cell(userVoteRecordId(vote()), COL.direction)).toEqual([".{ .number = 0 }"])
	})

	it("a replay from zero, or a redelivered last message, over a store that holds the stream converges on the same state", async () => {
		const stream = [vote({direction: 0}), vote({direction: 1, votedAtSeconds: 5n}), vote({user: USER2, direction: 2})]
		const once = await run(stream)
		const model = new ModelStore()
		const f = vfixture({model})
		for (const round of [stream, stream, stream.slice(-1)]) {
			for (const [i, v] of round.entries()) await handleVoteMessage(f.deps, 0, msg(voteCast(castOf(v)), i))
		}
		expect(model.snapshot()).toBe(once.model.snapshot())
	})

	it("a backfill row and the event that wrote it send the same intents", async () => {
		const row: UserVoteRow = {
			user_id: "b29c345a-b986-6679-b001-d7d2138d88a2",
			object_id: "0d9c345a-b986-6679-b001-d7d2138d88a4",
			object_type: 0,
			space_id: "a19c345a-b986-6679-b001-d7d2138d88a1",
			vote_kind: 2,
			vote_type: 1,
			voted_at_seconds: "1700000000",
		}
		expect(userVoteIntents(userVoteFromRow(row))).toEqual(
			userVoteIntents(parseVoteCast(voteCast({direction: 1, kind: 2}))),
		)
		// A negative kind, as vote-indexer stores `kind as i16`: the row and the event agree.
		expect(userVoteIntents(userVoteFromRow({...row, vote_kind: -1}))).toEqual(
			userVoteIntents(parseVoteCast(voteCast({direction: 1, kind: 0xffff}))),
		)
		expect(() => userVoteFromRow({...row, vote_kind: 40000})).toThrow()
	})
})

describe("parseVoteCast (vote-indexer's rules)", () => {
	it("decodes a cast", () => {
		expect(parseVoteCast(voteCast({direction: 2, kind: 1, objectType: "00000001"}))).toEqual(
			vote({direction: 2, kind: 1, objectType: 1}),
		)
	})
	it("keeps an unknown kind's raw value as its own key", () => {
		expect(parseVoteCast(voteCast({kind: 9})).kind).toBe(9)
	})
	it("narrows the kind to i16 as vote-indexer does, keeping negatives", () => {
		expect(parseVoteCast(voteCast({kind: -1})).kind).toBe(-1)
		expect(parseVoteCast(voteCast({kind: 0xffff})).kind).toBe(-1)
		expect(parseVoteCast(voteCast({kind: 0x8000})).kind).toBe(-32768)
	})
	it.each([
		["missing meta", {createdAt: null}],
		["an object type other than entity or relation", {objectType: "00000002"}],
		["a short id", {voter: "b29c"}],
		["a missing space", {space: null}],
		["an unknown direction", {direction: 3}],
		["Interested + Down", {kind: 3, direction: 1}],
	])("rejects %s", (_name, o) => {
		expect(() => parseVoteCast(voteCast(o))).toThrow()
	})
})

describe("handleVoteMessage", () => {
	it("founds the votes Space once with the votes nonce and writes every vote there", async () => {
		const f = vfixture()
		expect(await handleVoteMessage(f.deps, 0, msg(voteCast(), 0))).toBe("applied")
		expect(await handleVoteMessage(f.deps, 1, msg(voteCast({voter: USER2}), 1))).toBe("applied")
		expect(f.founds).toEqual([systemSpaceNonce("votes")])
		expect(f.founds[0]).toBe("744a1a2a52d0432199372b395a4057b7")
		expect(f.systemSpaces.records.votes?.geo).toBe(VOTES_SPACE)
		expect(f.applied.every((z) => z.includes(VOTES_SPACE))).toBe(true)
		expect(JSON.parse(f.published.at(-1) as string)).toMatchObject({votes: {geo: VOTES_SPACE}})
	})

	it("dead-letters what vote-indexer rejects, writing nothing", async () => {
		const f = vfixture()
		expect(await handleVoteMessage(f.deps, 0, msg(voteCast({kind: 3, direction: 1}), 7))).toBe("dead_letter")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toHaveLength(1)
	})

	it("ignores an empty value, and decodes regardless of event-type as vote-indexer does", async () => {
		const f = vfixture()
		expect(await handleVoteMessage(f.deps, 0, {offset: "0", value: null})).toBe("ignored")
		expect(await handleVoteMessage(f.deps, 0, msg(voteCast(), 1, "SOMETHING_ELSE"))).toBe("applied")
		expect(await handleVoteMessage(f.deps, 0, msg(voteCast({voter: USER2}), 2, null))).toBe("applied")
	})
})

describe("stall, never skip", () => {
	it("a ResourceExhausted founding throws (the consumer retries in place) and dead-letters nothing", async () => {
		const f = vfixture({foundFailsOnce: "error: ResourceExhausted (space cap)"})
		await expect(handleVoteMessage(f.deps, 0, msg(voteCast(), 0))).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toEqual([])
		expect(await handleVoteMessage(f.deps, 0, msg(voteCast(), 0))).toBe("applied")
	})

	it("a ResourceExhausted apply is retried until it lands, not dead-lettered", async () => {
		const f = vfixture({applyFailures: ["error: ResourceExhausted", "error: ResourceExhausted"]})
		expect(await handleVoteMessage(f.deps, 0, msg(voteCast(), 0))).toBe("applied")
		expect(f.applied).toHaveLength(1)
		expect(deadLetters(f.store)).toEqual([])
	})

	it("the backfill aborts on the first cap error, writing nothing", async () => {
		const f = vfixture({foundFails: "error: ResourceExhausted (space cap)"})
		const rows = [userVoteRowOf(vote())]
		await expect(backfillUserVotes(f.deps, rows)).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(f.applied).toEqual([])
	})

	it("refuses to send put while not allowed", async () => {
		const f = vfixture()
		await expect(handleVoteMessage({...f.deps, allowPut: false}, 0, msg(voteCast(), 0))).rejects.toThrow(/GEODB_RECORD_KEYS is off/)
	})
})

const userVoteRowOf = (v: UserVote): UserVoteRow => ({
	user_id: v.user,
	object_id: v.object,
	object_type: v.objectType,
	space_id: v.space,
	vote_kind: v.kind,
	vote_type: v.direction,
	voted_at_seconds: String(v.votedAtSeconds),
})

describe("backfillUserVotes", () => {
	it("writes each row as its record, from an async page source", async () => {
		const model = new ModelStore()
		const f = vfixture({model})
		async function* rows() {
			yield userVoteRowOf(vote())
			yield userVoteRowOf(vote({kind: 1, direction: 1}))
		}
		expect(await backfillUserVotes(f.deps, rows())).toEqual({rows: 2, applied: 2, deadLetter: 0})
		expect(model.cell(userVoteRecordId(vote({kind: 1})), COL.direction)).toEqual([".{ .number = 1 }"])
	})

	it("dead-letters a row no event could have written, and counts it", async () => {
		const f = vfixture()
		const bad = {...userVoteRowOf(vote()), vote_type: 5}
		expect(await backfillUserVotes(f.deps, [bad, userVoteRowOf(vote())])).toEqual({rows: 2, applied: 1, deadLetter: 1})
	})

	it("stops when aborted", async () => {
		const f = vfixture()
		const abort = new AbortController()
		abort.abort()
		await expect(backfillUserVotes({...f.deps, signal: abort.signal}, [userVoteRowOf(vote())])).rejects.toThrow(/aborted|shutting down/)
	})
})

describe("flag and gate", () => {
	const base = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}
	it("is off by default, and GEODB_VOTES alone does nothing without GEODB_RECORD_KEYS", () => {
		const off = loadConfig(base)
		expect(off.votes).toEqual({enabled: false, topic: "testnet.curation.votes", groupId: "geodb-feeder-votes-keyed-v1-testnet"})
		expect(votesKeyedEnabled(off)).toBe(false)
		expect(votesKeyedEnabled(loadConfig({...base, GEODB_VOTES: "1"}))).toBe(false)
		expect(votesKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1"}))).toBe(false)
		expect(votesKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_VOTES: "1"}))).toBe(true)
	})

	it("the backfill gate refuses once the votes consumer has offsets, unless told to reset them", () => {
		const dir = votesOffsetsDir(tmp())
		expect(keyedBackfillGate(dir, false)).toBe("fresh")
		const offsets = new ProposalOffsets(dir)
		offsets.markDone(0, 4)
		expect(() => keyedBackfillGate(dir, false)).toThrow(/BACKFILL_KEYED_RESET_CONSUMER/)
		expect(keyedBackfillGate(dir, true)).toBe("reset")
		expect(new ProposalOffsets(dir).hadOffsets).toBe(false)
	})

	it("keeps its offsets apart from the membership consumer's", () => {
		expect(votesOffsetsDir("/data")).toBe("/data/feeder/votes-keyed")
	})

	it("the backfill group check covers every consumer group the config defines", () => {
		const c = loadConfig(base)
		const all: string[] = []
		const walk = (o: unknown) => {
			if (!o || typeof o !== "object") return
			for (const [k, v] of Object.entries(o)) {
				if (/groupid$/i.test(k) && typeof v === "string") all.push(v)
				else walk(v)
			}
		}
		walk(c)
		expect(all.length).toBeGreaterThanOrEqual(6)
		expect(new Set(feederGroupIds(c))).toEqual(new Set(all))
	})
})
