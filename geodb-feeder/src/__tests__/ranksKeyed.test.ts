import {relationEntityId} from "@geoprotocol/grc-20"
import {describe, expect, it} from "vitest"
import {feederGroupIds, loadConfig} from "../config.js"
import {cid} from "../ids.js"
import {keyedBackfillGate} from "../membershipKeyed.js"
import {ProposalOffsets} from "../proposals.js"
import {idPart, recordEntityId, recordStatementId} from "../recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "../recordKinds.js"
import type {GrcOp, GrcPropertyValue} from "../translate.js"
import {
	backfillRanks,
	detectRanks,
	handleRanksMessage,
	type RankingBlockRow,
	type RankingItemRow,
	type RankingRow,
	ranksIntentGroups,
	ranksKeyedEnabled,
	ranksOffsetsDir,
	rankingBlockRecordId,
	rankingItemRecordId,
	rankingRecordId,
} from "../ranksKeyed.js"
import {deadLetters, fixture, ModelStore} from "./keyedHarness.js"

const hexBytes = (h: string) => Uint8Array.from(Buffer.from(h, "hex"))
const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const ITEM_SPACE = "b29c345ab9866679b001d7d2138d88a2"
const RANK = "11111111111141118111111111111111"
const RANK2 = "22222222222242228222222222222222"
const BLOCK = "33333333333343338333333333333333"
const ENT_A = "44444444444444448444444444444444"
const ENT_B = "55555555555545558555555555555555"
const RESTRICTION = "66666666666646668666666666666666"
const d = (u: string) => u.replaceAll("-", "")
const TYPE_REL = d("8f151ba4-de20-4e3c-9cb4-99ddf96f48f1")
const NAME = d("a126ca53-0c8e-48d5-b888-82c734c38935")
const RANK_TYPE = d("5c74731d-fabb-4dc8-b5c5-3346521c639a")
const RANK_TYPE_PROP = d("48e01bc8-324e-48c2-a6c9-cab3f49290c6")
const VOTES_REL = d("19a4cfff-45f2-4150-abf2-af0f43eb2eec")
const WEIGHT = d("103701dd-cabe-4a8e-835b-10345327b647")
const BLOCK_TYPE = d("150db6de-fe23-44f0-805a-fa57502e2c32")
const BLOCK_REL = d("09c219c1-03d1-4d2a-a5c7-8edbf2d0182a")
const START_DATE = d("eed03a04-0acd-4a9e-81e0-8272ed70a817")
const START_DT = d("2d696bf0-510f-403e-985b-8cd1e73feb9b")
const END_DATE = d("b08b8f63-dc1e-4156-8b08-19946f2b011c")
const RESTRICT_REL = d("1e4caa2d-e331-4efa-8ac2-4e8d9d3e9fe9")
const FILTER = d("14a46854-bfd1-4b18-8215-2785c2dab9f3")
const FREQ = d("c0ab9953-8eeb-4d7c-8ccb-1dd585ae4711")
const ROLLING = d("d00ddc89-b23c-4a20-a63f-ebf545ddc139")

const KB = RECORD_KINDS.rankingBlock.id
const KR = RECORD_KINDS.ranking.id
const KI = RECORD_KINDS.rankingItem.id
const CB = RECORD_COLUMNS.rankingBlock
const CR = RECORD_COLUMNS.ranking
const CI = RECORD_COLUMNS.rankingItem

const text = (property: string, value: string): GrcPropertyValue => ({property: hexBytes(property), value: {type: "text", value}})
const rel = (id: string, relationType: string, from: string, to: string, extra: Record<string, unknown> = {}): GrcOp => ({
	type: "createRelation",
	id: hexBytes(id),
	relationType: hexBytes(relationType),
	from: hexBytes(from),
	to: hexBytes(to),
	...extra,
})
const ent = (id: string, values: GrcPropertyValue[] = []): GrcOp => ({type: "createEntity", id: hexBytes(id), values})
const META = {space: SPACE, block: 1234, timestamp: 1_700_000_000}

