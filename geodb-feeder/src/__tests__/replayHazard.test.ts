import {readFileSync} from "node:fs"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {
	HAZARD_PAIRS,
	deadLetteredIntents,
	findHazardPairs,
	intentsOfZonDoc,
	pairName,
	parseIntent,
	planChunks,
	refusingHazardPairs,
} from "../replayHazard.js"
import {putIntent, zonDoc} from "../translate.js"

const h = (n: number) => n.toString(16).padStart(32, "0")
const E1 = h(1)
const E2 = h(2)
const P1 = h(11)
const P2 = h(12)
const S1 = h(21)
const S2 = h(22)
const S3 = h(23)

// Intent texts in the forms the translator (translate.ts) and putIntent write.
const put = (e: string, p: string) => putIntent({id: h(99), entity: e, property: p, value: ".{ .text = \"x\" }"})
const unset = (e: string, p: string) => `.{ .unset = .{ .entity = "${e}", .property = "${p}" } }`
const del = (e: string) => `.{ .delete_entity = "${e}" }`
const move = (e: string, id: string, replacement: string) =>
	`.{ .set_position = .{ .entity = "${e}", .property = "${P1}", .id = "${id}", .replacement = "${replacement}", .position = "a0" } }`
const retract = (s: string) => `.{ .set_active = .{ .statement = "${s}", .active = false } }`
const set = (e: string, p: string) => `.{ .set = .{ .entity = "${e}", .property = "${p}", .value = .{ .text = "y" } } }`

describe("parseIntent", () => {
	it("reads the kinds the pairs name, and their targets", () => {
		expect(parseIntent(put(E1, P1))).toEqual({kind: "put", entity: E1, property: P1})
		expect(parseIntent(unset(E1, P1))).toEqual({kind: "unset", entity: E1, property: P1})
		expect(parseIntent(`.{ .unset = .{ .statement = "${S1}", .property = "${P1}" } }`)).toEqual({kind: "unset", entity: S1, property: P1})
		expect(parseIntent(del(E1))).toEqual({kind: "delete_entity", entity: E1})
		expect(parseIntent(move(E1, S1, S2))).toEqual({kind: "relation_move", entity: E1, statement: S1, replacement: S2})
		expect(parseIntent(retract(S1))).toEqual({kind: "relation_retract", statement: S1})
	})
	it("calls every other intent other, a set_active true included", () => {
		expect(parseIntent(set(E1, P1)).kind).toBe("other")
		expect(parseIntent(`.{ .set_active = .{ .statement = "${S1}", .active = true } }`).kind).toBe("other")
		expect(parseIntent(`.{ .create_entity = "${E1}" }`).kind).toBe("other")
	})
})

describe("the six ISS-679-318 pairs", () => {
	it("is the issue's table, in its order", () => {
		expect(HAZARD_PAIRS.map((p) => `${pairName(p)}${p.prefixOnly ? " (prefix)" : ""}`)).toEqual([
			"put;unset",
			"put;delete_entity",
			"relation_move;delete_entity",
			"delete_entity;relation_move",
			"relation_retract;relation_move (prefix)",
			"relation_move;relation_move (prefix)",
		])
	})

	// One case per pair: the pair on the same target is found; the same kinds on another target, or the other order
	// where that order is not itself a pair, are not.
	const cases: {pair: string; hit: string[]; miss: string[][]}[] = [
		{pair: "put;unset", hit: [put(E1, P1), unset(E1, P1)], miss: [[put(E1, P1), unset(E1, P2)], [put(E1, P1), unset(E2, P1)], [unset(E1, P1), put(E1, P1)]]},
		{pair: "put;delete_entity", hit: [put(E1, P1), del(E1)], miss: [[put(E1, P1), del(E2)], [del(E1), put(E1, P1)]]},
		{pair: "relation_move;delete_entity", hit: [move(E1, S1, S2), del(E1)], miss: [[move(E1, S1, S2), del(E2)]]},
		{pair: "delete_entity;relation_move", hit: [del(E1), move(E1, S1, S2)], miss: [[del(E2), move(E1, S1, S2)]]},
		{pair: "relation_retract;relation_move", hit: [retract(S1), move(E1, S1, S2)], miss: [[retract(S3), move(E1, S1, S2)], [move(E1, S1, S2), retract(S2)]]},
		{pair: "relation_move;relation_move", hit: [move(E1, S1, S2), move(E1, S2, S3)], miss: [[move(E1, S1, S2), move(E1, S3, h(24))]]},
	]
	for (const c of cases) {
		it(`finds ${c.pair} on one target, and not across targets`, () => {
			const hits = findHazardPairs(c.hit)
			expect(hits.map((x) => x.pair)).toEqual([c.pair])
			expect(hits[0]).toMatchObject({earlier: 0, later: 1})
			for (const m of c.miss) expect(findHazardPairs(m)).toEqual([])
		})
	}

	it("finds pairs that are not adjacent, and ignores intents no pair names", () => {
		const hits = findHazardPairs([put(E1, P1), set(E1, P1), `.{ .create_entity = "${E2}" }`, del(E1)])
		expect(hits).toEqual([{pair: "put;delete_entity", earlier: 0, later: 3, prefixOnly: false}])
	})
})

