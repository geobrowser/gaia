import {describe, expect, it} from "vitest"
import {feederGroupIds, loadConfig} from "../config.js"
import {cid} from "../ids.js"
import {keyedBackfillGate} from "../membershipKeyed.js"
import {createMetrics} from "../metrics.js"
import {ProposalOffsets} from "../proposals.js"
import {RECORD_COLUMNS} from "../recordKinds.js"
import {subspacesBatchHandler} from "../subspacesConsumer.js"
import {
	backfillSubspaces,
	handleSubspaceMessage,
	type KeyedSubspacesDeps,
	parseTrustExtension,
	type SubspaceEvent,
	subspaceFromRow,
	subspaceIntents,
	subspaceStatementId,
	subspaceSubject,
	subspacesKeyedEnabled,
	subspacesOffsetsDir,
} from "../subspacesKeyed.js"
import {SystemSpaceCapError} from "../systemSpaces.js"
import {deadLetters, fixture, ModelStore, tmp} from "./keyedHarness.js"

const PARENT = "a19c345ab9866679b001d7d2138d88a1"
const CHILD = "b29c345ab9866679b001d7d2138d88a2"
const TOPIC = "c39c345ab9866679b001d7d2138d88a3"

const sfixture = (o: Parameters<typeof fixture>[0] = {}) => fixture({...o, offsetsDir: subspacesOffsetsDir})
const varint = (n: number) => Buffer.from([n])
const len = (f: number, b: Buffer) => Buffer.concat([varint((f << 3) | 2), varint(b.length), b])
const hexb = (h: string) => Buffer.from(h, "hex")
/** A trust extension: source, then the oneof member `member` holding one target. */
const trust = (member: number | null, target: string | Buffer = CHILD, source: Buffer = hexb(PARENT)) =>
	Buffer.concat([len(1, source), ...(member === null ? [] : [len(member, len(1, typeof target === "string" ? hexb(target) : target))])])
const msg = (value: Buffer | null, type = "TRUST_EXTENSION", offset = "0") => ({offset, value, headers: {"event-type": Buffer.from(type)}})
const ev = (o: Partial<SubspaceEvent> = {}): SubspaceEvent => ({kind: "add", space: PARENT, target: CHILD, type: "verified", ...o})

describe("parseTrustExtension", () => {
	it("maps each of the six variants", () => {
		const want: [number, SubspaceEvent["kind"], SubspaceEvent["type"]][] = [
			[2, "add", "verified"], [3, "add", "related"], [4, "add", "topic"], [6, "remove", "verified"], [7, "remove", "related"], [8, "remove", "topic"],
		]
		for (const [f, kind, type] of want) expect(parseTrustExtension(trust(f))).toEqual({kind, space: PARENT, target: CHILD, type})
	})
	it("returns null with no variant, and throws on a bad source or target", () => {
		expect(parseTrustExtension(trust(null))).toBeNull()
		expect(() => parseTrustExtension(trust(2, CHILD, Buffer.alloc(15)))).toThrow(/16-byte source/)
		expect(() => parseTrustExtension(trust(2, Buffer.alloc(3)))).toThrow(/16-byte target/)
		expect(() => parseTrustExtension(Buffer.concat([len(1, hexb(PARENT)), Buffer.from([(2 << 3) | 0, 1])]))).toThrow(/not a message/)
	})
})

describe("intents", () => {
	it("an add creates the subject, adds the edge and activates it; a removal only deactivates", () => {
		const sid = subspaceStatementId(PARENT, CHILD, "verified")
		const add = subspaceIntents(ev())
		expect(add).toHaveLength(3)
		expect(add[1]).toContain(`.id = "${sid}"`)
		expect(add[1]).toContain(`.property = "${RECORD_COLUMNS.subspace.verified}"`)
		expect(add[1]).toContain(`.reference = "${cid("entity", CHILD)}"`)
		expect(subspaceIntents(ev({kind: "remove"}))).toEqual([`.{ .set_active = .{ .statement = "${sid}", .active = false } }`])
	})
	it("keeps the types apart: verified, related and topic are three edges", () => {
		const ids = [subspaceStatementId(PARENT, CHILD, "verified"), subspaceStatementId(PARENT, CHILD, "related"), subspaceStatementId(PARENT, CHILD, "topic")]
		expect(new Set(ids).size).toBe(3)
		expect(subspaceIntents(ev({type: "topic"}))[1]).toContain(`.property = "${RECORD_COLUMNS.subspaceTopic.topic}"`)
	})
})

