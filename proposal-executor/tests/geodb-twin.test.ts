/**
 * Tests for the geo-db twin of detection (src/geodb-twin). A fixture "world" holds the records the feeder writes (a
 * status record, version records, vote records, a Space's editor edges) and answers the reader's three reads, so each
 * case is a proposal's state in geo-db and the twin's verdict on it.
 */
import {afterEach, describe, expect, test} from "bun:test"
import {existsSync, readFileSync} from "node:fs"
import {PassThrough} from "node:stream"
import {Effect} from "effect"
import {RATIO_BASE} from "../src/contracts.js"
import {CLOCK_SKEW_BUFFER, DETECTION_SQL, MAX_PROPOSAL_AGE} from "../src/detect.js"
import {
	createGeoDbTwinClient,
	GeoDbTwinError,
	type GeoDbTwinReader,
	MAX_BACKLINKS,
	type PipeChild,
} from "../src/geodb-twin/client.js"
import {countEditors, evaluateProposal, type Spaces} from "../src/geodb-twin/evaluate.js"
import {renderTwinMetrics, resetTwinMetrics, snapshot} from "../src/geodb-twin/metrics.js"
import {entityRef, idPart, recordEntityId} from "../src/geodb-twin/recordKey.js"
import {
	EDITOR_PROPERTY,
	PROPOSAL_STATUS_COLUMNS,
	PROPOSAL_STATUS_KIND,
	PROPOSAL_VERSION_COLUMNS,
	PROPOSAL_VERSION_KIND,
	PROPOSAL_VOTE_COLUMNS,
	PROPOSAL_VOTE_KIND,
	proposalStatusRecordId,
} from "../src/geodb-twin/records.js"
import {
	CANDIDATE_SQL,
	compareWithGeoDb,
	loadTwinConfig,
	parseSpaceMap,
	parseSystemSpaces,
	runGeoDbTwin,
	type TwinDeps,
} from "../src/geodb-twin/twin.js"

afterEach(() => resetTwinMetrics())

// ---------------------------------------------------------------------------
// Fixture world
// ---------------------------------------------------------------------------

const GOV = "11111111111141118111111111111111"
const VOTES = "22222222222242228222222222222222"
const DAO_GEO = "33333333333343338333333333333333"
const DAO = "44444444-4444-4444-8444-444444444444"
const NOW = 1_800_000_000
const HOUR = 3600

type Row = {prop: string; raw: string}

let seq = 0

class World implements GeoDbTwinReader {
	cards = new Map<string, Map<string, Row[]>>()
	reads = 0

	private put(space: string, entity: string, rows: Row[]) {
		const space_ = this.cards.get(space) ?? new Map<string, Row[]>()
		space_.set(entity, [...(space_.get(entity) ?? []), ...rows])
		this.cards.set(space, space_)
	}
	private id() {
		seq++
		return `a${seq.toString(16).padStart(11, "0")}4aaa8aaaaaaaaaaaaaaa`.slice(0, 32)
	}

	status(proposal: string, s: {executedAt?: number; unexecutableAt?: number}) {
		const rows: Row[] = []
		if (s.executedAt !== undefined)
			rows.push({prop: PROPOSAL_STATUS_COLUMNS.executedAt, raw: `instant:${s.executedAt}.0`})
		if (s.unexecutableAt !== undefined)
			rows.push({prop: PROPOSAL_STATUS_COLUMNS.unexecutableAt, raw: `instant:${s.unexecutableAt}.0`})
		this.put(GOV, proposalStatusRecordId(proposal), rows)
	}

	version(
		proposal: string,
		v: {
			block: number
			mode?: "Fast" | "Slow"
			end: number
			quorum?: number
			partial?: number
			universal?: number
			executeBy?: number
			createdAt?: number
		},
	) {
		const c = PROPOSAL_VERSION_COLUMNS
		const rows: Row[] = [
			{prop: c.proposal, raw: entityRef(proposal)},
			{prop: c.block, raw: String(v.block)},
			{prop: c.votingMode, raw: v.mode ?? "Slow"},
			{prop: c.endTime, raw: String(v.end)},
			{prop: c.quorum, raw: String(v.quorum ?? 0)},
			{prop: c.partial, raw: String(v.partial ?? RATIO_BASE / 2)},
			{prop: c.universal, raw: String(v.universal ?? 0)},
			{prop: c.createdAt, raw: String(v.createdAt ?? NOW - 2 * HOUR)},
		]
		if (v.executeBy !== undefined) rows.push({prop: c.executeBy, raw: String(v.executeBy)})
		this.put(GOV, this.id(), rows)
	}

