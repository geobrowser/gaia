import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {feederGroupIds, loadConfig} from "../config.js"
import {cid} from "../ids.js"
import {keyedBackfillGate} from "../membershipKeyed.js"
import {createMetrics} from "../metrics.js"
import {ProposalOffsets} from "../proposals.js"
import {proposalStatusBatchHandler} from "../proposalStatusConsumer.js"
import {gateProposalStatus, PG_COLUMNS} from "../proposalStatusGate.js"
import {
	backfillProposalStatus,
	handleProposalStatusMessage,
	type KeyedProposalStatusDeps,
	parseProposalExecuted,
	type ProposalStatusFacts,
	type ProposalStatusRow,
	proposalStatusFromRow,
	proposalStatusIntents,
	proposalStatusKeyedEnabled,
	proposalStatusOffsetsDir,
	proposalStatusRecordId,
} from "../proposalStatusKeyed.js"
import {idPart, recordEntityId, recordStatementId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import {SystemSpaceCapError, systemSpaceNonce} from "../systemSpaces.js"
import {deadLetters, fixture, ModelStore, tmp} from "./keyedHarness.js"

const KIND = RECORD_KINDS.proposalStatus.id
const COL = RECORD_COLUMNS.proposalStatus
const P1 = "a19c345ab9866679b001d7d2138d88a1"
const P2 = "b29c345ab9866679b001d7d2138d88a2"
const SPACE = "c39c345ab9866679b001d7d2138d88a3"

const pfixture = (o: Parameters<typeof fixture>[0] = {}) => fixture({...o, offsetsDir: proposalStatusOffsetsDir})

const facts = (o: Partial<ProposalStatusFacts> = {}): ProposalStatusFacts => ({proposal: P1, executedAt: 1_700_000_000n, unexecutableAt: null, ...o})

/** HermesProposalExecuted bytes (hermes-schema/proto/governance.proto). */
function executed(s: ProposalStatusFacts, o: {proposal?: string | null; meta?: boolean} = {}): Uint8Array {
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
	const parts: Buffer[] = [bytes(1, Buffer.from(SPACE, "hex"))]
	const p = o.proposal === undefined ? s.proposal : o.proposal
	if (p !== null) parts.push(bytes(2, Buffer.from(p, "hex")))
	if (o.meta !== false) parts.push(bytes(3, Buffer.concat([num(1, s.executedAt ?? 0n), num(3, 99n)])))
	return Buffer.concat(parts)
}

const msg = (value: Uint8Array, offset = 0, eventType: string | null = "PROPOSAL_EXECUTED") => ({
	offset: String(offset),
	value,
	headers: eventType === null ? {} : {"event-type": Buffer.from(eventType)},
})

const dashed = (h: string) => `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
const rowOf = (s: ProposalStatusFacts): ProposalStatusRow => ({
	id: dashed(s.proposal),
	executed_at: s.executedAt === null ? null : String(s.executedAt),
	unexecutable_at: s.unexecutableAt === null ? null : String(s.unexecutableAt),
})

describe("proposal status record key", () => {
	it("is recordEntityId(proposalStatus, [proposal]): the table's primary key, and the key alone", () => {
		expect(proposalStatusRecordId(P1)).toBe(recordEntityId(KIND, [idPart(P1)]))
		expect(proposalStatusRecordId(P1)).not.toBe(proposalStatusRecordId(P2))
		expect(RECORD_KINDS.proposalStatus.family).toBe("governance")
	})
})

describe("proposalStatusIntents", () => {
	it("creates the record, keys the proposal once and puts only the facts it has", () => {
		const record = proposalStatusRecordId(P1)
		const both = proposalStatusIntents(facts({unexecutableAt: 1_800_000_000n})).join("\n")
		const t = cid("entity", P1)
		const sid = recordStatementId(KIND, [idPart(record), idPart(COL.proposal), idPart(t)])
		expect(both).toContain(`.create_entity = "${record}"`)
		expect(both).toContain(`.add = .{ .id = "${sid}", .entity = "${record}", .property = "${COL.proposal}", .value = .{ .reference = "${t}" } }`)
		expect(both).toContain(`.property = "${COL.executedAt}", .value = .{ .instant = "1700000000000000" }`)
		expect(both).toContain(`.property = "${COL.unexecutableAt}", .value = .{ .instant = "1800000000000000" }`)
		const one = proposalStatusIntents(facts()).filter((i) => i.includes(".put = "))
		expect(one).toHaveLength(1)
		expect(one[0]).toContain(COL.executedAt)
	})
})

describe("parseProposalExecuted (kg-indexer's handle_proposal_executed)", () => {
	it("decodes an event: the block time is executed_at, unexecutable_at is absent", () => {
		expect(parseProposalExecuted(executed(facts()))).toEqual(facts())
	})

	it("reads missing metadata as time 0", () => {
		expect(parseProposalExecuted(executed(facts(), {meta: false}))).toEqual(facts({executedAt: 0n}))
	})

	it("refuses a proposal_id that is not 16 bytes", () => {
		expect(() => parseProposalExecuted(executed(facts(), {proposal: null}))).toThrow(/proposal_id/)
		expect(() => parseProposalExecuted(executed(facts(), {proposal: "abcd"}))).toThrow(/proposal_id/)
	})
})

describe("replace on change (put)", () => {
	const run = async (list: ProposalStatusFacts[]) => {
		const model = new ModelStore()
		const f = pfixture({model})
		for (const [i, s] of list.entries()) expect(await handleProposalStatusMessage(f.deps, 0, msg(executed(s), i))).toBe("applied")
		return {model, f}
	}

	it("a second execution replaces the first: one active value", async () => {
		const {model} = await run([facts(), facts({executedAt: 1_700_000_500n})])
		expect(model.cell(proposalStatusRecordId(P1), COL.executedAt)).toEqual(['.{ .instant = "1700000500000000" }'])
		expect(model.cell(proposalStatusRecordId(P1), COL.proposal)).toEqual([`.{ .reference = "${cid("entity", P1)}" }`])
	})

	it("A -> B -> A ends on A", async () => {
		const {model} = await run([facts({executedAt: 5n}), facts({executedAt: 6n}), facts({executedAt: 5n})])
		expect(model.cell(proposalStatusRecordId(P1), COL.executedAt)).toEqual(['.{ .instant = "5000000" }'])
	})

	it("an event never clears an unexecutable_at the backfill wrote", async () => {
		const model = new ModelStore()
		const f = pfixture({model})
		await backfillProposalStatus(f.deps, [rowOf(facts({executedAt: null, unexecutableAt: 77n}))])
		await handleProposalStatusMessage(f.deps, 0, msg(executed(facts()), 0))
		expect(model.cell(proposalStatusRecordId(P1), COL.unexecutableAt)).toEqual(['.{ .instant = "77000000" }'])
		expect(model.cell(proposalStatusRecordId(P1), COL.executedAt)).toEqual(['.{ .instant = "1700000000000000" }'])
	})

	it("two proposals are two records, and a replay converges on the same state", async () => {
		const stream = [facts(), facts({executedAt: 9n}), facts({proposal: P2})]
		const {model, f} = await run(stream)
		expect(model.entities.size).toBe(2)
		const before = model.snapshot()
		for (const [i, s] of stream.entries()) await handleProposalStatusMessage(f.deps, 0, msg(executed(s), i))
		expect(model.snapshot()).toBe(before)
	})

	it("a backfill row and the event that wrote it send the same intents", () => {
		for (const s of [facts(), facts({executedAt: 0n})]) {
			expect(proposalStatusIntents(proposalStatusFromRow(rowOf(s)))).toEqual(proposalStatusIntents(parseProposalExecuted(executed(s))))
		}
	})
})

describe("handleProposalStatusMessage", () => {
	it("founds the governance Space once and writes there", async () => {
		const f = pfixture()
		expect(await handleProposalStatusMessage(f.deps, 0, msg(executed(facts()), 0))).toBe("applied")
		expect(await handleProposalStatusMessage(f.deps, 1, msg(executed(facts({proposal: P2})), 1))).toBe("applied")
		expect(f.founds).toEqual([systemSpaceNonce("governance")])
	})

	it("ignores every other event type, no header and a null value", async () => {
		const f = pfixture()
		for (const t of ["PROPOSAL_CREATED", "VOTING_SETTINGS_UPDATED", null]) {
			expect(await handleProposalStatusMessage(f.deps, 0, msg(executed(facts()), 0, t))).toBe("ignored")
		}
		expect(await handleProposalStatusMessage(f.deps, 0, {offset: "0", value: null, headers: {"event-type": "PROPOSAL_EXECUTED"}})).toBe("ignored")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toEqual([])
	})

	it("dead-letters what kg-indexer rejects, writing nothing", async () => {
		const f = pfixture()
		expect(await handleProposalStatusMessage(f.deps, 0, msg(executed(facts(), {proposal: "ab"}), 7))).toBe("dead_letter")
		expect(f.applied).toEqual([])
		expect(deadLetters(f.store)).toHaveLength(1)
	})
})

describe("stall, never skip", () => {
	it("a ResourceExhausted founding throws and dead-letters nothing", async () => {
		const f = pfixture({foundFailsOnce: "error: ResourceExhausted (space cap)"})
		await expect(handleProposalStatusMessage(f.deps, 0, msg(executed(facts()), 0))).rejects.toBeInstanceOf(SystemSpaceCapError)
		expect(deadLetters(f.store)).toEqual([])
		expect(await handleProposalStatusMessage(f.deps, 0, msg(executed(facts()), 0))).toBe("applied")
	})

	it("refuses to send put while not allowed", async () => {
		const f = pfixture()
		await expect(handleProposalStatusMessage({...f.deps, allowPut: false}, 0, msg(executed(facts()), 0))).rejects.toThrow(/GEODB_RECORD_KEYS is off/)
	})

	it("the batch loop retries in place and counts outcomes on its own series", async () => {
		const done = new Map<number, number>()
		const deps = {
			metrics: createMetrics(),
			offsets: {hadOffsets: false, lastDone: (p: number) => done.get(p) ?? null, markDone: (p: number, o: number) => void done.set(p, o)},
		} as unknown as KeyedProposalStatusDeps
		const tries: string[] = []
		let failures = 1
		const run = proposalStatusBatchHandler({
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
		expect(deps.metrics.proposalStatusEvents.values).toEqual(new Map([['outcome="applied"', 1], ['outcome="ignored"', 1]]))
	})
})

describe("backfillProposalStatus", () => {
	it("writes each row as its record, including an unexecutable-only one", async () => {
		const model = new ModelStore()
		const f = pfixture({model})
		async function* rows() {
			yield rowOf(facts())
			yield rowOf(facts({proposal: P2, executedAt: null, unexecutableAt: 55n}))
		}
		expect(await backfillProposalStatus(f.deps, rows())).toEqual({rows: 2, applied: 2, deadLetter: 0})
		expect(model.cell(proposalStatusRecordId(P2), COL.unexecutableAt)).toEqual(['.{ .instant = "55000000" }'])
		expect(model.cell(proposalStatusRecordId(P2), COL.executedAt)).toEqual([])
	})

	it("dead-letters a row no writer could have produced", async () => {
		const f = pfixture()
		const bad = [
			{...rowOf(facts()), executed_at: "1.5"},
			{...rowOf(facts()), unexecutable_at: "99999999999999999999"},
			{...rowOf(facts()), executed_at: null, unexecutable_at: null},
		]
		expect(await backfillProposalStatus(f.deps, [...bad, rowOf(facts())])).toEqual({rows: 4, applied: 1, deadLetter: 3})
	})
})

describe("flag and gate", () => {
	const base = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}
	it("is off by default and needs GEODB_RECORD_KEYS", () => {
		const off = loadConfig(base)
		expect(off.proposalStatus).toEqual({enabled: false, topic: "testnet.space.governance", groupId: "geodb-feeder-proposal-status-keyed-v1-testnet"})
		expect(proposalStatusKeyedEnabled(off)).toBe(false)
		expect(proposalStatusKeyedEnabled(loadConfig({...base, GEODB_PROPOSAL_STATUS: "1"}))).toBe(false)
		expect(proposalStatusKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_PROPOSAL_ACTIONS: "1"}))).toBe(false)
		expect(proposalStatusKeyedEnabled(loadConfig({...base, GEODB_RECORD_KEYS: "1", GEODB_PROPOSAL_STATUS: "1"}))).toBe(true)
	})

	it("has its own group and is in the backfill group check", () => {
		const c = loadConfig(base)
		expect(c.proposalStatus.groupId).not.toBe(c.proposalActions.groupId)
		expect(feederGroupIds(c)).toContain(c.proposalStatus.groupId)
	})

	it("the backfill gate refuses once the consumer has offsets, unless told to reset them", () => {
		const dir = proposalStatusOffsetsDir(tmp())
		expect(keyedBackfillGate(dir, false)).toBe("fresh")
		new ProposalOffsets(dir).markDone(0, 4)
		expect(() => keyedBackfillGate(dir, false)).toThrow(/BACKFILL_KEYED_RESET_CONSUMER/)
		expect(keyedBackfillGate(dir, true)).toBe("reset")
	})

	function setup(geoExecuted: string, extraGeo = false) {
		const dir = mkdtempSync(join(tmpdir(), "ps-gate-"))
		const rec = proposalStatusRecordId(P1)
		const pgRow: Record<string, string> = {proposal: P1, executed_at: "1700000000", unexecutable_at: "\\N"}
		const geo: Record<string, string> = {proposal: cid("entity", P1), executed_at: geoExecuted}
		for (const [n, v] of Object.entries(geo)) {
			const lines = [`${rec}\t${v}\t1`]
			if (extraGeo) lines.push(`${"a".repeat(32)}\t${v}\t1`)
			writeFileSync(join(dir, `ps-${n}.tsv`), `${lines.join("\n")}\n`)
		}
		writeFileSync(join(dir, "ps-unexecutable_at.tsv"), "")
		writeFileSync(join(dir, "pg.tsv"), `${PG_COLUMNS.map((c) => pgRow[c]).join("\t")}\n`)
		return dir
	}

	it("the gate passes when Postgres and the records agree, null facts included", () => {
		const dir = setup("instant:1700000000.000000")
		const r = gateProposalStatus(join(dir, "pg.tsv"), dir)
		expect(r.matched).toBe(1)
		expect(r.pgOnly).toEqual([])
		expect(r.valueDiffs).toEqual([])
	})

	it("the gate reports a changed value and a geo-db-only record", () => {
		const dir = setup("instant:1700000999.000000", true)
		const r = gateProposalStatus(join(dir, "pg.tsv"), dir)
		expect(r.valueDiffs.map((d) => d.column)).toEqual(["executed_at"])
		expect(r.geoOnly).toEqual(["a".repeat(32)])
	})
})
