import {existsSync, mkdtempSync, rmSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {type HeldRepairDeps, type HeldRepairOptions, runHeldRepair} from "../heldRepair.js"
import type {EntityReader, GeoDbRow} from "../geodbPipe.js"
import {cid} from "../ids.js"
import {createMetrics} from "../metrics.js"
import type {PgRelationRow, PgValueRow} from "../pgAdapter.js"
import type {HeldRepairSource} from "../pgSource.js"
import {Store} from "../state.js"

const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const GEO = "daf0bc83ca744e1a9ee0377967846846"
const OTHER = "b29c345ab9866679b001d7d2138d88a2"
const ent = (n: number) => `${n.toString(16).padStart(8, "0")}-0000-4000-8000-000000000000`
const hex = (n: number) => ent(n).replaceAll("-", "")
const RTYPE = "33333333-3333-4333-8333-333333333333"
const PROP = "11111111-1111-4111-8111-111111111111"
const rel = (n: number) => `${(0x70000000 + n).toString(16)}-0000-4000-8000-000000000000`
const col = (entity: string, property: string, text: string | null): PgValueRow => ({
	entity_id: entity,
	property_id: property,
	text,
	language: null,
	integer: null,
	float: null,
	decimal: null,
	boolean: null,
	datetime: null,
	date: null,
	point: null,
})
const relRow = (n: number): PgRelationRow => ({id: rel(n), from_entity_id: ent(n), to_entity_id: ent(99), type_id: RTYPE})
const relGeo = (n: number): GeoDbRow => ({
	prop: cid("entity", RTYPE.replaceAll("-", "")),
	value: "ref",
	statement: cid("statement", rel(n).replaceAll("-", "")),
	entity: hex(n),
})

// 1: value "" + relation of the same type, geo-db holds only the relation (the bug). 2: geo-db holds the value too.
// 3: nothing in geo-db and not journaled. 4: a plain value on another property: not a candidate pair's concern.
const values = [col(ent(1), RTYPE, ""), col(ent(2), RTYPE, "x"), col(ent(3), RTYPE, "y"), col(ent(1), PROP, "name")]
const relations = [relRow(1), relRow(2), relRow(3)]

function source(extra: {space: string; entity: string}[] = []): HeldRepairSource & {reads: number} {
	const src = {
		reads: 0,
		listCandidates: async () => [1, 2, 3].map((n) => ({space: SPACE, entity: ent(n)})).concat(extra),
		readBatch: async (_s: string, ids: string[]) => {
			src.reads++
			const want = new Set(ids)
			return {values: values.filter((v) => want.has(v.entity_id)), relations: relations.filter((r) => want.has(r.from_entity_id))}
		},
	}
	return src
}

function world() {
	const rows = new Map<string, GeoDbRow[]>([
		[`${GEO}|${hex(1)}`, [relGeo(1)]],
		[`${GEO}|${hex(2)}`, [relGeo(2), {prop: cid("entity", RTYPE.replaceAll("-", "")), value: "x", statement: "v2", entity: hex(2)}]],
	])
	const applied: string[] = []
	const reader: EntityReader = {entityRows: async (g, e) => rows.get(`${g}|${e}`) ?? [], reset: () => {}}
	const db = {
		apply: async (zon: string) => {
			applied.push(zon)
			return {rc: 0, out: "", err: ""}
		},
		waitReady: async () => [GEO],
	}
	return {reader, db, applied}
}

const store = () => {
	const s = new Store(mkdtempSync(join(tmpdir(), "held-")))
	s.spaces[SPACE] = {geo: GEO, nonce: "n", keyed: true}
	s.saveSpaces()
	// entities 1 and 2 are journaled (the Space backfill wrote them), 3 is not.
	s.commitMarks([`${SPACE}|${hex(1)}`, `${SPACE}|${hex(2)}`], [])
	return s
}

const OPTS: HeldRepairOptions = {batch: 2, chunk: 200, pauseMs: 0, exclude: new Set(), maxEntities: 0, dryRun: false}
const mk = (s: Store, src: HeldRepairSource, w: ReturnType<typeof world>): HeldRepairDeps => ({
	source: src,
	reader: w.reader,
	db: w.db,
	store: s,
	metrics: createMetrics(),
	guard: async () => {},
	sleep: async () => {},
})

describe("runHeldRepair", () => {
	it("writes only the value geo-db lacks, never a relation, and creates nothing", async () => {
		const s = store()
		const w = world()
		const stats = await runHeldRepair(mk(s, source(), w), OPTS)
		expect(stats).toMatchObject({candidatePairs: 3, entities: 3, entitiesToRepair: 1, entitiesAbsent: 1, valuesSharing: 3, valuesAlreadyHeld: 1, valuesToWrite: 1, valuesWritten: 1})
		expect(w.applied.length).toBe(1)
		expect(w.applied[0]).toContain(hex(1))
		expect(w.applied[0]).not.toContain(hex(2))
		expect(w.applied[0]).not.toContain(hex(3))
		expect(w.applied[0]).not.toContain("create_entity")
		expect(w.applied[0]).not.toContain(".add =")
		// the unrelated value on a non-relation property is untouched
		expect(w.applied[0]).not.toContain(PROP.replaceAll("-", ""))
	})

	it("a dry run counts and writes nothing: no apply, no cursor, no list file", async () => {
		const s = store()
		const w = world()
		const stats = await runHeldRepair(mk(s, source(), w), {...OPTS, dryRun: true})
		expect(stats).toMatchObject({dryRun: true, entitiesToRepair: 1, valuesToWrite: 1, valuesWritten: 0})
		expect(w.applied).toEqual([])
		expect(existsSync(`${s.dir}/held-repair`)).toBe(false)
		expect(existsSync(`${s.dir}/backfill`)).toBe(false)
	})

	it("is idempotent: a rerun on the same cursor, and one after the cursor is gone, write nothing more", async () => {
		const s = store()
		const w = world()
		await runHeldRepair(mk(s, source(), w), OPTS)
		const again = await runHeldRepair(mk(new Store(s.dir), source(), w), OPTS)
		expect(again.entities).toBe(0)
		// the first run's value is now a geo-db row: a fresh cursor finds the property held
		w.reader.entityRows = async (g, e) =>
			e === hex(1)
				? [relGeo(1), {prop: cid("entity", RTYPE.replaceAll("-", "")), value: "", statement: "v1", entity: hex(1)}]
				: e === hex(2)
					? [relGeo(2), {prop: cid("entity", RTYPE.replaceAll("-", "")), value: "x", statement: "v2", entity: hex(2)}]
					: []
		rmSync(`${s.dir}/held-repair`, {recursive: true})
		const third = await runHeldRepair(mk(new Store(s.dir), source(), w), OPTS)
		expect(third).toMatchObject({entitiesToRepair: 0, valuesToWrite: 0, valuesWritten: 0})
		expect(w.applied.length).toBe(1)
	})

	it("leaves a Space geo-db does not have alone, keeps its own cursor dir, and honours the cap", async () => {
		const s = store()
		const w = world()
		const src = source([{space: OTHER, entity: ent(7)}])
		const capped = await runHeldRepair(mk(s, src, w), {...OPTS, batch: 1, maxEntities: 1})
		expect(capped).toMatchObject({stoppedAtLimit: true, entities: 1})
		expect(existsSync(`${s.dir}/held-repair/cursor.json`)).toBe(true)
		expect(existsSync(`${s.dir}/backfill`)).toBe(false)
		const rest = await runHeldRepair(mk(new Store(s.dir), src, w), {...OPTS, batch: 1})
		expect(rest).toMatchObject({entities: 2, spacesUnmapped: 1, entitiesUnmapped: 1})
	})
})