	votes(proposal: string, version: number, counts: {yes?: number; no?: number; abstain?: number}) {
		const cast = (option: number, n = 0) => {
			for (let i = 0; i < n; i++)
				this.put(VOTES, this.id(), [
					{prop: PROPOSAL_VOTE_COLUMNS.proposal, raw: entityRef(proposal)},
					{prop: PROPOSAL_VOTE_COLUMNS.version, raw: String(version)},
					{prop: PROPOSAL_VOTE_COLUMNS.vote, raw: String(option)},
				])
		}
		cast(1, counts.yes)
		cast(2, counts.no)
		cast(3, counts.abstain)
	}

	editors(count: number, extra: Row[] = []) {
		const rows = Array.from({length: count}, (_, i) => ({prop: EDITOR_PROPERTY, raw: `e${i}`.padEnd(32, "0")}))
		this.put(DAO_GEO, entityRef(DAO), [...rows, ...extra])
	}

	async readCard(space: string, entity: string) {
		this.reads++
		return this.cards.get(space)?.get(entity) ?? []
	}
	async readCards(space: string, entities: string[]) {
		this.reads++
		const out = new Map<string, Row[]>()
		for (const e of entities) {
			const rows = this.cards.get(space)?.get(e)
			if (rows) out.set(e, rows)
		}
		return out
	}
	async readBacklinks(space: string, entity: string, limit: number) {
		this.reads++
		const out: {prop: string; from: string}[] = []
		for (const [from, rows] of this.cards.get(space) ?? [])
			for (const r of rows) if (r.raw === entity) out.push({prop: r.prop, from})
		return out.slice(0, limit)
	}
}

const spaces: Spaces = {
	governance: GOV,
	votes: VOTES,
	of: (id) => (id.replaceAll("-", "") === DAO.replaceAll("-", "") ? DAO_GEO : null),
}
const P = "55555555-5555-4555-8555-555555555555"
const proposal = {id: P, spaceId: DAO}
const verdict = (w: World, now = NOW) => evaluateProposal(w, spaces, proposal, now)

/** A proposal whose voting ended an hour ago (past the skew buffer): the slow-late branch. */
const ended = (w: World, v: Partial<Parameters<World["version"]>[1]> = {}) =>
	w.version(P, {block: 10, end: NOW - HOUR, ...v})

// ---------------------------------------------------------------------------
// Decision tree: the fixture set
// ---------------------------------------------------------------------------

describe("slow-late", () => {
	test("passed: quorum met and yes outweighs no at the partial threshold", async () => {
		const w = new World()
		ended(w, {quorum: 3})
		w.votes(P, 1, {yes: 3, no: 1, abstain: 1})
		expect(await verdict(w)).toEqual({kind: "decided", ready: true, why: "late_passed"})
	})

	test("failed: no outweighs yes", async () => {
		const w = new World()
		ended(w, {quorum: 3})
		w.votes(P, 1, {yes: 1, no: 3})
		expect(await verdict(w)).toEqual({kind: "decided", ready: false, why: "late_threshold_not_met"})
	})

	test("failed at the boundary: yes == no at a 50% threshold is not enough (strict inequality)", async () => {
		const w = new World()
		ended(w, {quorum: 2})
		w.votes(P, 1, {yes: 2, no: 2})
		expect(await verdict(w)).toMatchObject({ready: false, why: "late_threshold_not_met"})
	})

	test("under quorum: passing votes do not execute below quorum, abstains count toward it", async () => {
		const w = new World()
		ended(w, {quorum: 5})
		w.votes(P, 1, {yes: 2, abstain: 2})
		expect(await verdict(w)).toEqual({kind: "decided", ready: false, why: "quorum_not_met"})
		w.votes(P, 1, {abstain: 1})
		expect(await verdict(w)).toMatchObject({ready: true, why: "late_passed"})
	})

	test("a partial threshold above 50% needs a larger majority", async () => {
		const w = new World()
		ended(w, {quorum: 1, partial: 6_666_667}) // two thirds
		w.votes(P, 1, {yes: 2, no: 1})
		expect(await verdict(w)).toMatchObject({ready: false})
		w.votes(P, 1, {yes: 1})
		expect(await verdict(w)).toMatchObject({ready: true})
	})

	test("no votes at quorum 0: 0 > 0 is false, so not ready", async () => {
		const w = new World()
		ended(w, {quorum: 0})
		expect(await verdict(w)).toMatchObject({ready: false, why: "late_threshold_not_met"})
	})

	test("inside the clock-skew buffer after end_time neither branch applies", async () => {
		const w = new World()
		w.version(P, {block: 10, end: NOW - CLOCK_SKEW_BUFFER, quorum: 1})
		w.votes(P, 1, {yes: 5})
		expect(await verdict(w)).toEqual({kind: "decided", ready: false, why: "clock_skew_window"})
		expect(await verdict(w, NOW + 1)).toMatchObject({ready: true})
	})
})

