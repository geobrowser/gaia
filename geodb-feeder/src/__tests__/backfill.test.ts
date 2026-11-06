import {appendFileSync, mkdtempSync, readFileSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {type BackfillDeps, type BackfillOptions, excludeFrom, parseScope, runBackfill} from "../backfill.js"
import {type EntityReader, type GeoDbRow, parseRows} from "../geodbPipe.js"
import {cid} from "../ids.js"
import {createMetrics} from "../metrics.js"
import {type OffsetsAdmin, checkOrdering} from "../ordering.js"
import {DERIVED_PROPERTIES, decimalValue, entityOps, grcValue, heldOf, type PgRelationRow, type PgValueRow, pointValue} from "../pgAdapter.js"
import type {BackfillSource} from "../pgSource.js"
import {Store, eachLine} from "../state.js"

const tmp = () => mkdtempSync(join(tmpdir(), "backfill-"))
const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const GEO = "daf0bc83ca744e1a9ee0377967846846"
const NEW_SPACE = "b29c345ab9866679b001d7d2138d88a2"
const NEW_GEO = "eaf0bc83ca744e1a9ee0377967846847"
// v4 ids pass through cid unchanged, which keeps the expectations readable.
const ent = (n: number) => `${n.toString(16).padStart(8, "0")}-0000-4000-8000-000000000000`
const hex = (n: number) => ent(n).replaceAll("-", "")
const PROP = "11111111-1111-4111-8111-111111111111"
const PROP2 = "22222222-2222-4222-8222-222222222222"
const RTYPE = "33333333-3333-4333-8333-333333333333"
const rel = (n: number) => `${(0x70000000 + n).toString(16)}-0000-4000-8000-000000000000`

const value = (entity: string, property: string, cols: Partial<PgValueRow>): PgValueRow => ({
	entity_id: entity,
	property_id: property,
	text: null,
	language: null,
	integer: null,
	float: null,
	decimal: null,
	boolean: null,
	datetime: null,
	date: null,
	point: null,
	...cols,
})

describe("pgAdapter", () => {
	it("maps each value column back to its GRC value", () => {
		expect(grcValue(value(ent(1), PROP, {text: "hi", language: "x"}))).toEqual({type: "text", value: "hi", language: "x"})
		expect(grcValue(value(ent(1), PROP, {integer: "-9007199254740993"}))).toEqual({type: "integer", value: -9007199254740993n})
		expect(grcValue(value(ent(1), PROP, {float: "1e+20"}))).toEqual({type: "float", value: 1e20})
		expect(grcValue(value(ent(1), PROP, {boolean: false}))).toEqual({type: "boolean", value: false})
		expect(grcValue(value(ent(1), PROP, {datetime: "2024-01-02T03:04:05Z"}))).toEqual({type: "datetime", value: "2024-01-02T03:04:05Z"})
		expect(grcValue(value(ent(1), PROP, {date: "2024-01"}))).toEqual({type: "date", value: "2024-01"})
		expect(grcValue(value(ent(1), PROP, {}))).toBeNull()
	})

	it("decimals keep scale, and oversized mantissas are marked big", () => {
		expect(decimalValue("-12.3400")).toEqual({type: "decimal", mantissa: {type: "i64", value: -123400n}, exponent: -4})
		expect(decimalValue("5")).toEqual({type: "decimal", mantissa: {type: "i64", value: 5n}, exponent: 0})
		expect(decimalValue("123456789012345678901234567890")?.mantissa).toEqual({type: "big", value: "123456789012345678901234567890"})
		expect(decimalValue("NaN")).toBeNull()
	})

	it("points are lat,lon[,alt] in Postgres", () => {
		expect(pointValue("51.5, -0.12")).toEqual({type: "point", lat: 51.5, lon: -0.12, alt: null})
		expect(pointValue("1,2,3")).toEqual({type: "point", lat: 1, lon: 2, alt: 3})
		expect(pointValue("1")).toBeNull()
		expect(pointValue("a,b")).toBeNull()
	})

	const rows = [value(ent(1), PROP, {text: "a"}), value(ent(1), PROP2, {integer: "3"})]
	const rels: PgRelationRow[] = [{id: rel(1), from_entity_id: ent(1), to_entity_id: ent(2), type_id: RTYPE}]

	it("an unknown entity gets everything", () => {
		const r = entityOps(ent(1), rows, rels, null, new Set())
		expect(r.ops.map((o) => o.type)).toEqual(["createEntity", "createRelation"])
		expect(r.values).toBe(2)
		expect(r.relations).toBe(1)
	})

	it("a relation's position rides along when Postgres has one", () => {
		const row = (n: number, position: string | null): PgRelationRow => ({id: rel(n), from_entity_id: ent(1), to_entity_id: ent(2), type_id: RTYPE, position})
		const positioned = [row(1, "a0"), row(2, null)]
		const r = entityOps(ent(1), rows, positioned, null, new Set())
		expect(r.ops.filter((o) => o.type === "createRelation").map((o) => o.position)).toEqual(["a0", undefined])
	})

	it("a known entity gets only what geo-db lacks", () => {
		const held = {props: new Set([cid("entity", PROP.replaceAll("-", ""))]), statements: new Set([cid("statement", rel(1).replaceAll("-", ""))])}
		const r = entityOps(ent(1), rows, rels, held, new Set())
		expect(r.ops).toEqual([{type: "updateEntity", id: hex(1), set: [{property: PROP2.replaceAll("-", ""), value: {type: "integer", value: 3n}}]}])
		const full = {props: new Set([cid("entity", PROP.replaceAll("-", "")), cid("entity", PROP2.replaceAll("-", ""))]), statements: held.statements}
		expect(entityOps(ent(1), rows, rels, full, new Set()).ops).toEqual([])
	})

	it("a relation's type does not mark a value on the same property as held", () => {
		const prop = PROP.replaceAll("-", "")
		const both = [value(ent(1), RTYPE, {text: ""})]
		const geoRel = [{prop: cid("entity", RTYPE.replaceAll("-", "")), statement: cid("statement", rel(1).replaceAll("-", ""))}]
		const held = heldOf(geoRel, rels)
		expect(held.props.size).toBe(0)
		expect(held.statements.size).toBe(1)
		expect(entityOps(ent(1), both, rels, held, new Set()).ops).toEqual([
			{type: "updateEntity", id: hex(1), set: [{property: RTYPE.replaceAll("-", ""), value: {type: "text", value: "", language: null}}]},
		])
		// a row whose statement is no Postgres relation of the entity still marks its property held
		const value1 = [{prop: cid("entity", prop), statement: cid("statement", "ab".repeat(16))}]
		expect(heldOf(value1, rels).props.has(cid("entity", prop))).toBe(true)
	})

	it("excludes derived properties and relation types by default", () => {
		const derived = DERIVED_PROPERTIES[0] as string
		const r = entityOps(
			ent(1),
			[value(ent(1), derived, {float: "1"})],
			[{id: rel(2), from_entity_id: ent(1), to_entity_id: ent(2), type_id: DERIVED_PROPERTIES[2] as string}],
			null,
			excludeFrom(undefined),
		)
		expect(r.excluded).toBe(2)
		expect(r.ops).toEqual([{type: "createEntity", id: hex(1), values: []}])
		expect(excludeFrom("none").size).toBe(0)
	})
})

describe("parseScope", () => {
	it("accepts mapped, all, and space lists; refuses the placeholder", () => {
		expect(parseScope("mapped")).toEqual({kind: "mapped"})
		expect(parseScope("all")).toEqual({kind: "all"})
		expect(parseScope(`41e85161-0e13-a194-41c4-d980f2f2ce6b, ${SPACE}`)).toEqual({kind: "list", spaces: ["41e851610e13a19441c4d980f2f2ce6b", SPACE]})
		expect(() => parseScope("__SPACES__")).toThrow()
		expect(() => parseScope("")).toThrow()
	})
})

describe("eachLine", () => {
	it("streams lines across chunk boundaries", () => {
		const p = join(tmp(), "f.log")
		const lines = Array.from({length: 40_000}, (_, i) => `${SPACE}|${i.toString(16).padStart(32, "0")}`)
		writeFileSync(p, `${lines.join("\n")}\n`)
		const seen: string[] = []
		eachLine(p, (l) => seen.push(l))
		expect(seen).toEqual(lines)
	})
})

describe("parseRows", () => {
	it("reads geodb-client's tab-separated rows", () => {
		expect(parseRows("p\tv\ts\te\t1\n\np2\tv2\ts2\te2\t1\n")).toEqual([
			{prop: "p", value: "v", statement: "s", entity: "e"},
			{prop: "p2", value: "v2", statement: "s2", entity: "e2"},
		])
	})
})

describe("checkOrdering", () => {
	const admin = (opts: {state?: string; members?: number; committed?: number[]; high?: number[]}): OffsetsAdmin => ({
		fetchTopicOffsets: async () => (opts.high ?? [100, 100]).map((h, partition) => ({partition, low: "0", high: String(h)})),
		fetchOffsets: async () => [{topic: "t", partitions: (opts.committed ?? [100, 100]).map((o, partition) => ({partition, offset: String(o)}))}],
		describeGroups: async (ids) => ({groups: [{groupId: ids[0] as string, state: opts.state ?? "Empty", members: Array(opts.members ?? 0).fill({})}]}),
	})
	const store = (done: Record<number, number>, inflight: Record<string, {offset: number; intentsDone: number}> = {}) => ({
		lastDone: (p: number) => done[p] ?? null,
		inflight,
	})
	const run = (a: OffsetsAdmin, s: ReturnType<typeof store>) =>
		checkOrdering({admin: a, topic: "t", feederGroup: "f", indexerGroup: "i", store: s})

	it("passes when the feeder is stopped between messages and the indexer is ahead", async () => {
		expect((await run(admin({}), store({0: 99, 1: 50}))).ok).toBe(true)
	})
	it("refuses while the feeder runs", async () => {
		const c = await run(admin({state: "Stable", members: 1}), store({0: 99}))
		expect(c.ok).toBe(false)
		expect(c.problems[0]).toMatch(/scale geodb-feeder to 0/)
	})
	it("refuses a feeder stopped inside a message", async () => {
		expect((await run(admin({}), store({0: 9}, {"0": {offset: 10, intentsDone: 3}}))).ok).toBe(false)
	})
	it("refuses when the indexer is behind the feeder", async () => {
		const c = await run(admin({committed: [99, 100]}), store({0: 99}))
		expect(c.ok).toBe(false)
		expect(c.problems[0]).toMatch(/partition 0/)
	})
})

// --- the backfill core, against fakes of Postgres, geo-db and its reader ---

type World = {values: PgValueRow[]; relations: PgRelationRow[]}

function fakeSource(world: Record<string, World>): BackfillSource & {batches: number} {
	const src = {
		batches: 0,
		spaceSizes: async () => new Map(Object.entries(world).map(([s, w]) => [s, w.values.length + w.relations.length])),
		listEntities: async (space: string) => {
			const w = world[space] ?? {values: [], relations: []}
			return [...new Set([...w.values.map((v) => v.entity_id), ...w.relations.map((r) => r.from_entity_id)])].sort()
		},
		readBatch: async (space: string, entities: string[]) => {
			src.batches++
			const w = world[space] ?? {values: [], relations: []}
			const want = new Set(entities)
			return {values: w.values.filter((v) => want.has(v.entity_id)), relations: w.relations.filter((r) => want.has(r.from_entity_id))}
		},
	}
	return src
}

/** geo-db as a set of rows per (geo space, entity), filled by parsing the intents the backfill applies. */
function fakeGeo(opts: {failAfter?: number} = {}) {
	const rows = new Map<string, GeoDbRow[]>()
	const created = new Set<string>()
	const applied: string[] = []
	let calls = 0
	const db = {
		apply: async (zon: string) => {
			if (opts.failAfter !== undefined && calls >= opts.failAfter) throw new Error("killed")
			calls++
			applied.push(zon)
			const geo = /\.space = "([0-9a-f]{32})"/.exec(zon)?.[1] ?? GEO
			for (const m of zon.matchAll(/\.create_entity = "([0-9a-f]+)"/g)) created.add(m[1] as string)
			for (const m of zon.matchAll(/\.set = \.{ \.entity = "([0-9a-f]+)", \.property = "([0-9a-f]+)", \.value = (.*?) } }/g)) {
				const k = `${geo}|${m[1]}`
				rows.set(k, [...(rows.get(k) ?? []), {prop: m[2] as string, value: m[3] as string, statement: `v${m[1]}${m[2]}`, entity: m[1] as string}])
			}
			for (const m of zon.matchAll(/\.add = \.{ \.id = "([0-9a-f]+)", \.entity = "([0-9a-f]+)", \.property = "([0-9a-f]+)"/g)) {
				const k = `${geo}|${m[2]}`
				rows.set(k, [...(rows.get(k) ?? []), {prop: m[3] as string, value: "ref", statement: m[1] as string, entity: m[2] as string}])
			}
			return {rc: 0, out: "", err: ""}
		},
		waitReady: async () => [GEO, NEW_GEO],
		found: async () => NEW_GEO,
	}
	const reader: EntityReader & {reads: number; resets: number} = {
		reads: 0,
		resets: 0,
		entityRows: async (geoSpace, entity) => {
			reader.reads++
			return rows.get(`${geoSpace}|${entity}`) ?? []
		},
		reset: () => {
			reader.resets++
		},
	}
	return {db, reader, rows, created, applied}
}

const OPTS: BackfillOptions = {scope: {kind: "mapped"}, spaceCap: 10, batch: 2, chunk: 200, pauseMs: 0, exclude: new Set(), maxEntities: 0}

function deps(store: Store, source: BackfillSource, geo: ReturnType<typeof fakeGeo>, published: string[] = []): BackfillDeps {
	return {
		source,
		reader: geo.reader,
		db: geo.db,
		store,
		metrics: createMetrics(),
		sealer: async () => "5".repeat(64),
		publish: (t) => published.push(t),
		guard: async () => {},
		sleep: async () => {},
	}
}

function world(n: number): World {
	const values: PgValueRow[] = []
	const relations: PgRelationRow[] = []
	for (let i = 1; i <= n; i++) {
		values.push(value(ent(i), PROP, {text: `name ${i}`}))
		values.push(value(ent(i), PROP2, {integer: String(i)}))
		relations.push({id: rel(i), from_entity_id: ent(i), to_entity_id: ent(i + 1), type_id: RTYPE})
	}
	return {values, relations}
}

const mappedStore = () => {
	const store = new Store(tmp())
	store.spaces[SPACE] = {geo: GEO, nonce: "n", keyed: true}
	store.saveSpaces()
	return store
}

describe("runBackfill", () => {
	it("writes every unknown entity and journals it", async () => {
		const store = mappedStore()
		const geo = fakeGeo()
		const stats = await runBackfill(deps(store, fakeSource({[SPACE]: world(5)}), geo), OPTS)
		expect(stats.entities).toBe(5)
		expect(stats.unknownEntities).toBe(5)
		expect(stats.values).toBe(10)
		expect(stats.relations).toBe(5)
		for (let i = 1; i <= 5; i++) expect(store.hasEntity(SPACE, hex(i))).toBe(true)
		expect(geo.reader.reads).toBe(0)
	})

	it("never overwrites a property or relation geo-db already holds", async () => {
		const store = mappedStore()
		const geo = fakeGeo()
		// entity 1 already came from the live stream with a newer name, and its relation.
		store.commitMarks([`${SPACE}|${hex(1)}`], [cid("statement", rel(1).replaceAll("-", ""))])
		geo.rows.set(`${GEO}|${hex(1)}`, [
			{prop: cid("entity", PROP.replaceAll("-", "")), value: "newer", statement: "s", entity: hex(1)},
			{prop: cid("entity", RTYPE.replaceAll("-", "")), value: "ref", statement: cid("statement", rel(1).replaceAll("-", "")), entity: hex(1)},
		])
		const stats = await runBackfill(deps(store, fakeSource({[SPACE]: world(2)}), geo), OPTS)
		expect(stats.knownEntities).toBe(1)
		expect(stats.knownEntitiesFilled).toBe(1)
		const all = geo.applied.join("\n")
		expect(all).not.toContain(`.create_entity = "${hex(1)}"`)
		expect(all).not.toContain(`.entity = "${hex(1)}", .property = "${cid("entity", PROP.replaceAll("-", ""))}"`)
		expect(all).toContain(`.entity = "${hex(1)}", .property = "${cid("entity", PROP2.replaceAll("-", ""))}"`)
		expect(all).not.toContain(`.id = "${cid("statement", rel(1).replaceAll("-", ""))}"`)
	})

	it("a rerun after it finished writes nothing", async () => {
		const store = mappedStore()
		const geo = fakeGeo()
		const src = fakeSource({[SPACE]: world(5)})
		await runBackfill(deps(store, src, geo), OPTS)
		const n = geo.applied.length
		const again = await runBackfill(deps(new Store(store.dir), src, geo), OPTS)
		expect(again.entities).toBe(0)
		expect(geo.applied.length).toBe(n)
	})

	it("resumes after a crash mid-batch without writing anything twice", async () => {
		const store = mappedStore()
		const src = fakeSource({[SPACE]: world(5)})
		// Batch 1 (entities 1-2) is 8 intents, 3 applies at chunk 3; batch 2's second apply is killed.
		const geo = fakeGeo({failAfter: 4})
		await expect(runBackfill(deps(store, src, geo), {...OPTS, chunk: 3})).rejects.toThrow("killed")
		// Batch 2 applied its first chunk (3 intents) to geo-db, then died: nothing of it is journaled.
		const cursor = JSON.parse(readFileSync(`${store.dir}/backfill/cursor.json`, "utf8"))
		expect(cursor.done[SPACE]).toBe(2)
		expect(cursor.inflight).toEqual({space: SPACE, from: 2, to: 4})

		const resumed = fakeGeo()
		for (const [k, v] of geo.rows) resumed.rows.set(k, v)
		const reopened = new Store(store.dir)
		const stats = await runBackfill(deps(reopened, src, resumed), {...OPTS, chunk: 3})
		expect(stats.entities).toBe(3)
		// The suspect batch's entities were read back first; the half-applied one is not created twice.
		expect(resumed.reader.reads).toBe(2)
		const written = [...geo.applied, ...resumed.applied].join("\n")
		for (let i = 1; i <= 5; i++) {
			expect(written.split(`.create_entity = "${hex(i)}"`).length - 1).toBe(1)
			expect(reopened.hasEntity(SPACE, hex(i))).toBe(true)
		}
		const nameSets = written.split(`, .property = "${cid("entity", PROP.replaceAll("-", ""))}", .value`).length - 1
		expect(nameSets).toBe(5)
	})

	it("founds new Spaces largest first, up to the cap, and publishes the map", async () => {
		const store = mappedStore()
		const geo = fakeGeo()
		const published: string[] = []
		const third = "c39c345ab9866679b001d7d2138d88a3"
		const src = fakeSource({[SPACE]: world(1), [NEW_SPACE]: world(3), [third]: world(2)})
		const stats = await runBackfill(deps(store, src, geo, published), {...OPTS, scope: {kind: "all"}, spaceCap: 2})
		expect(stats.spacesFounded).toBe(1)
		expect(stats.spacesSkippedCap).toEqual([third])
		expect(store.spaces[NEW_SPACE]?.geo).toBe(NEW_GEO)
		expect(published.at(-1)).toContain(NEW_SPACE)
		expect(geo.reader.resets).toBe(1)
	})

	it("stops when the guard says the feeder is back, and at the entity limit", async () => {
		const store = mappedStore()
		const geo = fakeGeo()
		const d = deps(store, fakeSource({[SPACE]: world(5)}), geo)
		let n = 0
		d.guard = async () => {
			if (++n > 1) throw new Error("feeder back")
		}
		await expect(runBackfill(d, OPTS)).rejects.toThrow("feeder back")
		const limited = await runBackfill(deps(new Store(store.dir), fakeSource({[SPACE]: world(5)}), geo), {...OPTS, maxEntities: 2})
		expect(limited.stoppedAtLimit).toBe(true)
		expect(limited.entities).toBe(2)
	})
})

describe("journal compatibility", () => {
	it("the feeder's Store reads what the backfill journaled", async () => {
		const store = mappedStore()
		appendFileSync(`${store.dir}/known-entities.log`, "")
		await runBackfill(deps(store, fakeSource({[SPACE]: world(3)}), fakeGeo()), OPTS)
		const fresh = new Store(store.dir)
		expect(fresh.entityCount).toBe(3)
		expect(fresh.statementCount).toBe(3)
	})
})
