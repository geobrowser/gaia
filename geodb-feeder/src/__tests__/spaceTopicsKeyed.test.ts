import {describe, expect, it} from "vitest"
import {feederGroupIds, loadConfig} from "../config.js"
import {cid} from "../ids.js"
import {keyedBackfillGate} from "../membershipKeyed.js"
import {createMetrics} from "../metrics.js"
import {ProposalOffsets} from "../proposals.js"
import {idPart, recordEntityId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {spaceTopicsBatchHandler} from "../spaceTopicsConsumer.js"
import {
	backfillSpaceTopics,
	handleSpaceTopicMessage,
	type KeyedSpaceTopicsDeps,
	parseTopicEvent,
	type SpaceTopicRow,
	spaceTopicFromRow,
	spaceTopicIntents,
	spaceTopicRecordId,
	spaceTopicsKeyedEnabled,
	spaceTopicsOffsetsDir,
} from "../spaceTopicsKeyed.js"
import {SystemSpaceCapError, systemSpaceNonce} from "../systemSpaces.js"
import {deadLetters, fixture, ModelStore, tmp} from "./keyedHarness.js"

const KIND = RECORD_KINDS.spaceTopic.id
const COL = RECORD_COLUMNS.spaceTopic
const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const SPACE2 = "b29c345ab9866679b001d7d2138d88a2"
const T1 = "c1111111111111111111111111111111"
const T2 = "c2222222222222222222222222222222"

const sfixture = (o: Parameters<typeof fixture>[0] = {}) => fixture({...o, offsetsDir: spaceTopicsOffsetsDir})

/** HermesTopicDeclared / HermesTopicRemoved bytes: space_id (1) and topic_id (2). */
function event(space: string | null, topic: string | null): Uint8Array {
	const bytes = (n: number, hex: string) => {
		const b = Buffer.from(hex, "hex")
		return Buffer.concat([Buffer.from([(n << 3) | 2, b.length]), b])
	}
	return Buffer.concat([...(space === null ? [] : [bytes(1, space)]), ...(topic === null ? [] : [bytes(2, topic)])])
}

const msg = (value: Uint8Array | null, offset = 0, eventType: string | null = "TOPIC_DECLARED") => ({
	offset: String(offset),
	value,
	headers: eventType === null ? {} : {"event-type": Buffer.from(eventType)},
})
const declare = (space: string, topic: string, offset = 0) => msg(event(space, topic), offset)
const remove = (space: string, topic: string, offset = 0) => msg(event(space, topic), offset, "TOPIC_REMOVED")
const ref = (hex: string) => `.{ .reference = "${cid("entity", hex)}" }`

describe("space topic record key", () => {
	it("is recordEntityId(spaceTopic, [space]) in the governance family", () => {
		expect(spaceTopicRecordId(SPACE)).toBe(recordEntityId(KIND, [idPart(SPACE)]))
		expect(spaceTopicRecordId(SPACE)).not.toBe(spaceTopicRecordId(SPACE2))
		expect(RECORD_KINDS.spaceTopic.family).toBe("governance")
	})
})

describe("spaceTopicIntents", () => {
	it("a declare creates the record, keys the Space once and puts the topic", () => {
		const intents = spaceTopicIntents({kind: "declared", space: SPACE, topic: T1})
		expect(intents[0]).toBe(`.{ .create_entity = "${spaceTopicRecordId(SPACE)}" }`)
		expect(intents.filter((i) => i.includes(".add = ")).length).toBe(1)
		expect(intents.filter((i) => i.includes(".put = ")).length).toBe(1)
		expect(intents.join("\n")).toContain(`.property = "${COL.topic}", .value = ${ref(T1)}`)
	})

	it("a removal only closes the statement of that topic", () => {
		const i = spaceTopicIntents({kind: "removed", space: SPACE, topic: T1})
		expect(i).toHaveLength(1)
		expect(i[0]).toContain(".set_active = ")
		expect(i[0]).toContain(".active = false")
	})
})

describe("parseTopicEvent", () => {
	it("decodes both events", () => {
		expect(parseTopicEvent("declared", event(SPACE, T1))).toEqual({kind: "declared", space: SPACE, topic: T1})
		expect(parseTopicEvent("removed", event(SPACE, T1))).toEqual({kind: "removed", space: SPACE, topic: T1})
	})
	it("refuses an id that is missing or not 16 bytes", () => {
		expect(() => parseTopicEvent("declared", event(null, T1))).toThrow(/space_id/)
		expect(() => parseTopicEvent("declared", event(SPACE, "abcd"))).toThrow(/topic_id/)
	})
})

describe("replace and clear (put, set_active)", () => {
	const run = async (list: ReturnType<typeof declare>[]) => {
		const model = new ModelStore()
		const f = sfixture({model})
		for (const m of list) expect(await handleSpaceTopicMessage(f.deps, 0, m)).toBe("applied")
		return {model, f}
	}
	const rec = spaceTopicRecordId(SPACE)

	it("a new topic replaces the old: one active value, the last", async () => {
		const {model} = await run([declare(SPACE, T1, 0), declare(SPACE, T2, 1)])
		expect(model.cell(rec, COL.topic)).toEqual([ref(T2)])
		expect(model.cell(rec, COL.space)).toEqual([`.{ .reference = "${cid("entity", SPACE)}" }`])
	})

	it("A -> B -> A ends on A", async () => {
		const {model} = await run([declare(SPACE, T1, 0), declare(SPACE, T2, 1), declare(SPACE, T1, 2)])
		expect(model.cell(rec, COL.topic)).toEqual([ref(T1)])
	})

	it("removing the current topic clears it", async () => {
		const {model} = await run([declare(SPACE, T1, 0), remove(SPACE, T1, 1)])
		expect(model.cell(rec, COL.topic)).toEqual([])
	})

	it("removing a topic that is not current changes nothing (kg-indexer's guard)", async () => {
		const {model} = await run([declare(SPACE, T1, 0), declare(SPACE, T2, 1), remove(SPACE, T1, 2)])
		expect(model.cell(rec, COL.topic)).toEqual([ref(T2)])
	})

	it("a replay converges on the same state", async () => {
		const stream = [declare(SPACE, T1, 0), declare(SPACE, T2, 1), remove(SPACE, T2, 2), declare(SPACE2, T1, 3)]
		const {model, f} = await run(stream)
		const before = model.snapshot()
		for (const m of stream) await handleSpaceTopicMessage(f.deps, 0, m)
		expect(model.snapshot()).toBe(before)
	})

	it("a backfill row and the declare that wrote it send the same intents", () => {
		const row = {space_id: `${SPACE.slice(0, 8)}-${SPACE.slice(8, 12)}-${SPACE.slice(12, 16)}-${SPACE.slice(16, 20)}-${SPACE.slice(20)}`, topic_id: T1}
		expect(spaceTopicIntents(spaceTopicFromRow(row))).toEqual(spaceTopicIntents(parseTopicEvent("declared", event(SPACE, T1))))
	})
})

describe("handleSpaceTopicMessage", () => {
	it("founds the governance Space once and writes there", async () => {
		const f = sfixture()
		expect(await handleSpaceTopicMessage(f.deps, 0, declare(SPACE, T1, 0))).toBe("applied")
		expect(await handleSpaceTopicMessage(f.deps, 1, declare(SPACE2, T1, 1))).toBe("applied")
		expect(f.founds).toEqual([systemSpaceNonce("governance")])
	})

	it("reads a missing event-type header as declared", async () => {
		const model = new ModelStore()
		const f = sfixture({model})
		expect(await handleSpaceTopicMessage(f.deps, 0, msg(event(SPACE, T1), 0, null))).toBe("applied")
		expect(model.cell(spaceTopicRecordId(SPACE), COL.topic)).toEqual([ref(T1)])
	})

	it("ignores other event types and a null value", async () => {
		const f = sfixture()
		expect(await handleSpaceTopicMessage(f.deps, 0, msg(event(SPACE, T1), 0, "SPACE_CREATED"))).toBe("ignored")
		expect(await handleSpaceTopicMessage(f.deps, 0, msg(null))).toBe("ignored")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toEqual([])
	})

	it("dead-letters what kg-indexer rejects, writing nothing", async () => {
		const f = sfixture()
		expect(await handleSpaceTopicMessage(f.deps, 0, msg(event(SPACE, "ab"), 7))).toBe("dead_letter")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toHaveLength(1)
	})
})

describe("stall, never skip", () => {
	it("a ResourceExhausted founding throws and dead-letters nothing", async () => {
		const f = sfixture({foundFailsOnce: "error: ResourceExhausted (space cap)"})
		await expect(handleSpaceTopicMessage(f.deps, 0, declare(SPACE, T1))).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(deadLetters(f.store)).toEqual([])
		expect(await handleSpaceTopicMessage(f.deps, 0, declare(SPACE, T1))).toBe("applied")
	})

	it("refuses to send put while not allowed", async () => {
		const f = sfixture()
		await expect(handleSpaceTopicMessage({...f.deps, allowPut: false}, 0, declare(SPACE, T1))).rejects.toThrow(/GEODB_RECORD_KEYS is off/)
	})

	it("the batch loop retries in place and counts outcomes on its own series", async () => {
		const done = new Map<number, number>()
		const deps = {
			metrics: createMetrics(),
			offsets: {hadOffsets: false, lastDone: (p: number) => done.get(p) ?? null, markDone: (p: number, o: number) => void done.set(p, o)},
		} as unknown as KeyedSpaceTopicsDeps
		const tries: string[] = []
		let failures = 1
		const run = spaceTopicsBatchHandler({
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
		expect(deps.metrics.spaceTopicsEvents.values).toEqual(new Map([['outcome="applied"', 1], ['outcome="ignored"', 1]]))
	})
})

describe("backfillSpaceTopics", () => {
	it("writes each row as its record", async () => {
		const model = new ModelStore()
		const f = sfixture({model})
		async function* rows() {
			yield {space_id: SPACE, topic_id: T1}
			yield {space_id: SPACE2, topic_id: T2}
		}
		expect(await backfillSpaceTopics(f.deps, rows())).toEqual({rows: 2, applied: 2, deadLetter: 0})
		expect(model.cell(spaceTopicRecordId(SPACE2), COL.topic)).toEqual([ref(T2)])
	})

	it("dead-letters a row no event could have written", async () => {
		const f = sfixture()
		const bad = [{space_id: "xyz", topic_id: T1}, {space_id: SPACE, topic_id: ""}] as SpaceTopicRow[]
		expect(await backfillSpaceTopics(f.deps, [...bad, {space_id: SPACE, topic_id: T1}])).toEqual({rows: 3, applied: 1, deadLetter: 2})
	})
})

describe("flag and gate", () => {
	const base = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}
	it("is off by default and needs GEODB_RECORD_KEYS", () => {
		const off = loadConfig(base)
		expect(off.spaceTopics).toEqual({enabled: false, topic: "testnet.space.topics", groupId: "geodb-feeder-space-topics-keyed-v1-testnet"})
		expect(spaceTopicsKeyedEnabled(off)).toBe(false)
		expect(spaceTopicsKeyedEnabled(loadConfig({...base, GEODB_SPACE_TOPICS: "1"}))).toBe(false)
		expect(spaceTopicsKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_SPACE_TOPICS: "1"}))).toBe(true)
	})

	it("has its own group and is in the backfill group check", () => {
		const c = loadConfig(base)
		expect(c.spaceTopics.groupId).not.toBe(c.spaceVotingSettings.groupId)
		expect(feederGroupIds(c)).toContain(c.spaceTopics.groupId)
	})

	it("the backfill gate refuses once the consumer has offsets, unless told to reset them", () => {
		const dir = spaceTopicsOffsetsDir(tmp())
		expect(keyedBackfillGate(dir, false)).toBe("fresh")
		new ProposalOffsets(dir).markDone(0, 4)
		expect(() => keyedBackfillGate(dir, false)).toThrow(/BACKFILL_KEYED_RESET_CONSUMER/)
		expect(keyedBackfillGate(dir, true)).toBe("reset")
	})
})