/** An edit: a ranking with a rank type, two voted items (one weighted), and a block link. */
const rankingOps = (ranking = RANK, items: [string, string, string?][] = [[ENT_A, "a0"], [ENT_B, "b0", "2.5"]]): GrcOp[] => [
	ent(ranking, [text(RANK_TYPE_PROP, "ordinal")]),
	rel("90000000000040008000000000000001", TYPE_REL, ranking, RANK_TYPE),
	...items.flatMap(([e, pos, weight], i): GrcOp[] => {
		const id = `9100000000004000800000000000000${i}`
		return [
			rel(id, VOTES_REL, ranking, e, {toSpace: hexBytes(ITEM_SPACE), position: pos}),
			...(weight === undefined ? [] : [{type: "createEntity", id: relationEntityId(hexBytes(id) as never), values: [{property: hexBytes(WEIGHT), value: {type: "float", value: Number(weight)}}]} as GrcOp]),
		]
	}),
]

describe("record kinds", () => {
	it("live in the votes family, so no fourth system Space is founded", () => {
		expect(RECORD_KINDS.rankingBlock.family).toBe("votes")
		expect(RECORD_KINDS.ranking.family).toBe("votes")
		expect(RECORD_KINDS.rankingItem.family).toBe("votes")
	})
	it("key on the primary key of each table", () => {
		expect(rankingBlockRecordId(BLOCK, SPACE)).toBe(recordEntityId(KB, [idPart(BLOCK), idPart(SPACE)]))
		expect(rankingRecordId(RANK, SPACE)).toBe(recordEntityId(KR, [idPart(RANK), idPart(SPACE)]))
		expect(rankingItemRecordId(RANK, ENT_A, ITEM_SPACE)).toBe(recordEntityId(KI, [idPart(RANK), idPart(ENT_A), idPart(ITEM_SPACE)]))
		expect(rankingRecordId(RANK, SPACE)).not.toBe(rankingRecordId(RANK, ITEM_SPACE))
	})
})

describe("detectRanks (ranking-indexer detect.rs)", () => {
	it("a Rank-typed entity is a ranking with its rank type, block, timestamp and op index", () => {
		const r = detectRanks(rankingOps(), META)
		expect(r.rankings).toEqual([{ranking: RANK, rankType: "ordinal", submittedAt: 1_700_000_000n, updatedAtBlock: 1234n, updateIndex: 0n}])
	})
	it("an untyped entity, or one typed only in another edit, is nothing", () => {
		expect(detectRanks([ent(RANK, [text(RANK_TYPE_PROP, "x")])], META).rankings).toEqual([])
	})
	it("RANK_VOTES with to_space is an item with position and the reified entity's weight; without to_space it is skipped", () => {
		const votes = rel("91000000000040008000000000000009", VOTES_REL, RANK, ENT_A, {toSpace: hexBytes(ITEM_SPACE), position: "a0"})
		const weightEnt = {type: "createEntity", id: relationEntityId(hexBytes("91000000000040008000000000000009") as never), values: [{property: hexBytes(WEIGHT), value: {type: "float", value: 0.75}}]}
		const r = detectRanks([votes, weightEnt, rel("91000000000040008000000000000008", VOTES_REL, RANK, ENT_B, {position: "b0"})], META)
		expect(r.items).toEqual([{ranking: RANK, entity: ENT_A, space: ITEM_SPACE, position: "a0", weight: 0.75}])
	})
	it("a block: name, filter, datetime wins over legacy date, rolling type, restriction, frequency", () => {
		const ops = [
			ent(BLOCK, [
				text(NAME, "Top"),
				text(FILTER, "f"),
				{property: hexBytes(START_DATE), value: {type: "date", value: "2024-01-02"}},
				{property: hexBytes(START_DT), value: {type: "datetime", value: "2024-03-04T05:06:07.123456Z"}},
				{property: hexBytes(END_DATE), value: {type: "date", value: "2024-05-06"}},
				{property: hexBytes(FREQ), value: {type: "integer", value: 7}},
			]),
			rel("90000000000040008000000000000002", TYPE_REL, BLOCK, BLOCK_TYPE),
			rel("90000000000040008000000000000003", TYPE_REL, BLOCK, ROLLING),
			rel("90000000000040008000000000000004", RESTRICT_REL, BLOCK, RESTRICTION),
		]
		expect(detectRanks(ops, META).blocks).toEqual([
			{
				block: BLOCK,
				name: "Top",
				filter: "f",
				startDate: BigInt(Date.UTC(2024, 2, 4, 5, 6, 7)) * 1000n + 123_456n,
				endDate: BigInt(Date.UTC(2024, 4, 6)) * 1000n,
				restriction: RESTRICTION,
				rankingType: ROLLING,
				submissionFrequency: 7n,
			},
		])
	})
	it("a RANK_BLOCK relation is a link", () => {
		expect(detectRanks([rel("90000000000040008000000000000005", BLOCK_REL, RANK, BLOCK)], META).links).toEqual([{ranking: RANK, block: BLOCK}])
	})
	it("a later duplicate of a ranking or item within the edit wins", () => {
		const ops = [...rankingOps(), ...rankingOps(RANK, [[ENT_A, "zz"]])]
		const r = detectRanks(ops, META)
		expect(r.rankings.length).toBe(1)
		expect(r.items.find((i) => i.entity === ENT_A)?.position).toBe("zz")
	})
})

