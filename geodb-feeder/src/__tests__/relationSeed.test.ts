import {existsSync, mkdtempSync, readFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {parseClaimRows, claimsDocument, type GeoDbClaimRow, type RelationReader} from "../geodbPipe.js"
import {cid} from "../ids.js"
import type {PgRelationRow} from "../pgAdapter.js"
import type {RelationSeedSource} from "../pgSource.js"
import {runRelationSeed, type RelationSeedDeps, type RelationSeedOptions} from "../relationSeed.js"
import {Overlay, Store} from "../state.js"
import {RELATION_FACET_PROPERTY, translateOps, type GrcOp} from "../translate.js"

const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const GEO = "daf0bc83ca744e1a9ee0377967846846"
const ent = (n: number) => `${n.toString(16).padStart(8, "0")}-0000-4000-8000-000000000000`
const hex = (n: number) => ent(n).replaceAll("-", "")
const rel = (n: number) => `${(0x70000000 + n).toString(16)}-0000-4000-8000-000000000000`
const RTYPE = "33333333-3333-4333-8333-333333333333"
const typ = cid("entity", RTYPE.replaceAll("-", ""))
const sidOf = (n: number) => cid("statement", rel(n).replaceAll("-", ""))
const relRow = (n: number, from = n): PgRelationRow => ({id: rel(n), from_entity_id: ent(from), to_entity_id: ent(99), type_id: RTYPE})
const row = (n: number, from: number, claim?: [string, string]): GeoDbClaimRow => ({
	prop: typ,
	statement: sidOf(n),
	entity: cid("entity", hex(from)),
	claimProp: claim?.[0] ?? null,
	claimRaw: claim?.[1] ?? null,
})

// Entity 1: relations 1 (no facets) and 2 (to-space + verified). Entity 2: relation 3 is not open in geo-db, relation 4 is.
// Entity 3: relation 5 already has a feeder-made record.
const relations = [relRow(1), relRow(2, 1), relRow(3, 2), relRow(4, 2), relRow(5, 3)]
const geoRows = new Map<string, GeoDbClaimRow[]>([
	[cid("entity", hex(1)), [row(1, 1), row(2, 1, [RELATION_FACET_PROPERTY.toSpace, hex(40)]), row(2, 1, [RELATION_FACET_PROPERTY.verified, "true"])]],
	[cid("entity", hex(2)), [row(4, 2)]],
	[cid("entity", hex(3)), [row(5, 3)]],
])

function world() {
	const reads: string[] = []
	const reader: RelationReader = {
		entityClaimRows: async (g, e) => {
			reads.push(`${g}|${e}`)
			return geoRows.get(e) ?? []
		},
		reset: () => {},
	}
	const source: RelationSeedSource = {
		listRelationEntities: async () => [ent(1), ent(2), ent(3)],
		readRelations: async (_s, ids) => relations.filter((r) => ids.includes(r.from_entity_id)),
	}
	return {reader, source, reads}
}

const store = () => {
	const s = new Store(mkdtempSync(join(tmpdir(), "seed-")))
	s.spaces[SPACE] = {geo: GEO, nonce: "n", keyed: true}
	s.spaces.b29c345ab9866679b001d7d2138d88a2 = {geo: null, nonce: "m", keyed: true}
	s.saveSpaces()
	// relation 5 was made by the feeder, and later moved: its record is current, and the seed must keep it.
	s.commitMarks([], [sidOf(5)], new Map([[sidOf(5), {from: cid("entity", hex(3)), prop: typ, current: "moved-id"}]]))
	return s
}

const OPTS: RelationSeedOptions = {batch: 2, pauseMs: 0, maxEntities: 0, dryRun: false}
const mk = (s: Store, w: ReturnType<typeof world>): RelationSeedDeps => ({source: w.source, reader: w.reader, store: s, guard: async () => {}, sleep: async () => {}})

describe("runRelationSeed", () => {
	it("records each open relation's cell, current statement and facets, and leaves the rest", async () => {
		const s = store()
		const w = world()
		const stats = await runRelationSeed(mk(s, w), OPTS)
		expect(stats).toMatchObject({
			spaces: 1,
			spacesUnmapped: 1,
			entities: 3,
			relations: 5,
			alreadyRecorded: 1,
			notOpen: 1,
			toSeed: 3,
			seeded: 3,
			withFacets: 1,
			statementsMarked: 3,
			facetsUnreadable: 0,
		})
		const from1 = cid("entity", hex(1))
		expect(s.relationOf(sidOf(1))).toEqual({from: from1, prop: typ, current: sidOf(1)})
		expect(s.relationOf(sidOf(2))).toEqual({from: from1, prop: typ, current: sidOf(2), facets: {toSpace: hex(40), verified: "true"}})
		expect(s.relationOf(sidOf(3))).toBeUndefined()
		expect(s.relationOf(sidOf(4))?.from).toBe(cid("entity", hex(2)))
		// the feeder's own record, moved since, is untouched
		expect(s.relationOf(sidOf(5))?.current).toBe("moved-id")
		// the statements journal now holds what geo-db holds, which updateRelation requires
		expect(s.hasStatement(sidOf(1))).toBe(true)
		expect(s.hasStatement(sidOf(3))).toBe(false)
		// and the records survive a restart of the feeder
		expect(new Store(s.dir).relationOf(sidOf(2))?.facets).toEqual({toSpace: hex(40), verified: "true"})
	})

	it("logs progress when each Space finishes and at most every 30s inside one", async () => {
		const lines: Record<string, unknown>[] = []
		let clock = 0
		const w = world()
		const deps: RelationSeedDeps = {...mk(store(), w), now: () => (clock += 20_000), onProgress: (f) => lines.push(f)}
		await runRelationSeed(deps, OPTS)
		// the fake clock moves 20s per read, so a mid-Space line appears, plus one for the finished Space
		const done = lines.filter((l) => l.spaceDone)
		expect(done).toHaveLength(1)
		expect(done[0]!).toMatchObject({space: SPACE, spacesTotal: 2, entities: 3, relations: 5, toSeed: 3, seeded: 3})
		expect(done[0]!.spacesDone).toBe(1)
		expect(typeof done[0]!.entitiesPerSecond).toBe("number")
		expect(lines.some((l) => !l.spaceDone)).toBe(true)
	})

	it("a dry run counts what a real run records and writes no journal line and no cursor", async () => {
		const s = store()
		const before = readFileSync(`${s.dir}/known-relations.log`, "utf8")
		const stats = await runRelationSeed(mk(s, world()), {...OPTS, dryRun: true})
		expect(stats).toMatchObject({dryRun: true, toSeed: 3, seeded: 0})
		expect(readFileSync(`${s.dir}/known-relations.log`, "utf8")).toBe(before)
		expect(s.relationOf(sidOf(1))).toBeUndefined()
		expect(existsSync(`${s.dir}/relation-seed`)).toBe(false)
	})

	it("is idempotent: a rerun, and one with the cursor gone, seed nothing more", async () => {
		const s = store()
		await runRelationSeed(mk(s, world()), OPTS)
		const again = await runRelationSeed(mk(new Store(s.dir), world()), OPTS)
		expect(again.entities).toBe(0)
		const lines = readFileSync(`${s.dir}/known-relations.log`, "utf8").split("\n").filter(Boolean).length
		const fresh = new Store(s.dir)
		const w = world()
		// the cursor is gone but the records are not
		const third = await runRelationSeed({...mk(fresh, w), source: {...w.source}}, {...OPTS, maxEntities: 0, batch: 10})
		expect(third.entities).toBe(0)
		expect(readFileSync(`${s.dir}/known-relations.log`, "utf8").split("\n").filter(Boolean).length).toBe(lines)
	})

	it("a seeded relation is movable: the feeder's move now applies instead of relation-move-cell-unknown", async () => {
		const s = store()
		await runRelationSeed(mk(s, world()), OPTS)
		const op = (n: number): GrcOp => ({type: "updateRelation", id: rel(n).replaceAll("-", ""), position: "b1"}) as GrcOp
		const r = translateOps([op(2)], new Overlay(s, SPACE), {positions: true, facets: true})
		const replacement = cid("relation-position", `${sidOf(2)}b1`)
		expect(r.stats["relation-move-cell-unknown"]).toBeUndefined()
		expect(r.intents[0]).toContain(`.set_position = .{ .entity = "${cid("entity", hex(1))}", .property = "${typ}", .id = "${sidOf(2)}", .replacement = "${replacement}", .position = "b1" }`)
		// the facet claims it held are written again on the copy
		expect(r.intents.length).toBe(3)
		expect(r.intents[1]).toContain(`.statement = "${replacement}"`)
	})

	it("stops at the entity limit and resumes from the cursor", async () => {
		const s = store()
		const first = await runRelationSeed(mk(s, world()), {...OPTS, batch: 1, maxEntities: 1})
		expect(first).toMatchObject({stoppedAtLimit: true, entities: 1})
		const rest = await runRelationSeed(mk(new Store(s.dir), world()), {...OPTS, batch: 1})
		expect(rest.entities).toBe(2)
		expect(new Store(s.dir).relationOf(sidOf(4))).toBeDefined()
	})
})

describe("claims read", () => {
	it("asks for relations and claims", () => {
		expect(claimsDocument("e")).toContain(".relations = true, .claims = true")
	})

	it("parses the claim columns, `-` meaning no claim, and takes the statement id's last 32 digits", () => {
		const s = "a".repeat(32)
		const out = `p1\tv\t${s}\te1\t-\t-\np2\tv\t${"b".repeat(32)}:${s}\te1\tprop\ttrue\n`
		expect(parseClaimRows(out)).toEqual([
			{prop: "p1", statement: s, entity: "e1", claimProp: null, claimRaw: null},
			{prop: "p2", statement: s, entity: "e1", claimProp: "prop", claimRaw: "true"},
		])
	})
})

describe("a redelivered move keeps its (statement id, replacement id) pair", () => {
	// geo-db's set_position is idempotent only for the same pair. The feeder derives the replacement from its own
	// journal (the record's `current` and the wanted position), never from a read of what geo-db holds open.
	const op = (position: string): GrcOp => ({type: "updateRelation", id: rel(1).replaceAll("-", ""), position}) as GrcOp

	function seeded() {
		const s = store()
		s.commitMarks([], [sidOf(1)], new Map([[sidOf(1), {from: cid("entity", hex(1)), prop: typ, current: sidOf(1)}]]))
		return s
	}

	it("re-translating a message the journals have not yet absorbed gives the same intents", () => {
		const s = seeded()
		const run = () => translateOps([op("b1")], new Overlay(s, SPACE), {positions: true}).intents
		expect(run()).toEqual(run())
		expect(run()[0]).toContain(`.replacement = "${cid("relation-position", `${sidOf(1)}b1`)}"`)
	})

	it("a replay after the journal committed (crash before markDone) is clamped by the saved progress, not re-applied", () => {
		const s = seeded()
		const first = new Overlay(s, SPACE)
		const a = translateOps([op("b1")], first, {positions: true}).intents
		first.commit()
		// The replay now sees the moved record, so it would name a different pair ...
		const b = translateOps([op("b1")], new Overlay(s, SPACE), {positions: true}).intents
		expect(b[0]).not.toBe(a[0])
		// ... but it has the same length, and handleMessage resumes at min(progress, length) = length: nothing is applied.
		expect(b.length).toBe(a.length)
		expect(Math.min(a.length, b.length)).toBe(b.length)
	})
})