describe("deadLetteredIntents", () => {
	it("counts one edit's intents in errors.jsonl and skips other edits, decode letters and junk", () => {
		const meta = {partition: 0, offset: "98656", space: "s", edit: "e", idempotent: true}
		const jsonl = [
			JSON.stringify({t: 1, ...meta, intent: "A", err: "connect: Refused"}),
			JSON.stringify({t: 2, ...meta, intent: "A", err: "connect: Refused"}),
			JSON.stringify({t: 3, ...meta, intent: "B", err: "x"}),
			JSON.stringify({t: 4, ...meta, edit: "other", intent: "C", err: "x"}),
			JSON.stringify({t: 5, partition: 0, offset: "1", decode: "bad"}),
			"not json",
			"",
		].join("\n")
		expect(deadLetteredIntents(jsonl, {space: "s", edit: "e"})).toEqual(
			new Map([
				["A", 2],
				["B", 1],
			]),
		)
	})
})

describe("planChunks", () => {
	const intents = [put(E1, P1), unset(E1, P1), set(E2, P1), set(E2, P2)]

	it("a chunk whose every intent is dead-lettered is a first apply and passes, pair or not", () => {
		const all = new Map(intents.map((i) => [i, 1]))
		const plans = planChunks(intents, 2, all)
		expect(plans.map((p) => [p.lo, p.hi, p.status, p.refuse])).toEqual([
			[0, 2, "first-apply", null],
			[2, 4, "first-apply", null],
		])
		expect(plans[0]?.pairs.map((p) => p.pair)).toEqual(["put;unset"])
	})

	it("refuses an applied chunk with a pair, and passes an applied chunk without one", () => {
		const plans = planChunks(intents, 2, new Map([[put(E1, P1), 1]]))
		expect(plans[0]).toMatchObject({status: "applied", deadLettered: 1})
		expect(plans[0]?.refuse).toMatch(/chunk \[0,2\) already applied \(1 of 2 .*put;unset \(ISS-679-318\)/)
		expect(plans[1]).toMatchObject({status: "applied", refuse: null})
	})

	it("with no record keyed (no --edit), every chunk counts as applied", () => {
		const plans = planChunks(intents, 4, null)
		expect(plans).toHaveLength(1)
		expect(plans[0]).toMatchObject({status: "applied", deadLettered: 0})
		expect(plans[0]?.refuse).not.toBeNull()
	})

	it("uses each dead letter once: a repeated intent dead-lettered once leaves its twin applied", () => {
		const twice = [move(E1, S1, S2), move(E1, S1, S2)]
		const plans = planChunks(twice, 2, new Map([[twice[0] as string, 1]]))
		expect(plans[0]).toMatchObject({status: "applied", deadLettered: 1})
		expect(plans[0]?.pairs.map((p) => p.pair)).toEqual(["relation_move;relation_move"])
	})

	it("reports pair indices in the edit, not the chunk", () => {
		const plans = planChunks([set(E2, P1), set(E2, P2), put(E1, P1), del(E1)], 2, null)
		expect(plans[1]?.pairs).toEqual([{pair: "put;delete_entity", earlier: 2, later: 3, prefixOnly: false}])
	})
})

describe("refusingHazardPairs", () => {
	it("reads a zonDoc back into its intents", () => {
		const doc = zonDoc(h(5), h(6), [put(E1, P1), del(E2)])
		expect(intentsOfZonDoc(doc)).toEqual([put(E1, P1), del(E2)])
	})

	it("sends a document without a pair and refuses one with a pair, sending nothing", async () => {
		const sent: string[] = []
		const apply = refusingHazardPairs(async (doc: string) => {
			sent.push(doc)
			return {rc: 0}
		})
		await apply(zonDoc(h(5), h(6), [put(E1, P1), put(E1, P2)]))
		await expect(apply(zonDoc(h(5), h(6), [put(E1, P1), unset(E1, P1)]))).rejects.toThrow(/put;unset \(ISS-679-318\)/)
		expect(sent).toHaveLength(1)
	})

	it("the keyed families replay-keyed replays write no intent that ends a pair", () => {
		// Every pair's later kind is unset, delete_entity or a move; a retract is the only other earlier kind. These
		// families build add, set_active true and put only (keyedRecord.ts, putIntent). A new form here must come with
		// a look at ISS-679-318 before replay-keyed's "no pair possible" line stays true.
		const files = ["editMetaKeyed.ts", "proposalStatusKeyed.ts", "proposalVersionsKeyed.ts", "proposalVotesKeyed.ts", "keyedRecord.ts"]
		for (const f of files) {
			const src = readFileSync(join(import.meta.dirname, "..", f), "utf8")
			expect(src, f).not.toMatch(/\.(unset|delete_entity|set_position)\s*=|\.active\s*=\s*false/)
		}
	})
})