describe("intents", () => {
	it("a ranking is a record, its two key columns, and puts for the mutable ones; nothing absent is written", () => {
		const groups = ranksIntentGroups(detectRanks(rankingOps(), META), META.space)
		const all = groups.flat().join("\n")
		const rec = rankingRecordId(RANK, SPACE)
		expect(all).toContain(`.{ .create_entity = "${rec}" }`)
		expect(all).toContain(`.property = "${CR.ranking}", .value = .{ .reference = "${cid("entity", RANK)}" }`)
		expect(all).toContain(`.property = "${CR.space}", .value = .{ .reference = "${cid("entity", SPACE)}" }`)
		expect(all).toContain(`.property = "${CR.rankType}", .value = .{ .text = "ordinal" }`)
		expect(all).toContain(`.property = "${CR.submittedAt}", .value = .{ .instant = "1700000000000000" }`)
		expect(all).toContain(`.property = "${CR.updatedAtBlock}", .value = .{ .number = 1234 }`)
		expect(all).not.toContain(`.property = "${CR.block}"`)
		expect(all).not.toContain(`.property = "${CR.author}"`)
	})
	it("items carry the edit's block as their generation; a missing weight is not written", () => {
		const all = ranksIntentGroups(detectRanks(rankingOps(), META), META.space).flat().join("\n")
		expect(all).toContain(`.property = "${CI.generation}", .value = .{ .number = 1234 }`)
		expect(all).toContain(`.property = "${CI.weight}", .value = .{ .decimal = "25,-1" }`)
		expect(all.match(new RegExp(`\\.property = "${CI.weight}"`, "g"))?.length).toBe(1)
	})
	it("a link writes the ranking record's block reference", () => {
		const all = ranksIntentGroups(detectRanks([rel("90000000000040008000000000000005", BLOCK_REL, RANK, BLOCK)], META), META.space).flat().join("\n")
		expect(all).toContain(`.property = "${CR.block}", .value = .{ .reference = "${cid("entity", BLOCK)}" }`)
	})
})

const msg = (offset = 0) => ({offset: String(offset), value: editBytes(), headers: {}})
function editBytes(): Uint8Array {
	const varint = (n: number) => {
		const out: number[] = []
		let v = n
		do {
			let b = v & 0x7f
			v = Math.floor(v / 128)
			if (v) b |= 0x80
			out.push(b)
		} while (v)
		return out
	}
	const bytes = (f: number, b: number[]) => [...varint((f << 3) | 2), ...varint(b.length), ...b]
	const meta = [...varint(1 << 3), ...varint(1_700_000_000), ...varint(3 << 3), ...varint(1234)]
	return Uint8Array.from([...bytes(3, [1]), ...bytes(6, [...hexBytes(SPACE)]), ...bytes(8, meta)])
}