describe("handleSubspaceMessage", () => {
	it("an add then a removal leaves the edge inactive; a re-add reactivates it; a replay is a no-op", async () => {
		const model = new ModelStore()
		const f = sfixture({model})
		const sid = subspaceStatementId(PARENT, CHILD, "related")
		const active = () => model.statements.get(sid)?.active
		expect(await handleSubspaceMessage(f.deps, 0, msg(trust(3), "TRUST_EXTENSION", "1"))).toBe("applied")
		expect(active()).toBe(true)
		expect(model.entities.has(subspaceSubject(PARENT))).toBe(true)
		await handleSubspaceMessage(f.deps, 0, msg(trust(7), "TRUST_EXTENSION", "2"))
		expect(active()).toBe(false)
		await handleSubspaceMessage(f.deps, 0, msg(trust(3), "TRUST_EXTENSION", "3"))
		expect(active()).toBe(true)
		const snap = model.snapshot()
		await handleSubspaceMessage(f.deps, 0, msg(trust(3), "TRUST_EXTENSION", "3"))
		expect(model.snapshot()).toBe(snap)
	})
	it("a removal of an edge never seen is admitted", async () => {
		const model = new ModelStore()
		const f = sfixture({model})
		expect(await handleSubspaceMessage(f.deps, 0, msg(trust(6)))).toBe("applied")
	})
	it("ignores other event types, empty values and an unknown variant", async () => {
		const f = sfixture()
		expect(await handleSubspaceMessage(f.deps, 0, msg(trust(2), "ROLE_GRANTED"))).toBe("ignored")
		expect(await handleSubspaceMessage(f.deps, 0, msg(null))).toBe("ignored")
		expect(await handleSubspaceMessage(f.deps, 0, msg(trust(null)))).toBe("ignored")
		expect(f.applied).toEqual([])
	})
	it("dead-letters a message that does not decode", async () => {
		const f = sfixture()
		expect(await handleSubspaceMessage(f.deps, 0, msg(trust(2, Buffer.alloc(3))))).toBe("dead_letter")
		expect(deadLetters(f.store)).toHaveLength(1)
	})
	it("throws when the governance Space cannot be founded (the consumer retries in place)", async () => {
		const f = sfixture({foundFails: "ResourceExhausted"})
		await expect(handleSubspaceMessage(f.deps, 0, msg(trust(2)))).rejects.toThrow(SystemSpaceCapError)
	})
	it("the batch loop retries a failing message in place and counts outcomes on the subspaces series", async () => {
		const done = new Map<number, number>()
		const deps = {
			metrics: createMetrics(),
			offsets: {hadOffsets: false, lastDone: (p: number) => done.get(p) ?? null, markDone: (p: number, o: number) => void done.set(p, o)},
		} as unknown as KeyedSubspacesDeps
		const tries: string[] = []
		let failures = 1
		const run = subspacesBatchHandler({
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
		expect(deps.metrics.subspaceEvents.values).toEqual(new Map([['outcome="applied"', 1], ['outcome="ignored"', 1]]))
	})
})

describe("backfillSubspaces", () => {
	const row = (type: string, parent = PARENT, target = CHILD) => ({parent_space_id: parent, target_id: target, type})
	it("writes each row as the edge a live add writes (dashed ids too)", async () => {
		const live = new ModelStore()
		await handleSubspaceMessage(sfixture({model: live}).deps, 0, msg(trust(4, TOPIC)))
		const model = new ModelStore()
		const dashed = (h: string) => `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
		const r = await backfillSubspaces(sfixture({model}).deps, [row("topic", dashed(PARENT), dashed(TOPIC)), row("verified")])
		expect(r).toEqual({rows: 2, applied: 2, deadLetter: 0})
		expect(model.statements.get(subspaceStatementId(PARENT, TOPIC, "topic"))).toEqual(live.statements.get(subspaceStatementId(PARENT, TOPIC, "topic")))
	})
	it("dead-letters a row no event could have written", async () => {
		const f = sfixture()
		const r = await backfillSubspaces(f.deps, [row("friend"), row("verified", "abc"), row("related")])
		expect(r).toEqual({rows: 3, applied: 1, deadLetter: 2})
		expect(() => subspaceFromRow(row("friend"))).toThrow(/unknown subspace type/)
	})
	it("stops when aborted", async () => {
		const abort = new AbortController()
		abort.abort()
		await expect(backfillSubspaces({...sfixture().deps, signal: abort.signal}, [row("verified")])).rejects.toThrow(/aborted|shutting down/)
	})
})

describe("flag and gate", () => {
	const base = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}
	it("is off by default and needs GEODB_RECORD_KEYS", () => {
		const off = loadConfig(base)
		expect(off.subspaces).toEqual({enabled: false, topic: "testnet.space.trust.extensions", groupId: "geodb-feeder-subspaces-keyed-v1-testnet"})
		expect(subspacesKeyedEnabled(off)).toBe(false)
		expect(subspacesKeyedEnabled(loadConfig({...base, GEODB_SUBSPACES: "1"}))).toBe(false)
		expect(subspacesKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_SUBSPACES: "1"}))).toBe(true)
	})
	it("has its own group, in the backfill group check", () => {
		expect(feederGroupIds(loadConfig(base))).toContain("geodb-feeder-subspaces-keyed-v1-testnet")
	})
	it("the backfill gate refuses once the consumer has offsets, unless told to reset them", () => {
		const dir = subspacesOffsetsDir(tmp())
		expect(keyedBackfillGate(dir, false)).toBe("fresh")
		new ProposalOffsets(dir).markDone(0, 4)
		expect(() => keyedBackfillGate(dir, false)).toThrow(/BACKFILL_KEYED_RESET_CONSUMER/)
		expect(keyedBackfillGate(dir, true)).toBe("reset")
	})
})
