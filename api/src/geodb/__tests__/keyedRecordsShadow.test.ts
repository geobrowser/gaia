import {readFileSync} from "node:fs"
import {join} from "node:path"
import {beforeEach, describe, expect, it, vi} from "vitest"
import type {Database} from "../../forYou/queries"
import type {GeoCardRow} from "../card"
import {GeoDbError} from "../client"
import {
	__resetKeyedRecordsShadowMetricsForTests,
	compareRecord,
	createKeyedRecordsShadow,
	RANKING_BLOCK_COLUMNS,
	RANKING_BLOCK_KIND,
	RANKING_COLUMNS,
	RANKING_KIND,
	rankingBlockRecord,
	rankingRecord,
	readKeyedRecordsShadowConfig,
	renderKeyedRecordsShadowMetrics,
	USER_VOTE_COLUMNS,
	USER_VOTE_KIND,
	userVoteRecord,
} from "../keyedRecordsShadow"

const U = "11111111-1111-4111-8111-111111111111"
const O = "22222222-2222-4222-8222-222222222222"
const S = "33333333-3333-4333-8333-333333333333"
const hex = (id: string) => id.replaceAll("-", "")
const log = {warn: vi.fn()}

const vote = {
	user_id: U,
	object_id: O,
	object_type: 0,
	space_id: S,
	vote_kind: 3,
	vote_type: 0,
	voted_at_seconds: "1700000000",
}
const cardOfVote = (over: Partial<Record<string, string>> = {}): GeoCardRow[] => {
	const C = USER_VOTE_COLUMNS
	const rows: Record<string, string> = {
		[C.user]: hex(U),
		[C.object]: hex(O),
		[C.space]: hex(S),
		[C.objectType]: "0",
		[C.kind]: "3.0",
		[C.direction]: "0",
		[C.votedAt]: "instant:1700000000.000000000",
		...over,
	}
	return Object.entries(rows).map(([prop, raw]) => ({prop, raw}))
}
const count = (table: string, result: string) =>
	Number(new RegExp(`table="${table}",result="${result}"\\} (\\d+)`).exec(renderKeyedRecordsShadowMetrics())?.[1])

beforeEach(() => {
	__resetKeyedRecordsShadowMetricsForTests()
	log.warn.mockClear()
})

describe("pins", () => {
	it("copies the feeder's kinds and columns", () => {
		const feeder = readFileSync(join(__dirname, "../../../../geodb-feeder/src/recordKinds.ts"), "utf8")
		const all = [
			USER_VOTE_KIND,
			RANKING_KIND,
			RANKING_BLOCK_KIND,
			...Object.values(USER_VOTE_COLUMNS),
			...Object.values(RANKING_COLUMNS),
			...Object.values(RANKING_BLOCK_COLUMNS),
		]
		for (const id of all) expect(feeder).toContain(id)
	})

	it("derives each record id from the key parts the feeder uses", () => {
		const read = (f: string) =>
			readFileSync(join(__dirname, "../../../../geodb-feeder/src", f), "utf8").replace(/\s+/g, " ")
		expect(read("votesKeyed.ts")).toContain(
			"recordEntityId(KIND, [idPart(v.user), idPart(v.object), intPart(v.objectType), idPart(v.space), intPart(voteKindKey(v.kind))])",
		)
		expect(read("ranksKeyed.ts")).toContain("recordEntityId(K.ranking.id, [idPart(ranking), idPart(space)])")
		expect(read("ranksKeyed.ts")).toContain("recordEntityId(K.block.id, [idPart(block), idPart(space)])")
		// Different keys give different records; a negative kind keys as its u16 pattern, distinct from every kind >= 0.
		const ids = new Set([
			userVoteRecord(vote)?.entityId,
			userVoteRecord({...vote, vote_kind: -1})?.entityId,
			userVoteRecord({...vote, vote_kind: 65535 - 1})?.entityId,
			userVoteRecord({...vote, space_id: O})?.entityId,
		])
		expect(ids.size).toBe(4)
	})
})

describe("config", () => {
	it("is off unless the flag says on, and clamps tunables", () => {
		expect(readKeyedRecordsShadowConfig({}).enabled).toBe(false)
		expect(readKeyedRecordsShadowConfig({GEODB_KEYED_RECORDS_SHADOW: "1"}).enabled).toBe(true)
		const c = readKeyedRecordsShadowConfig({GEODB_KEYED_RECORDS_SHADOW_SAMPLE: "9999"})
		expect(c.sample).toBe(200)
		expect(c.intervalMs).toBe(60_000)
	})
})

