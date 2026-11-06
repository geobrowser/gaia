import {describe, expect, it} from "vitest"
import {feederGroupIds, loadConfig} from "../config.js"
import {cid} from "../ids.js"
import {keyedBackfillGate} from "../membershipKeyed.js"
import {createMetrics} from "../metrics.js"
import {ProposalOffsets} from "../proposals.js"
import {idPart, recordEntityId, recordStatementId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {spaceVotingSettingsBatchHandler} from "../spaceVotingSettingsConsumer.js"
import {
	backfillSpaceVotingSettings,
	handleSpaceVotingSettingsMessage,
	type KeyedSpaceVotingSettingsDeps,
	parseVotingSettingsUpdated,
	type SpaceVotingSettings,
	type SpaceVotingSettingsRow,
	spaceVotingSettingsFromRow,
	spaceVotingSettingsIntents,
	spaceVotingSettingsKeyedEnabled,
	spaceVotingSettingsOffsetsDir,
	spaceVotingSettingsRecordId,
} from "../spaceVotingSettingsKeyed.js"
import {SystemSpaceCapError, systemSpaceNonce} from "../systemSpaces.js"
import {deadLetters, fixture, ModelStore, tmp} from "./keyedHarness.js"

const KIND = RECORD_KINDS.spaceVotingSettings.id
const COL = RECORD_COLUMNS.spaceVotingSettings
const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const SPACE2 = "b29c345ab9866679b001d7d2138d88a2"

const sfixture = (o: Parameters<typeof fixture>[0] = {}) => fixture({...o, offsetsDir: spaceVotingSettingsOffsetsDir})

const settings = (o: Partial<SpaceVotingSettings> = {}): SpaceVotingSettings => ({
	space: SPACE,
	partial: 500_000n,
	universal: 750_000n,
	flat: 3n,
	quorum: 2n,
	duration: 86_400n,
	disableFastPath: false,
	grace: 3_600n,
	updatedAtSeconds: 1_700_000_000n,
	updatedAtBlock: 123n,
	...o,
})

/** HermesVotingSettingsUpdated bytes (hermes-schema/proto/governance.proto). */
function updated(s: SpaceVotingSettings, o: {space?: string | null; meta?: boolean} = {}): Uint8Array {
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
	const sp = o.space === undefined ? s.space : o.space
	if (sp !== null) parts.push(bytes(1, Buffer.from(sp, "hex")))
	parts.push(num(2, s.partial), num(3, s.universal), num(4, s.flat), num(5, s.quorum), num(6, s.duration))
	parts.push(num(7, s.disableFastPath ? 1n : 0n), num(8, s.grace))
	if (o.meta !== false) parts.push(bytes(9, Buffer.concat([num(1, s.updatedAtSeconds), num(3, s.updatedAtBlock)])))
	return Buffer.concat(parts)
}

const msg = (value: Uint8Array, offset = 0, eventType: string | null = "VOTING_SETTINGS_UPDATED") => ({
	offset: String(offset),
	value,
	headers: eventType === null ? {} : {"event-type": Buffer.from(eventType)},
})

const rowOf = (s: SpaceVotingSettings): SpaceVotingSettingsRow => ({
	space_id: `${s.space.slice(0, 8)}-${s.space.slice(8, 12)}-${s.space.slice(12, 16)}-${s.space.slice(16, 20)}-${s.space.slice(20)}`,
	partial_percentage_support_threshold: String(s.partial),
	universal_percentage_support_threshold: String(s.universal),
	flat_support_threshold: String(s.flat),
	quorum: String(s.quorum),
	duration: String(s.duration),
	disable_fast_path_access_for_new_members: s.disableFastPath,
	execution_grace_period: String(s.grace),
	updated_at: String(s.updatedAtSeconds),
	updated_at_block: String(s.updatedAtBlock),
})

describe("space voting settings record key", () => {
	it("is recordEntityId(spaceVotingSettings, [space]): the table's primary key, and the key alone", () => {
		expect(spaceVotingSettingsRecordId(SPACE)).toBe(recordEntityId(KIND, [idPart(SPACE)]))
		expect(spaceVotingSettingsRecordId(SPACE)).not.toBe(spaceVotingSettingsRecordId(SPACE2))
		expect(RECORD_KINDS.spaceVotingSettings.family).toBe("governance")
	})
})

describe("spaceVotingSettingsIntents", () => {
	it("creates the record, keys the Space once and puts the nine mutable columns", () => {
		const intents = spaceVotingSettingsIntents(settings())
		const record = spaceVotingSettingsRecordId(SPACE)
		expect(intents[0]).toBe(`.{ .create_entity = "${record}" }`)
		expect(intents.filter((i) => i.includes(".add = ")).length).toBe(1)
		expect(intents.filter((i) => i.includes(".set_active = ")).length).toBe(1)
		expect(intents.filter((i) => i.includes(".put = ")).length).toBe(9)
		const all = intents.join("\n")
		const t = cid("entity", SPACE)
		const sid = recordStatementId(KIND, [idPart(record), idPart(COL.space), idPart(t)])
		expect(all).toContain(`.add = .{ .id = "${sid}", .entity = "${record}", .property = "${COL.space}", .value = .{ .reference = "${t}" } }`)
		expect(all).toContain(`.property = "${COL.partial}", .value = .{ .number = 500000 }`)
		expect(all).toContain(`.property = "${COL.disableFastPath}", .value = .{ .boolean = false }`)
		expect(all).toContain(`.property = "${COL.updatedAt}", .value = .{ .instant = "1700000000000000" }`)
		expect(all).toContain(`.property = "${COL.updatedAtBlock}", .value = .{ .number = 123 }`)
	})
})

describe("parseVotingSettingsUpdated (kg-indexer's handle_voting_settings_updated)", () => {
	it("decodes an event", () => {
		expect(parseVotingSettingsUpdated(updated(settings({disableFastPath: true})))).toEqual(settings({disableFastPath: true}))
	})

	it("applies the as-i64 cast to a u64 above i64::MAX", () => {
		const big = 2n ** 63n + 5n
		expect(parseVotingSettingsUpdated(updated(settings({partial: big}))).partial).toBe(BigInt.asIntN(64, big))
	})

	it("reads missing metadata as time 0, block 0", () => {
		expect(parseVotingSettingsUpdated(updated(settings(), {meta: false}))).toEqual(settings({updatedAtSeconds: 0n, updatedAtBlock: 0n}))
	})

	it("refuses a space_id that is not 16 bytes, and a wrong wire type", () => {
		expect(() => parseVotingSettingsUpdated(updated(settings(), {space: null}))).toThrow(/space_id/)
		expect(() => parseVotingSettingsUpdated(updated(settings(), {space: "abcd"}))).toThrow(/space_id/)
		const bad = Buffer.concat([updated(settings()), Buffer.from([(5 << 3) | 2, 1, 1])])
		expect(() => parseVotingSettingsUpdated(bad)).toThrow(/quorum/)
	})
})

describe("replace on change (put)", () => {
	const run = async (list: SpaceVotingSettings[]) => {
		const model = new ModelStore()
		const f = sfixture({model})
		for (const [i, s] of list.entries()) expect(await handleSpaceVotingSettingsMessage(f.deps, 0, msg(updated(s), i))).toBe("applied")
		return {model, f}
	}

	it("a changed setting leaves one active value per column: the last", async () => {
		const {model} = await run([settings(), settings({quorum: 9n, disableFastPath: true, updatedAtSeconds: 20n, updatedAtBlock: 2n})])
		const record = spaceVotingSettingsRecordId(SPACE)
		expect(model.cell(record, COL.quorum)).toEqual([".{ .number = 9 }"])
		expect(model.cell(record, COL.disableFastPath)).toEqual([".{ .boolean = true }"])
		expect(model.cell(record, COL.updatedAt)).toEqual(['.{ .instant = "20000000" }'])
		expect(model.cell(record, COL.space)).toEqual([`.{ .reference = "${cid("entity", SPACE)}" }`])
	})

	it("A -> B -> A ends on A", async () => {
		const {model} = await run([settings({disableFastPath: false}), settings({disableFastPath: true}), settings({disableFastPath: false})])
		expect(model.cell(spaceVotingSettingsRecordId(SPACE), COL.disableFastPath)).toEqual([".{ .boolean = false }"])
	})

	it("two Spaces are two records", async () => {
		const {model} = await run([settings(), settings({space: SPACE2, quorum: 7n})])
		expect(model.entities.size).toBe(2)
		expect(model.cell(spaceVotingSettingsRecordId(SPACE2), COL.quorum)).toEqual([".{ .number = 7 }"])
	})

	it("a replay converges on the same state", async () => {
		const stream = [settings(), settings({quorum: 4n}), settings({space: SPACE2})]
		const {model, f} = await run(stream)
		const before = model.snapshot()
		for (const [i, s] of stream.entries()) await handleSpaceVotingSettingsMessage(f.deps, 0, msg(updated(s), i))
		expect(model.snapshot()).toBe(before)
	})

	it("a backfill row and the event that wrote it send the same intents", () => {
		for (const s of [settings(), settings({disableFastPath: true, updatedAtSeconds: 0n, updatedAtBlock: 0n}), settings({flat: -1n})]) {
			expect(spaceVotingSettingsIntents(spaceVotingSettingsFromRow(rowOf(s)))).toEqual(spaceVotingSettingsIntents(parseVotingSettingsUpdated(updated(s))))
		}
	})
})

describe("handleSpaceVotingSettingsMessage", () => {
	it("founds the governance Space once and writes there", async () => {
		const f = sfixture()
		expect(await handleSpaceVotingSettingsMessage(f.deps, 0, msg(updated(settings()), 0))).toBe("applied")
		expect(await handleSpaceVotingSettingsMessage(f.deps, 1, msg(updated(settings({space: SPACE2})), 1))).toBe("applied")
		expect(f.founds).toEqual([systemSpaceNonce("governance")])
	})

	it("ignores every other event type, no header and a null value", async () => {
		const f = sfixture()
		for (const t of ["PROPOSAL_CREATED", "PROPOSAL_VOTED", null]) {
			expect(await handleSpaceVotingSettingsMessage(f.deps, 0, msg(updated(settings()), 0, t))).toBe("ignored")
		}
		expect(await handleSpaceVotingSettingsMessage(f.deps, 0, {offset: "0", value: null, headers: {"event-type": "VOTING_SETTINGS_UPDATED"}})).toBe("ignored")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toEqual([])
	})

	it("dead-letters what kg-indexer rejects, writing nothing", async () => {
		const f = sfixture()
		expect(await handleSpaceVotingSettingsMessage(f.deps, 0, msg(updated(settings(), {space: "ab"}), 7))).toBe("dead_letter")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toHaveLength(1)
	})
})

describe("stall, never skip", () => {
	it("a ResourceExhausted founding throws and dead-letters nothing", async () => {
		const f = sfixture({foundFailsOnce: "error: ResourceExhausted (space cap)"})
		await expect(handleSpaceVotingSettingsMessage(f.deps, 0, msg(updated(settings()), 0))).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(deadLetters(f.store)).toEqual([])
		expect(await handleSpaceVotingSettingsMessage(f.deps, 0, msg(updated(settings()), 0))).toBe("applied")
	})

	it("refuses to send put while not allowed", async () => {
		const f = sfixture()
		await expect(handleSpaceVotingSettingsMessage({...f.deps, allowPut: false}, 0, msg(updated(settings()), 0))).rejects.toThrow(/GEODB_RECORD_KEYS is off/)
	})

	it("the batch loop retries in place and counts outcomes on its own series", async () => {
		const done = new Map<number, number>()
		const deps = {
			metrics: createMetrics(),
			offsets: {hadOffsets: false, lastDone: (p: number) => done.get(p) ?? null, markDone: (p: number, o: number) => void done.set(p, o)},
		} as unknown as KeyedSpaceVotingSettingsDeps
		const tries: string[] = []
		let failures = 1
		const run = spaceVotingSettingsBatchHandler({
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
		expect(deps.metrics.spaceVotingSettingsEvents.values).toEqual(new Map([['outcome="applied"', 1], ['outcome="ignored"', 1]]))
	})
})

describe("backfillSpaceVotingSettings", () => {
	it("writes each row as its record", async () => {
		const model = new ModelStore()
		const f = sfixture({model})
		async function* rows() {
			yield rowOf(settings())
			yield rowOf(settings({space: SPACE2, quorum: 7n}))
		}
		expect(await backfillSpaceVotingSettings(f.deps, rows())).toEqual({rows: 2, applied: 2, deadLetter: 0})
		expect(model.cell(spaceVotingSettingsRecordId(SPACE2), COL.quorum)).toEqual([".{ .number = 7 }"])
	})

	it("dead-letters a row no event could have written", async () => {
		const f = sfixture()
		const bad = [
			{...rowOf(settings()), quorum: "1.5"},
			{...rowOf(settings()), duration: "99999999999999999999"},
			{...rowOf(settings()), updated_at: "x"},
			{...rowOf(settings()), disable_fast_path_access_for_new_members: "t" as unknown as boolean},
		]
		expect(await backfillSpaceVotingSettings(f.deps, [...bad, rowOf(settings())])).toEqual({rows: 5, applied: 1, deadLetter: 4})
	})
})

describe("flag and gate", () => {
	const base = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}
	it("is off by default and needs GEODB_RECORD_KEYS", () => {
		const off = loadConfig(base)
		expect(off.spaceVotingSettings).toEqual({
			enabled: false,
			topic: "testnet.space.governance",
			groupId: "geodb-feeder-space-voting-settings-keyed-v1-testnet",
		})
		expect(spaceVotingSettingsKeyedEnabled(off)).toBe(false)
		expect(spaceVotingSettingsKeyedEnabled(loadConfig({...base, GEODB_SPACE_VOTING_SETTINGS: "1"}))).toBe(false)
		expect(spaceVotingSettingsKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_PROPOSAL_ACTIONS: "1"}))).toBe(false)
		expect(spaceVotingSettingsKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_SPACE_VOTING_SETTINGS: "1"}))).toBe(true)
	})

	it("has its own group and is in the backfill group check", () => {
		const c = loadConfig(base)
		expect(c.spaceVotingSettings.groupId).not.toBe(c.proposalActions.groupId)
		expect(feederGroupIds(c)).toContain(c.spaceVotingSettings.groupId)
	})

	it("the backfill gate refuses once the consumer has offsets, unless told to reset them", () => {
		const dir = spaceVotingSettingsOffsetsDir(tmp())
		expect(keyedBackfillGate(dir, false)).toBe("fresh")
		new ProposalOffsets(dir).markDone(0, 4)
		expect(() => keyedBackfillGate(dir, false)).toThrow(/BACKFILL_KEYED_RESET_CONSUMER/)
		expect(keyedBackfillGate(dir, true)).toBe("reset")
	})
})