const rfixture = (ops: GrcOp[], o: Parameters<typeof fixture>[0] = {}) => {
	const model = new ModelStore()
	const f = fixture({...o, model, offsetsDir: ranksOffsetsDir})
	const deps = {...f.deps, decode: async () => ({ops: ops})}
	return {...f, model, deps}
}

describe("handleRanksMessage", () => {
	it("applies one edit and a replay of it changes nothing", async () => {
		const f = rfixture(rankingOps())
		expect(await handleRanksMessage(f.deps, 0, msg())).toBe("applied")
		const once = f.model.snapshot()
		expect(await handleRanksMessage(f.deps, 0, msg())).toBe("applied")
		expect(f.model.snapshot()).toBe(once)
	})
	it("an edit with nothing rank-relevant is ignored and writes nothing", async () => {
		const f = rfixture([ent(ENT_A)])
		expect(await handleRanksMessage(f.deps, 0, msg())).toBe("ignored")
		expect(f.applied).toEqual([])
	})
	it("a message with no value is ignored; an undecodable or spaceless one is dead-lettered", async () => {
		const f = rfixture([])
		expect(await handleRanksMessage(f.deps, 0, {offset: "1", value: null, headers: {}})).toBe("ignored")
		expect(await handleRanksMessage(f.deps, 0, {offset: "2", value: Uint8Array.from([0x1a, 0x05, 1]), headers: {}})).toBe("dead_letter")
		expect(deadLetters(f.store).length).toBe(1)
	})
	it("a later edit replaces the whole ranking: only the newest generation's items are current", async () => {
		const f = rfixture(rankingOps())
		await handleRanksMessage(f.deps, 0, msg())
		const f2 = {...f.deps, decode: async () => ({ops: rankingOps(RANK, [[ENT_A, "a1"]]) })}
		const later = {...msg(1), value: editBytesAt(1300)}
		await handleRanksMessage(f2, 0, later)
		const gens = (entity: string) => f.model.cell(rankingItemRecordId(RANK, entity, ITEM_SPACE), CI.generation)
		expect(gens(ENT_A)).toEqual([".{ .number = 1300 }"])
		expect(gens(ENT_B)).toEqual([".{ .number = 1234 }"]) // stale, below the ranking's max generation
	})
	it("replaying the stream from the start ends in the same state as one pass", async () => {
		const f = rfixture(rankingOps())
		const older = {...msg(0)}
		const newer = {...msg(1), value: editBytesAt(1300)}
		const newerDeps = {...f.deps, decode: async () => ({ops: rankingOps(RANK, [[ENT_A, "a1"]]) })}
		const pass = async () => {
			await handleRanksMessage(f.deps, 0, older)
			await handleRanksMessage(newerDeps, 0, newer)
		}
		await pass()
		const once = f.model.snapshot()
		await pass()
		expect(f.model.snapshot()).toBe(once)
		expect(f.model.cell(rankingItemRecordId(RANK, ENT_A, ITEM_SPACE), CI.position)).toEqual([`.{ .text = "a1" }`])
	})
})

function editBytesAt(block: number): Uint8Array {
	const b = Buffer.from(editBytes())
	// patch the trailing varint of the block (1234 is two bytes) by rebuilding
	const varint = (n: number) => {
		const out: number[] = []
		let v = n
		do {
			let x = v & 0x7f
			v = Math.floor(v / 128)
			if (v) x |= 0x80
			out.push(x)
		} while (v)
		return out
	}
	const bytes = (f: number, a: number[]) => [...varint((f << 3) | 2), ...varint(a.length), ...a]
	void b
	const meta = [...varint(1 << 3), ...varint(1_700_000_000), ...varint(3 << 3), ...varint(block)]
	return Uint8Array.from([...bytes(3, [1]), ...bytes(6, [...hexBytes(SPACE)]), ...bytes(8, meta)])
}