describe("compareRecord", () => {
	const sampled = userVoteRecord(vote)
	if (!sampled) throw new Error("vote record")

	it("matches a record holding every column", () => {
		expect(compareRecord(sampled, cardOfVote())).toBe("match")
	})
	it("is missing when the card is empty", () => {
		expect(compareRecord(sampled, [])).toBe("missing")
	})
	it("mismatches on a changed direction", () => {
		expect(compareRecord(sampled, cardOfVote({[USER_VOTE_COLUMNS.direction]: "1"}))).toBe("mismatch")
	})
	it("mismatches on a column holding two values", () => {
		const card = [...cardOfVote(), {prop: USER_VOTE_COLUMNS.direction, raw: "1"}]
		expect(compareRecord(sampled, card)).toBe("mismatch")
	})
	it("does not compare a relation vote's object", () => {
		const rel = userVoteRecord({...vote, object_type: 1})
		if (!rel) throw new Error("rel")
		expect(
			compareRecord(rel, cardOfVote({[USER_VOTE_COLUMNS.objectType]: "1", [USER_VOTE_COLUMNS.object]: "ff"})),
		).toBe("match")
	})
	it("requires a null Postgres column to be absent", () => {
		const r = rankingRecord({
			id: O,
			space_id: S,
			block_id: null,
			rank_type: null,
			updated_at_block: "5",
			update_index: "2",
			submitted_at_seconds: null,
		})
		if (!r) throw new Error("ranking")
		const C = RANKING_COLUMNS
		const card = [
			{prop: C.ranking, raw: hex(O)},
			{prop: C.space, raw: hex(S)},
			{prop: C.updatedAtBlock, raw: "5"},
			{prop: C.updateIndex, raw: "2"},
		]
		expect(compareRecord(r, card)).toBe("match")
		expect(compareRecord(r, [...card, {prop: C.rankType, raw: "x"}])).toBe("mismatch")
	})
	it("compares a ranking's text, block reference and instant", () => {
		const r = rankingRecord({
			id: O,
			space_id: S,
			block_id: U,
			rank_type: "top",
			updated_at_block: "5",
			update_index: "2",
			submitted_at_seconds: "10",
		})
		if (!r) throw new Error("ranking")
		const C = RANKING_COLUMNS
		const card = [
			{prop: C.ranking, raw: hex(O)},
			{prop: C.space, raw: hex(S)},
			{prop: C.block, raw: hex(U)},
			{prop: C.rankType, raw: "top"},
			{prop: C.submittedAt, raw: "instant:10.000000000"},
			{prop: C.updatedAtBlock, raw: "5"},
			{prop: C.updateIndex, raw: "2"},
		]
		expect(compareRecord(r, card)).toBe("match")
		expect(
			compareRecord(
				r,
				card.map((c) => (c.prop === C.rankType ? {...c, raw: "bottom"} : c)),
			),
		).toBe("mismatch")
	})
})

describe("sampler", () => {
	const db = (rows: Record<string, unknown>[]) => ({execute: vi.fn(async () => ({rows}))}) as unknown as Database
	const make = (
		rows: Record<string, unknown>[],
		readCard: (a: {entityId: string}) => Promise<GeoCardRow[]>,
		space: string | null = "s",
	) =>
		createKeyedRecordsShadow({
			config: {enabled: true, intervalMs: 1000, sample: 5},
			db: db(rows),
			geoDb: {readCard: vi.fn(readCard)},
			systemSpaceOf: async () => space,
			log,
			random: () => U,
		})

	it("exports nothing until mounted", () => {
		expect(renderKeyedRecordsShadowMetrics()).toBe("")
		make([], async () => [])
		expect(renderKeyedRecordsShadowMetrics()).toContain('table="user_votes",result="match"} 0')
	})

	it("counts match, missing and failed per table", async () => {
		const outcomes = [cardOfVote(), []]
		let i = 0
		const shadow = make([vote], async () => {
			const o = outcomes[i++]
			if (!o) throw new GeoDbError("timeout", "slow")
			return o
		})
		await shadow.runOnce()
		// Three tables each read the one fake row; the user_votes pass runs first, so only it can match.
		expect(count("user_votes", "match")).toBe(1)
		expect(count("rankings", "missing")).toBe(0)
		expect(count("rankings", "failed") + count("rankings", "missing")).toBe(0)
	})

	it("counts unsupported when the votes Space is unknown", async () => {
		const shadow = make([vote], async () => [], null)
		await shadow.runOnce()
		expect(count("user_votes", "unsupported")).toBe(1)
		expect(count("rankings", "unsupported")).toBe(1)
	})

	it("counts a failed sample and never throws", async () => {
		const shadow = createKeyedRecordsShadow({
			config: {enabled: true, intervalMs: 1000, sample: 5},
			db: {execute: async () => Promise.reject(new Error("down"))} as unknown as Database,
			geoDb: {readCard: async () => []},
			systemSpaceOf: async () => "s",
			log,
		})
		await expect(shadow.runOnce()).resolves.toBeUndefined()
		expect(count("user_votes", "failed")).toBe(1)
	})
})