describe("slow-early and the editor-count edge", () => {
	const early = (w: World, universal: number) => w.version(P, {block: 10, end: NOW + HOUR, universal})

	test("ceil(universal * editors / BASE): 3 editors at a one-third threshold need 1 yes, at 40% need 2", async () => {
		for (const [universal, editors, need] of [
			[3_333_334, 3, 2], // 3 * 3_333_334 / 1e7 = 1.0000002 -> ceil 2
			[3_333_333, 3, 1], // 0.9999999 -> ceil 1
			[4_000_000, 3, 2], // 1.2 -> 2
			[5_000_000, 4, 2], // exactly 2.0 -> 2 (no rounding up on an exact fit)
			[RATIO_BASE, 7, 7],
		] as const) {
			const below = new World()
			early(below, universal)
			below.editors(editors)
			below.votes(P, 1, {yes: need - 1})
			expect(await verdict(below)).toMatchObject({ready: false, why: "early_threshold_not_met"})

			const at = new World()
			early(at, universal)
			at.editors(editors)
			at.votes(P, 1, {yes: need})
			expect(await verdict(at)).toMatchObject({ready: true, why: "early_passed"})
		}
	})

	test("a Space with no editors is never ready early, however many yes votes", async () => {
		const w = new World()
		early(w, 1)
		w.votes(P, 1, {yes: 10})
		expect(await verdict(w)).toEqual({kind: "decided", ready: false, why: "no_editors"})
	})

	test("universal threshold 0 disables the early branch", async () => {
		const w = new World()
		early(w, 0)
		w.editors(3)
		w.votes(P, 1, {yes: 10})
		expect(await verdict(w)).toEqual({kind: "decided", ready: false, why: "early_disabled"})
	})

	test("an editor referenced twice counts once, and non-editor edges do not count", async () => {
		const rows = [
			{prop: EDITOR_PROPERTY, raw: "a"},
			{prop: EDITOR_PROPERTY, raw: "a"},
			{prop: EDITOR_PROPERTY, raw: "b"},
			{prop: "dbe3bdf637a5458d9206d737fb104793", raw: "member-not-editor"},
		]
		expect(countEditors(rows)).toBe(2)
	})

	test("the editor count is read from the Space's own geo-db Space; an unloaded Space is unsupported", async () => {
		const w = new World()
		early(w, 5_000_000)
		w.votes(P, 1, {yes: 5})
		const unloaded: Spaces = {...spaces, of: () => null}
		expect(await evaluateProposal(w, unloaded, proposal, NOW)).toEqual({
			kind: "unsupported",
			reason: "space_not_loaded",
		})
	})

	test("votes are not read when the early branch cannot apply", async () => {
		const w = new World()
		early(w, 0)
		await verdict(w)
		expect(w.reads).toBe(3) // status, version backlinks, version cards: no votes, no editors
	})
})

describe("preconditions and deadline", () => {
	test("executed is not ready, and stops the reads", async () => {
		const w = new World()
		ended(w, {quorum: 1})
		w.votes(P, 1, {yes: 5})
		w.status(P, {executedAt: NOW - 10})
		w.reads = 0
		expect(await verdict(w)).toEqual({kind: "decided", ready: false, why: "executed"})
		expect(w.reads).toBe(1)
	})

	test("unexecutable is not ready", async () => {
		const w = new World()
		ended(w, {quorum: 1})
		w.votes(P, 1, {yes: 5})
		w.status(P, {unexecutableAt: NOW - 10})
		expect(await verdict(w)).toEqual({kind: "decided", ready: false, why: "unexecutable"})
	})

	test("a status record with no columns (never executed) does not block", async () => {
		const w = new World()
		ended(w, {quorum: 1})
		w.votes(P, 1, {yes: 1})
		w.status(P, {})
		expect(await verdict(w)).toMatchObject({ready: true})
	})

	test("execute_by set: ready through the deadline second, not after", async () => {
		const w = new World()
		ended(w, {quorum: 1, executeBy: NOW})
		w.votes(P, 1, {yes: 1})
		expect(await verdict(w)).toMatchObject({ready: true})
		expect(await verdict(w, NOW + 1)).toEqual({kind: "decided", ready: false, why: "past_execute_by"})
	})

	test("execute_by unset: ready inside MAX_PROPOSAL_AGE of created_at, not at it", async () => {
		const w = new World()
		w.version(P, {block: 10, end: NOW - HOUR, quorum: 1, createdAt: NOW - MAX_PROPOSAL_AGE + 1})
		w.votes(P, 1, {yes: 1})
		expect(await verdict(w)).toMatchObject({ready: true})
		expect(await verdict(w, NOW + 1)).toEqual({kind: "decided", ready: false, why: "past_max_age"})
	})

	test("created_at is the earliest version's, not the latest's", async () => {
		const w = new World()
		w.version(P, {block: 5, end: NOW - 2 * HOUR, quorum: 1, createdAt: NOW - MAX_PROPOSAL_AGE - 5})
		w.version(P, {block: 9, end: NOW - HOUR, quorum: 1, createdAt: NOW - HOUR})
		w.votes(P, 2, {yes: 1})
		expect(await verdict(w)).toMatchObject({ready: false, why: "past_max_age"})
	})

	test("created in the future is not ready", async () => {
		const w = new World()
		w.version(P, {block: 10, end: NOW + HOUR, createdAt: NOW + 5})
		expect(await verdict(w)).toMatchObject({ready: false, why: "created_in_future"})
	})
})