describe("backfill", () => {
	const dashed = (h: string) => `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
	const blocks: RankingBlockRow[] = [{id: dashed(BLOCK), space_id: dashed(SPACE), name: "Top", filter: null, start_date: "1704153600000000", end_date: null, restriction_id: null, ranking_type: null, submission_frequency: 3}]
	const rankings: RankingRow[] = [{id: dashed(RANK), space_id: dashed(SPACE), block_id: dashed(BLOCK), author_address: "0xabc", rank_type: "ordinal", submitted_at: "1700000000000000", updated_at_block: "1234", update_index: "0"}]
	const items: RankingItemRow[] = [{ranking_id: dashed(RANK), entity_id: dashed(ENT_A), space_id: dashed(ITEM_SPACE), position: "a0", weight: 0.5}]
	it("writes the three tables at generation 0 and is rerunnable", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: ranksOffsetsDir})
		const r = await backfillRanks(f.deps, {blocks, rankings, items})
		expect(r).toEqual({rows: 3, applied: 3, deadLetter: 0})
		const once = model.snapshot()
		await backfillRanks(f.deps, {blocks, rankings, items})
		expect(model.snapshot()).toBe(once)
		expect(model.cell(rankingItemRecordId(RANK, ENT_A, ITEM_SPACE), CI.generation)).toEqual([".{ .number = 0 }"])
		expect(model.cell(rankingRecordId(RANK, SPACE), CR.block)).toEqual([`.{ .reference = "${cid("entity", BLOCK)}" }`])
		expect(model.cell(rankingRecordId(RANK, SPACE), CR.author)).toEqual([`.{ .text = "0xabc" }`])
	})
	it("a stream replay supersedes the backfill's items", async () => {
		const model = new ModelStore()
		const f = fixture({model, offsetsDir: ranksOffsetsDir})
		await backfillRanks(f.deps, {blocks, rankings, items})
		await handleRanksMessage({...f.deps, decode: async () => ({ops: rankingOps(RANK, [[ENT_B, "b1"]]) })}, 0, msg())
		expect(model.cell(rankingItemRecordId(RANK, ENT_B, ITEM_SPACE), CI.generation)).toEqual([".{ .number = 1234 }"])
	})
	it("a row no writer could have written is dead-lettered", async () => {
		const f = fixture({offsetsDir: ranksOffsetsDir})
		const r = await backfillRanks(f.deps, {blocks: [], rankings: [{...rankings[0]!, updated_at_block: "x"}], items: []})
		expect(r.deadLetter).toBe(1)
	})
})

describe("flag and group", () => {
	const env = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b", ENVIRONMENT: "t", GEODB_RANKS: "1", GEODB_RECORD_KEYS: "1"}
	it("is on only with GEODB_RANKS and GEODB_RECORD_KEYS", () => {
		const c = loadConfig(env)
		expect(ranksKeyedEnabled(c)).toBe(true)
		expect(ranksKeyedEnabled({...c, recordKeys: false})).toBe(false)
		expect(ranksKeyedEnabled({...c, ranks: {...c.ranks, enabled: false}})).toBe(false)
	})
	it("has its own group on the edits topic, and the backfill Job's empty-group check covers it", () => {
		const c = loadConfig(env)
		expect(c.ranks.topic).toBe("t.knowledge.edits")
		expect(c.ranks.groupId).toBe("geodb-feeder-ranks-keyed-v1-t")
		expect(feederGroupIds(c)).toContain(c.ranks.groupId)
	})
	it("backfill gate uses the consumer's offsets dir", () => {
		expect(ranksOffsetsDir("/d")).toBe("/d/feeder/ranks-keyed")
		expect(typeof keyedBackfillGate).toBe("function")
		expect(new ProposalOffsets(ranksOffsetsDir(fixture().store.dir))).toBeTruthy()
	})
})
