import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {feederGroupIds, loadConfig} from "../config.js"
import {cid} from "../ids.js"
import {keyedBackfillGate} from "../membershipKeyed.js"
import {createMetrics} from "../metrics.js"
import {ProposalOffsets} from "../proposals.js"
import {idPart, recordEntityId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {spaceAddressesBatchHandler} from "../spaceAddressesConsumer.js"
import {gateSpaceAddresses} from "../spaceAddressesGate.js"
import {
	backfillSpaceAddresses,
	checksumAddress,
	handleSpaceAddressMessage,
	type KeyedSpaceAddressesDeps,
	parseAddressUpdated,
	parseCreateSpaceAddress,
	type SpaceAddressRow,
	spaceAddressFromRow,
	spaceAddressIntents,
	spaceAddressRecordId,
	spaceAddressesKeyedEnabled,
	spaceAddressesOffsetsDir,
} from "../spaceAddressesKeyed.js"
import {SystemSpaceCapError, systemSpaceNonce} from "../systemSpaces.js"
import {deadLetters, fixture, ModelStore, tmp} from "./keyedHarness.js"

const KIND = RECORD_KINDS.spaceAddress.id
const COL = RECORD_COLUMNS.spaceAddress
const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const SPACE2 = "b29c345ab9866679b001d7d2138d88a2"
// EIP-55 reference vectors (the spec's own).
const LOWER1 = "5aaeb6053f3e94c9b9a09f33669435e7ef1beaed"
const SUM1 = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
const LOWER2 = "fb6916095ca1df60bb79ce92ce3ea74c37c5d359"
const SUM2 = "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359"

const sfixture = (o: Parameters<typeof fixture>[0] = {}) => fixture({...o, offsetsDir: spaceAddressesOffsetsDir})

const field = (n: number, b: Buffer) => Buffer.concat([Buffer.from([(n << 3) | 2, b.length]), b])
const hex = (h: string) => Buffer.from(h, "hex")

/** HermesSpaceAddressUpdated: space_id (1), address (2). */
const updated = (space: string, address: string) => Buffer.concat([field(1, hex(space)), field(2, hex(address))])
/** HermesCreateSpace: space_id (1), eoa_space (3 {owner 1}) or default_dao_space (4 {address 1}), meta (5, empty). */
const created = (space: string, address: string, payload: 3 | 4 | null = 3, meta = true) =>
	Buffer.concat([field(1, hex(space)), ...(payload === null ? [] : [field(payload, field(1, hex(address)))]), ...(meta ? [field(5, Buffer.from([0x18, 1]))] : [])])

const msg = (value: Uint8Array | null, offset = 0, eventType: string | null = "SPACE_ADDRESS_UPDATED") => ({
	offset: String(offset),
	value,
	headers: eventType === null ? {} : {"event-type": Buffer.from(eventType)},
})
const upd = (space: string, address: string, offset = 0) => msg(updated(space, address), offset)
const crt = (space: string, address: string, offset = 0) => msg(created(space, address), offset, null)
const text = (a: string) => `.{ .text = "${a}" }`

describe("checksumAddress (indexer_utils::checksum_address)", () => {
	it("matches the EIP-55 vectors, from either case and with a 0x prefix", () => {
		expect(checksumAddress(LOWER1)).toBe(SUM1)
		expect(checksumAddress(LOWER2)).toBe(SUM2)
		expect(checksumAddress(`0x${LOWER1.toUpperCase()}`)).toBe(SUM1)
	})
})

describe("space address record key", () => {
	it("is recordEntityId(spaceAddress, [space]) in the governance family", () => {
		expect(spaceAddressRecordId(SPACE)).toBe(recordEntityId(KIND, [idPart(SPACE)]))
		expect(spaceAddressRecordId(SPACE)).not.toBe(spaceAddressRecordId(SPACE2))
		expect(RECORD_KINDS.spaceAddress.family).toBe("governance")
	})
})

describe("decoding", () => {
	it("an update is the lowercase 0x address, as update_space_address writes it", () => {
		expect(parseAddressUpdated(updated(SPACE, LOWER1))).toEqual({space: SPACE, address: `0x${LOWER1}`})
	})
	it("a create is the checksummed address, from an EOA owner or a DAO address", () => {
		expect(parseCreateSpaceAddress(created(SPACE, LOWER1, 3))).toEqual({space: SPACE, address: SUM1})
		expect(parseCreateSpaceAddress(created(SPACE, LOWER2, 4))).toEqual({space: SPACE, address: SUM2})
	})
	it("refuses what kg-indexer errors on", () => {
		expect(() => parseAddressUpdated(updated("abcd", LOWER1))).toThrow(/space_id/)
		expect(() => parseAddressUpdated(updated(SPACE, "abcd"))).toThrow(/20 bytes/)
		expect(() => parseCreateSpaceAddress(created(SPACE, LOWER1, null))).toThrow(/payload/)
		expect(() => parseCreateSpaceAddress(created(SPACE, "abcd"))).toThrow(/20 bytes/)
	})
})

describe("spaceAddressIntents", () => {
	it("creates the record, keys the Space once and puts the address text", () => {
		const intents = spaceAddressIntents({space: SPACE, address: SUM1})
		expect(intents[0]).toBe(`.{ .create_entity = "${spaceAddressRecordId(SPACE)}" }`)
		expect(intents.filter((i) => i.includes(".add = ")).length).toBe(1)
		expect(intents.filter((i) => i.includes(".put = ")).length).toBe(1)
		expect(intents.join("\n")).toContain(`.property = "${COL.address}", .value = ${text(SUM1)}`)
	})
})

describe("replace (put)", () => {
	const rec = spaceAddressRecordId(SPACE)
	const run = async (list: ReturnType<typeof upd>[]) => {
		const model = new ModelStore()
		const f = sfixture({model})
		for (const m of list) expect(await handleSpaceAddressMessage(f.deps, 0, m)).toBe("applied")
		return {model, f}
	}

	it("a create then an update leaves the updated address", async () => {
		const {model} = await run([crt(SPACE, LOWER1, 0), upd(SPACE, LOWER2, 1)])
		expect(model.cell(rec, COL.address)).toEqual([text(`0x${LOWER2}`)])
		expect(model.cell(rec, COL.space)).toEqual([`.{ .reference = "${cid("entity", SPACE)}" }`])
	})

	it("a create alone is the checksummed address", async () => {
		const {model} = await run([crt(SPACE, LOWER1)])
		expect(model.cell(rec, COL.address)).toEqual([text(SUM1)])
	})

	it("A -> B -> A ends on A", async () => {
		const {model} = await run([upd(SPACE, LOWER1, 0), upd(SPACE, LOWER2, 1), upd(SPACE, LOWER1, 2)])
		expect(model.cell(rec, COL.address)).toEqual([text(`0x${LOWER1}`)])
	})

	it("a replay converges on the same state", async () => {
		const stream = [crt(SPACE, LOWER1, 0), upd(SPACE, LOWER2, 1), upd(SPACE2, LOWER1, 2)]
		const {model, f} = await run(stream)
		const before = model.snapshot()
		for (const m of stream) await handleSpaceAddressMessage(f.deps, 0, m)
		expect(model.snapshot()).toBe(before)
	})

	it("a backfill row and the event that wrote it send the same intents", () => {
		const row = {space_id: `${SPACE.slice(0, 8)}-${SPACE.slice(8, 12)}-${SPACE.slice(12, 16)}-${SPACE.slice(16, 20)}-${SPACE.slice(20)}`, address: SUM1}
		expect(spaceAddressIntents(spaceAddressFromRow(row))).toEqual(spaceAddressIntents(parseCreateSpaceAddress(created(SPACE, LOWER1))))
	})
})

describe("handleSpaceAddressMessage", () => {
	it("founds the governance Space once and writes there", async () => {
		const f = sfixture()
		expect(await handleSpaceAddressMessage(f.deps, 0, upd(SPACE, LOWER1, 0))).toBe("applied")
		expect(await handleSpaceAddressMessage(f.deps, 1, crt(SPACE2, LOWER2, 1))).toBe("applied")
		expect(f.founds).toEqual([systemSpaceNonce("governance")])
	})

	it("ignores other event types and a null value", async () => {
		const f = sfixture()
		expect(await handleSpaceAddressMessage(f.deps, 0, msg(updated(SPACE, LOWER1), 0, "SPACE_CREATED"))).toBe("ignored")
		expect(await handleSpaceAddressMessage(f.deps, 0, msg(null))).toBe("ignored")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toEqual([])
	})

	it("dead-letters what kg-indexer rejects, writing nothing", async () => {
		const f = sfixture()
		expect(await handleSpaceAddressMessage(f.deps, 0, msg(updated(SPACE, "abcd"), 7))).toBe("dead_letter")
		expect(await handleSpaceAddressMessage(f.deps, 0, msg(created(SPACE, LOWER1, null), 8, null))).toBe("dead_letter")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toHaveLength(2)
	})
})

describe("stall, never skip", () => {
	it("a ResourceExhausted founding throws and dead-letters nothing", async () => {
		const f = sfixture({foundFailsOnce: "error: ResourceExhausted (space cap)"})
		await expect(handleSpaceAddressMessage(f.deps, 0, upd(SPACE, LOWER1))).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(deadLetters(f.store)).toEqual([])
		expect(await handleSpaceAddressMessage(f.deps, 0, upd(SPACE, LOWER1))).toBe("applied")
	})

	it("refuses to send put while not allowed", async () => {
		const f = sfixture()
		await expect(handleSpaceAddressMessage({...f.deps, allowPut: false}, 0, upd(SPACE, LOWER1))).rejects.toThrow(/GEODB_RECORD_KEYS is off/)
	})

	it("the batch loop retries in place and counts outcomes on its own series", async () => {
		const done = new Map<number, number>()
		const deps = {
			metrics: createMetrics(),
			offsets: {hadOffsets: false, lastDone: (p: number) => done.get(p) ?? null, markDone: (p: number, o: number) => void done.set(p, o)},
		} as unknown as KeyedSpaceAddressesDeps
		const tries: string[] = []
		let failures = 1
		const run = spaceAddressesBatchHandler({
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
		expect(deps.metrics.spaceAddressesEvents.values).toEqual(new Map([['outcome="applied"', 1], ['outcome="ignored"', 1]]))
	})
})

describe("backfillSpaceAddresses", () => {
	it("writes each row as stored, keeping its case", async () => {
		const model = new ModelStore()
		const f = sfixture({model})
		async function* rows() {
			yield {space_id: SPACE, address: SUM1}
			yield {space_id: SPACE2, address: `0x${LOWER2}`}
		}
		expect(await backfillSpaceAddresses(f.deps, rows())).toEqual({rows: 2, applied: 2, deadLetter: 0})
		expect(model.cell(spaceAddressRecordId(SPACE), COL.address)).toEqual([text(SUM1)])
		expect(model.cell(spaceAddressRecordId(SPACE2), COL.address)).toEqual([text(`0x${LOWER2}`)])
	})

	it("dead-letters a row no event could have written", async () => {
		const f = sfixture()
		const bad = [{space_id: "xyz", address: SUM1}, {space_id: SPACE, address: "0x12"}] as SpaceAddressRow[]
		expect(await backfillSpaceAddresses(f.deps, [...bad, {space_id: SPACE, address: SUM1}])).toEqual({rows: 3, applied: 1, deadLetter: 2})
	})
})

describe("flag", () => {
	const base = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}
	it("is off by default and needs GEODB_RECORD_KEYS", () => {
		const off = loadConfig(base)
		expect(off.spaceAddresses).toEqual({enabled: false, topic: "testnet.space.creations", groupId: "geodb-feeder-space-addresses-keyed-v1-testnet"})
		expect(spaceAddressesKeyedEnabled(off)).toBe(false)
		expect(spaceAddressesKeyedEnabled(loadConfig({...base, GEODB_SPACE_ADDRESSES: "1"}))).toBe(false)
		expect(spaceAddressesKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_SPACE_ADDRESSES: "1"}))).toBe(true)
	})

	it("has its own group and is in the backfill group check", () => {
		const c = loadConfig(base)
		expect(c.spaceAddresses.groupId).not.toBe(c.spaceEntities.groupId)
		expect(feederGroupIds(c)).toContain(c.spaceAddresses.groupId)
	})

	it("the backfill gate refuses once the consumer has offsets, unless told to reset them", () => {
		const dir = spaceAddressesOffsetsDir(tmp())
		expect(keyedBackfillGate(dir, false)).toBe("fresh")
		new ProposalOffsets(dir).markDone(0, 4)
		expect(() => keyedBackfillGate(dir, false)).toThrow(/BACKFILL_KEYED_RESET_CONSUMER/)
		expect(keyedBackfillGate(dir, true)).toBe("reset")
	})
})