describe("versions and votes", () => {
	test("the latest version's settings apply, and only votes naming its ordinal count", async () => {
		const w = new World()
		w.version(P, {block: 5, end: NOW - HOUR, quorum: 1})
		w.version(P, {block: 9, end: NOW - HOUR, quorum: 4}) // ordinal 2, quorum raised
		w.votes(P, 1, {yes: 9}) // old version: ignored
		w.votes(P, 2, {yes: 2})
		expect(await verdict(w)).toMatchObject({ready: false, why: "quorum_not_met"})
		w.votes(P, 2, {yes: 2})
		expect(await verdict(w)).toMatchObject({ready: true})
	})

	test("another proposal's votes do not count", async () => {
		const w = new World()
		ended(w, {quorum: 1})
		w.votes("66666666-6666-4666-8666-666666666666", 1, {yes: 9})
		expect(await verdict(w)).toMatchObject({ready: false})
	})

	test("no version record is unsupported", async () => {
		expect(await verdict(new World())).toEqual({kind: "unsupported", reason: "version_missing"})
	})

	test("a Fast latest version over a Slow row is an escalation geo-db cannot resolve here", async () => {
		const w = new World()
		w.version(P, {block: 5, mode: "Slow", end: NOW - HOUR})
		w.version(P, {block: 9, mode: "Fast", end: NOW - HOUR})
		expect(await verdict(w)).toEqual({kind: "unsupported", reason: "escalation_pending"})
	})

	test("an ambiguous column is unsupported, not guessed", async () => {
		const w = new World()
		ended(w)
		const v = [...(w.cards.get(GOV)?.values() ?? [])][0] as Row[]
		v.push({prop: PROPOSAL_VERSION_COLUMNS.quorum, raw: "99"})
		expect(await verdict(w)).toEqual({kind: "unsupported", reason: "ambiguous_record"})
	})

	test("a read that hits the backlink cap is unsupported", async () => {
		const w = new World()
		ended(w, {quorum: 1})
		const capped: GeoDbTwinReader = {
			readCard: (s, e) => w.readCard(s, e),
			readCards: (s, e) => w.readCards(s, e),
			readBacklinks: async (s, e, l) =>
				s === VOTES
					? Array.from({length: MAX_BACKLINKS}, (_, i) => ({
							prop: PROPOSAL_VOTE_COLUMNS.proposal,
							from: String(i),
						}))
					: w.readBacklinks(s, e, l),
		}
		expect(await evaluateProposal(capped, spaces, proposal, NOW)).toEqual({
			kind: "unsupported",
			reason: "too_many_votes",
		})
	})

	test("a missing system Space is unsupported", async () => {
		const w = new World()
		expect(await evaluateProposal(w, {...spaces, governance: null}, proposal, NOW)).toMatchObject({
			reason: "system_space_missing",
		})
		ended(w)
		expect(await evaluateProposal(w, {...spaces, votes: null}, proposal, NOW)).toMatchObject({
			reason: "system_space_missing",
		})
	})
})

// ---------------------------------------------------------------------------
// Parity with the SQL decision, by construction
// ---------------------------------------------------------------------------