describe("space addresses gate", () => {
	function setup(geoAddress: string | null, extraGeo = false, pgAddress = SUM1) {
		const dir = mkdtempSync(join(tmpdir(), "sa-gate-"))
		const rec = spaceAddressRecordId(SPACE)
		const space = [`${rec}\t${cid("entity", SPACE)}\t1`]
		if (extraGeo) space.push(`${"a".repeat(32)}\t${cid("entity", SPACE2)}\t1`)
		writeFileSync(join(dir, "sa-space.tsv"), `${space.join("\n")}\n`)
		writeFileSync(join(dir, "sa-address.tsv"), geoAddress === null ? "" : `${rec}\t${geoAddress}\t1\n`)
		writeFileSync(join(dir, "pg.tsv"), `${SPACE}\t${pgAddress}\n`)
		return dir
	}
	it("passes when Postgres and the records agree, case included", () => {
		const dir = setup(SUM1)
		const r = gateSpaceAddresses(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
		expect(r.pgChecksum).toBe(r.geoChecksum)
	})
	it("reports a case difference and a geo-db-only record", () => {
		const dir = setup(`0x${LOWER1}`, true)
		const r = gateSpaceAddresses(join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs.map((d) => d.column)).toEqual(["address"])
		expect(r.geoOnly).toEqual(["a".repeat(32)])
	})
	it("reports a cleared address and a Postgres address with no record", () => {
		const dir = setup(null)
		expect(gateSpaceAddresses(join(dir, "pg.tsv"), dir).valueDiffs.map((d) => [d.column, d.geo])).toEqual([["address", "<missing>"]])
		writeFileSync(join(dir, "pg.tsv"), `${SPACE}\t${SUM1}\n${"b".repeat(32)}\t${SUM1}\n`)
		expect(gateSpaceAddresses(join(dir, "pg.tsv"), dir).pgOnly).toHaveLength(1)
	})
})