describe("parity with detection SQL", () => {
	test("the constants the twin shares with DETECTION_SQL are interpolated into it", () => {
		expect(DETECTION_SQL).toContain(`+ ${CLOCK_SKEW_BUFFER}`)
		expect(DETECTION_SQL).toContain(`< ${MAX_PROPOSAL_AGE}`)
		expect(DETECTION_SQL).toContain(String(RATIO_BASE))
	})

	test("the candidate query uses the same execution window as detection", () => {
		expect(CANDIDATE_SQL).toContain(`< ${MAX_PROPOSAL_AGE}`)
		expect(CANDIDATE_SQL).toContain("pc.voting_mode = 'Slow'")
		expect(CANDIDATE_SQL).toContain("LIMIT $2")
	})

	test("a reference implementation of the SQL formulas agrees with the twin over a grid", async () => {
		const sqlLate = (yes: number, no: number, ab: number, quorum: number, partial: number) =>
			yes + no + ab >= quorum && (RATIO_BASE - partial) * yes > partial * no
		const sqlEarly = (yes: number, editors: number, universal: number) =>
			universal > 0 && editors > 0 && yes >= Math.ceil((universal * editors) / RATIO_BASE)
		for (const partial of [3_000_000, 5_000_000, 6_666_667])
			for (const [yes, no, ab] of [
				[0, 0, 0],
				[1, 1, 0],
				[2, 1, 1],
				[3, 2, 0],
				[1, 3, 4],
			] as const)
				for (const quorum of [0, 2, 5]) {
					const w = new World()
					ended(w, {quorum, partial})
					w.votes(P, 1, {yes, no, abstain: ab})
					expect(await verdict(w)).toMatchObject({ready: sqlLate(yes, no, ab, quorum, partial)})
				}
		for (const universal of [0, 1, 3_333_333, 3_333_334, 5_000_000, RATIO_BASE])
			for (const editors of [0, 1, 3, 4, 7])
				for (const yes of [0, 1, 2, 3, 7]) {
					const w = new World()
					w.version(P, {block: 10, end: NOW + HOUR, universal})
					w.editors(editors)
					w.votes(P, 1, {yes})
					expect(await verdict(w)).toMatchObject({ready: sqlEarly(yes, editors, universal)})
				}
	})
})

// ---------------------------------------------------------------------------
// Record ids
// ---------------------------------------------------------------------------

describe("record ids", () => {
	test("recordEntityId matches the engine vector the feeder vendors (record-entity)", () => {
		const id = recordEntityId("203f5e7d-9cbb-4af9-9837-567594b3d2f1", [
			idPart("7493b2d1-f00f-4e4d-ac8b-aac9e8072645"),
			idPart("85a4c3e2-0120-4f5e-bd9c-bbdaf9183756"),
		])
		expect(id).toBe("71162d82efd14c0b8c5cddc49747834e")
	})

	test("entityRef keeps a v4 id and remaps any other deterministically", () => {
		expect(entityRef(DAO)).toBe(DAO.replaceAll("-", ""))
		const legacy = "00000000-0000-0000-0000-000000000001"
		expect(entityRef(legacy)).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
		expect(entityRef(legacy)).toBe(entityRef(legacy.toUpperCase()))
	})

	test("the copied ids equal the feeder's recordKinds.ts when the checkout has it", () => {
		const file = new URL("../../geodb-feeder/src/recordKinds.ts", import.meta.url)
		if (!existsSync(file)) return
		const text = readFileSync(file, "utf8")
		for (const id of [
			PROPOSAL_STATUS_KIND,
			PROPOSAL_VERSION_KIND,
			PROPOSAL_VOTE_KIND,
			...Object.values(PROPOSAL_VERSION_COLUMNS),
			...Object.values(PROPOSAL_STATUS_COLUMNS),
			...Object.values(PROPOSAL_VOTE_COLUMNS),
		])
			expect(text).toContain(id)
	})
})

// ---------------------------------------------------------------------------
// Comparison, mismatch metric, isolation
// ---------------------------------------------------------------------------

const prop = (n: number): {id: string; spaceId: string} => ({
	id: `0000000${n}-0000-4000-8000-000000000000`,
	spaceId: DAO,
})

/** Deps whose reader answers a fixed verdict per proposal through a World built for it. */
function depsFor(
	worlds: Map<string, World>,
	candidates: {id: string; spaceId: string}[],
	extra: Partial<TwinDeps> = {},
): TwinDeps {
	const reader: GeoDbTwinReader = {
		readCard: (s, e) => pick(s).readCard(s, e),
		readCards: (s, e) => pick(s).readCards(s, e),
		readBacklinks: (s, e, l) => pick(s).readBacklinks(s, e, l),
	}
	// One merged world: every proposal's records live together.
	const merged = new World()
	for (const w of worlds.values())
		for (const [space, m] of w.cards) {
			const t = merged.cards.get(space) ?? new Map()
			for (const [k, v] of m) t.set(k, [...(t.get(k) ?? []), ...v])
			merged.cards.set(space, t)
		}
	function pick(_space: string) {
		return merged
	}
	return {candidates: async () => candidates, reader, spaces, failed: () => false, now: Date.now, ...extra}
}

function readyWorld(id: string, pass: boolean): World {
	const w = new World()
	w.version(id, {block: 10, end: NOW - HOUR, quorum: 1})
	w.votes(id, 1, pass ? {yes: 2} : {no: 2})
	return w
}

describe("compareWithGeoDb", () => {
	const cfg = {budgetMs: 10_000, candidateMax: 100}

	test("match, missing_in_geodb and extra_in_geodb are counted and logged; SQL ready ids are always compared", async () => {
		const [a, b, c, d] = [prop(1), prop(2), prop(3), prop(4)]
		const worlds = new Map([
			[a.id, readyWorld(a.id, true)], // SQL ready, geo ready: match
			[b.id, readyWorld(b.id, false)], // SQL ready, geo not: missing_in_geodb
			[c.id, readyWorld(c.id, true)], // SQL not ready, geo ready: extra_in_geodb
			[d.id, readyWorld(d.id, false)], // neither: match
		])
		const logged: unknown[] = []
		const out = await compareWithGeoDb(depsFor(worlds, [c, d]), [a, b], NOW, cfg, (m) => logged.push(m))
		expect(out).toMatchObject({compared: 4, match: 2, mismatch: 2, unsupported: 0, failed: 0, aborted: null})
		expect(snapshot().mismatches).toEqual({missing_in_geodb: 1, extra_in_geodb: 1})
		expect(snapshot().compared).toEqual({match: 2, mismatch: 2})
		expect(logged).toEqual([
			{proposalId: b.id, kind: "missing_in_geodb", sql: true, geodb: false, why: "late_threshold_not_met"},
			{proposalId: c.id, kind: "extra_in_geodb", sql: false, geodb: true, why: "late_passed"},
		])
	})

	test("dashed and compact ids are the same proposal, and a candidate that is also SQL-ready is compared once", async () => {
		const a = prop(1)
		const worlds = new Map([[a.id, readyWorld(a.id, true)]])
		const out = await compareWithGeoDb(depsFor(worlds, [{...a, id: a.id.replaceAll("-", "")}]), [a], NOW, cfg)
		expect(out).toMatchObject({compared: 1, match: 1})
	})

	test("an unsupported proposal is counted by reason and is not a mismatch", async () => {
		const out = await compareWithGeoDb(depsFor(new Map(), [prop(1)]), [prop(2)], NOW, cfg)
		expect(out).toMatchObject({compared: 2, unsupported: 2, mismatch: 0})
		expect(snapshot().unsupported).toEqual({version_missing: 2})
	})

	test("a failed read counts failed and stops the run; nothing throws", async () => {
		let n = 0
		const deps = depsFor(new Map(), [prop(1), prop(2), prop(3)], {
			reader: {
				readCard: async () => {
					n++
					throw new GeoDbTwinError("timeout", "slow")
				},
				readCards: async () => new Map(),
				readBacklinks: async () => [],
			},
		})
		const out = await compareWithGeoDb(deps, [], NOW, cfg)
		expect(out).toMatchObject({compared: 1, failed: 1, aborted: "failure"})
		expect(n).toBe(1)
	})

	test("the run stops when the client is cooling down", async () => {
		const deps = depsFor(new Map(), [prop(1)], {failed: () => true})
		expect(await compareWithGeoDb(deps, [], NOW, cfg)).toMatchObject({compared: 0, aborted: "failure"})
	})

	test("the run stops at its time budget", async () => {
		let t = 0
		const deps = depsFor(new Map(), [prop(1), prop(2), prop(3)], {now: () => (t += 6_000)})
		const out = await compareWithGeoDb(deps, [], NOW, {budgetMs: 10_000, candidateMax: 100})
		expect(out.aborted).toBe("budget")
		expect(out.compared).toBeLessThan(3)
	})

	test("a failing candidate query rejects (runGeoDbTwin swallows it)", async () => {
		const deps = depsFor(new Map(), [], {
			candidates: async () => {
				throw new Error("statement timeout")
			},
		})
		await expect(compareWithGeoDb(deps, [], NOW, cfg)).rejects.toThrow("statement timeout")
	})

	test("metrics render as Prometheus text", async () => {
		const a = prop(1)
		await compareWithGeoDb(depsFor(new Map([[a.id, readyWorld(a.id, false)]]), []), [a], NOW, cfg)
		const text = renderTwinMetrics()
		expect(text).toContain('proposal_executor_geodb_twin_compared_total{result="mismatch"} 1')
		expect(text).toContain('proposal_executor_geodb_twin_mismatch_total{kind="missing_in_geodb"} 1')
	})
})

describe("runGeoDbTwin is inert and cannot fail", () => {
	test("flag off (unset, or anything but true): returns at once, touches nothing", async () => {
		for (const env of [{}, {PROPOSAL_EXECUTOR_GEODB_TWIN: "false"}, {PROPOSAL_EXECUTOR_GEODB_TWIN: "1"}]) {
			expect(loadTwinConfig(env)).toBeNull()
			await Effect.runPromise(runGeoDbTwin(env, "postgres://unused", [], NOW))
		}
		expect(snapshot().runs).toEqual({})
	})

	test("flag on without its paths: skipped, no throw", async () => {
		const env = {PROPOSAL_EXECUTOR_GEODB_TWIN: "true"}
		expect(loadTwinConfig(env)).toBe("misconfigured")
		await Effect.runPromise(runGeoDbTwin(env, "postgres://unused", [], NOW))
	})

	test("flag on, unreadable space map: the Effect succeeds, an error run is counted", async () => {
		const env = {
			PROPOSAL_EXECUTOR_GEODB_TWIN: "true",
			GEODB_CLIENT_BIN: "/nonexistent/geodb-client",
			GEODB_KEY_PATH: "/nonexistent/key",
			GEODB_BINDING_PATH: "/nonexistent/binding",
			GEODB_SPACE_MAP_PATH: "/nonexistent/space-map.json",
		}
		await Effect.runPromise(runGeoDbTwin(env, "postgres://u:secret@127.0.0.1:1/db", [prop(1)], NOW))
		expect(snapshot().runs).toEqual({error: 1})
	})

	test("config defaults and bounds", () => {
		const base = {
			PROPOSAL_EXECUTOR_GEODB_TWIN: "true",
			GEODB_CLIENT_BIN: "b",
			GEODB_KEY_PATH: "k",
			GEODB_BINDING_PATH: "g",
			GEODB_SPACE_MAP_PATH: "m",
		}
		expect(loadTwinConfig(base)).toMatchObject({timeoutMs: 2000, budgetMs: 20000, candidateMax: 100})
		expect(
			loadTwinConfig({
				...base,
				PROPOSAL_EXECUTOR_GEODB_TWIN_TIMEOUT_MS: "999999",
				PROPOSAL_EXECUTOR_GEODB_TWIN_CANDIDATE_MAX: "x",
			}),
		).toMatchObject({timeoutMs: 30000, candidateMax: 100})
	})

	test("space map and system spaces parse like the api's", () => {
		const map = parseSpaceMap(
			JSON.stringify({[DAO]: {geo: DAO_GEO}, "77777777-7777-4777-8777-777777777777": {geo: GOV, loaded: false}}),
		)
		expect([...map.entries()]).toEqual([[DAO.replaceAll("-", ""), DAO_GEO]])
		expect(() => parseSpaceMap('{"x":{"geo":"y"}}')).toThrow()
		expect(
			parseSystemSpaces(JSON.stringify({governance: {geo: GOV}, votes: {geo: VOTES}, bad: {geo: "z"}})).size,
		).toBe(2)
	})
})

// ---------------------------------------------------------------------------
// Client: frames, refusal, timeout, cool-down
// ---------------------------------------------------------------------------

function fakeChild() {
	const stdin = new PassThrough()
	const stdout = new PassThrough()
	const written: string[] = []
	stdin.on("data", (c: Buffer) => written.push(c.toString()))
	let killed = 0
	const listeners: Record<string, (() => void)[]> = {}
	const child = {
		stdin,
		stdout,
		stderr: new PassThrough(),
		kill: () => {
			killed++
			return true
		},
		on: (event: string, l: () => void) => {
			listeners[event] = [...(listeners[event] ?? []), l]
		},
	} as unknown as PipeChild
	return {
		child,
		stdout,
		written,
		killed: () => killed,
		exit: () => {
			for (const l of listeners.exit ?? []) l()
		},
	}
}

const clientConfig = {clientBin: "x", keyPath: "k", bindingPath: "b", timeoutMs: 40, cooldownMs: 1000}
const frame = (kind: "+" | "-", body: string) => `${kind}${Buffer.byteLength(body)}\n${body}`
const ENTITY = "0123456789ab4def8123456789abcdef"

describe("geo-db twin client", () => {
	test("a card read writes one line and parses the frame; text is unescaped", async () => {
		const f = fakeChild()
		const client = createGeoDbTwinClient(clientConfig, {spawnPipe: () => f.child})
		const p = client.readCard(GOV, ENTITY)
		await Bun.sleep(5)
		expect(f.written.join("")).toStartWith(`${GOV}\t.{ .version = 1`)
		expect(f.written.join("")).toContain(`.id = "${ENTITY}"`)
		f.stdout.write(frame("+", `p1\ta\\tb\tsid\t${ENTITY}\nother\tz\tsid\t${"f".repeat(32)}\n`))
		expect(await p).toEqual([{prop: "p1", raw: "a\tb"}])
		client.close()
	})

	test("a frame split across chunks is reassembled; the next read reuses the worker", async () => {
		const f = fakeChild()
		let spawned = 0
		const client = createGeoDbTwinClient(clientConfig, {
			spawnPipe: () => {
				spawned++
				return f.child
			},
		})
		const body = `p1\t1\tsid\t${ENTITY}\n`
		const whole = frame("+", body)
		const p = client.readCard(GOV, ENTITY)
		await Bun.sleep(5)
		f.stdout.write(whole.slice(0, 4))
		await Bun.sleep(5)
		f.stdout.write(whole.slice(4))
		expect(await p).toHaveLength(1)
		const p2 = client.readCard(GOV, ENTITY)
		await Bun.sleep(5)
		f.stdout.write(frame("+", ""))
		expect(await p2).toEqual([])
		expect(spawned).toBe(1)
		client.close()
	})

	test("a refusal rejects client_failed, kills the worker and opens the cool-down", async () => {
		const f = fakeChild()
		let t = 0
		const client = createGeoDbTwinClient(clientConfig, {spawnPipe: () => f.child, now: () => t})
		const p = client.readCard(GOV, ENTITY)
		await Bun.sleep(5)
		f.stdout.write(frame("-", "MalformedRequest"))
		await expect(p).rejects.toMatchObject({reason: "client_failed"})
		expect(f.killed()).toBe(1)
		expect(client.coolingDown()).toBe(true)
		await expect(client.readCard(GOV, ENTITY)).rejects.toMatchObject({reason: "cooling_down"})
		t = 1001
		expect(client.coolingDown()).toBe(false)
		client.close()
	})

	test("a read with no answer times out, kills the worker and cools down", async () => {
		const f = fakeChild()
		const client = createGeoDbTwinClient(clientConfig, {spawnPipe: () => f.child})
		await expect(client.readCard(GOV, ENTITY)).rejects.toMatchObject({reason: "timeout"})
		expect(f.killed()).toBe(1)
		expect(client.coolingDown()).toBe(true)
		client.close()
	})

	test("a worker that exits while owing a reply rejects it", async () => {
		const f = fakeChild()
		const client = createGeoDbTwinClient(clientConfig, {spawnPipe: () => f.child})
		const p = client.readCard(GOV, ENTITY)
		await Bun.sleep(5)
		f.exit()
		await expect(p).rejects.toMatchObject({reason: "client_failed"})
		client.close()
	})

	test("batch cards go 32 at a time; backlinks cap at geo-db's limit; close refuses further reads", async () => {
		const f = fakeChild()
		const client = createGeoDbTwinClient({...clientConfig, timeoutMs: 500}, {spawnPipe: () => f.child})
		const ids = Array.from({length: 33}, (_, i) =>
			`${i.toString(16).padStart(12, "0")}4aaa8aaaaaaaaaaaaaaa`.slice(0, 32),
		)
		const p = client.readCards(GOV, ids)
		await Bun.sleep(5)
		f.stdout.write(frame("+", `p\t1\ts\t${ids[0]}\n`))
		await Bun.sleep(5)
		f.stdout.write(frame("+", `p\t2\ts\t${ids[32]}\n`))
		const got = await p
		expect([...got.keys()].sort()).toEqual([ids[0], ids[32]].sort() as string[])
		expect(f.written.filter((l) => l.includes(".ids = ")).length).toBe(2)

		const b = client.readBacklinks(GOV, ENTITY, 999_999)
		await Bun.sleep(5)
		expect(f.written.join("")).toContain(`.backlinks = ${MAX_BACKLINKS}`)
		f.stdout.write(frame("+", `c\t${ENTITY}\ts\t${"e".repeat(32)}\nc\telse\ts\t${"d".repeat(32)}\n`))
		expect(await b).toEqual([{prop: "c", from: "e".repeat(32)}])

		client.close()
		await expect(client.readCard(GOV, ENTITY)).rejects.toMatchObject({reason: "closed"})
	})
})
