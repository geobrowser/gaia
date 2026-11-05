import {mkdtempSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {CREATED_AT_PROPERTY, UPDATED_AT_PROPERTY} from "../entityTimes.js"
import type {EntityReader, GeoDbRow} from "../geodbPipe.js"
import {cid} from "../ids.js"
import {createMetrics} from "../metrics.js"
import type {TimesSource} from "../pgSource.js"
import {Store} from "../state.js"
import {runTimesBackfill, secondsFromText, type TimesDeps, type TimesOptions} from "../timesBackfill.js"

const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const SPACE2 = "b29c345ab9866679b001d7d2138d88a2"
const GEO = "daf0bc83ca744e1a9ee0377967846846"
const GEO2 = "eaf0bc83ca744e1a9ee0377967846847"
const ent = (n: number) => `${n.toString(16).padStart(8, "0")}-0000-4000-8000-000000000000`
const hex = (n: number) => ent(n).replaceAll("-", "")
const OPTS: TimesOptions = {spaces: [], batch: 2, chunk: 200, pauseMs: 0, maxEntities: 0}

type Times = Record<string, {created_at: string | null; updated_at: string | null}>

function setup(times: Times, held: Record<string, GeoDbRow[]>, spaces: Record<string, string[]> = {[SPACE]: [1, 2, 3, 4, 5].map(ent)}) {
	const store = new Store(mkdtempSync(join(tmpdir(), "times-")))
	store.spaces[SPACE] = {geo: GEO, nonce: "n", keyed: true}
	store.spaces[SPACE2] = {geo: GEO2, nonce: "n", keyed: true}
	store.saveSpaces()
	const applied: {geo: string; zon: string}[] = []
	const source: TimesSource = {
		listEntities: async (s) => (spaces[s] ?? []).sort(),
		readTimes: async (ids) => ids.filter((i) => times[i]).map((i) => ({id: i, ...(times[i] as Times[string])})),
	}
	const reader: EntityReader & {reads: number} = {
		reads: 0,
		entityRows: async (geo, e) => {
			reader.reads++
			return held[`${geo}|${e}`] ?? []
		},
		reset: () => {},
	}
	const db = {
		waitReady: async () => [GEO],
		apply: async (zon: string) => {
			applied.push({geo: /\.space = "([0-9a-f]{32})"/.exec(zon)?.[1] ?? "", zon})
			return {rc: 0, out: "", err: ""}
		},
	}
	const deps: TimesDeps = {source, reader, db, store, metrics: createMetrics(), guard: async () => {}, sleep: async () => {}}
	return {deps, applied, reader, store}
}

const row = (e: string, prop: string): GeoDbRow => ({prop, value: "instant:1.000000000", statement: `s${e}${prop}`, entity: e})
const name = (n: number): GeoDbRow => ({prop: cid("entity", "11111111111141118111111111111111"), value: "x", statement: `n${n}`, entity: cid("entity", hex(n))})
const heldName = (n: number) => ({[`${GEO}|${cid("entity", hex(n))}`]: [name(n)]})

describe("secondsFromText", () => {
	it("takes unix seconds text only", () => {
		expect(secondsFromText("1793337008")).toBe(1793337008)
		expect(secondsFromText(" 5 ")).toBe(5)
		expect(secondsFromText(null)).toBeNull()
		expect(secondsFromText("2026-01-01")).toBeNull()
		expect(secondsFromText("-3")).toBeNull()
	})
})

describe("runTimesBackfill", () => {
	it("stamps both times, in microseconds, on an entity geo-db holds", async () => {
		const w = setup({[ent(1)]: {created_at: "1700000000", updated_at: "1793337008"}}, heldName(1), {[SPACE]: [ent(1)]})
		const stats = await runTimesBackfill(w.deps, OPTS)
		expect(stats).toMatchObject({entities: 1, createdWritten: 1, updatedWritten: 1, absent: 0})
		const e = cid("entity", hex(1))
		const all = w.applied.map((a) => a.zon).join("\n")
		expect(all).toContain(`.set = .{ .entity = "${e}", .property = "${CREATED_AT_PROPERTY}", .value = .{ .instant = "1700000000000000" } }`)
		expect(all).toContain(`.set = .{ .entity = "${e}", .property = "${UPDATED_AT_PROPERTY}", .value = .{ .instant = "1793337008000000" } }`)
		expect(w.applied[0]?.geo).toBe(GEO)
	})

	it("never overwrites a time geo-db holds, and writes only the missing one", async () => {
		const e = cid("entity", hex(1))
		const w = setup({[ent(1)]: {created_at: "1700000000", updated_at: "1793337008"}}, {[`${GEO}|${e}`]: [name(1), row(e, UPDATED_AT_PROPERTY)]}, {[SPACE]: [ent(1)]})
		const stats = await runTimesBackfill(w.deps, OPTS)
		expect(stats).toMatchObject({createdWritten: 1, updatedWritten: 0, alreadyHeld: 1})
		const all = w.applied.map((a) => a.zon).join("\n")
		expect(all).toContain(CREATED_AT_PROPERTY)
		expect(all).not.toContain(UPDATED_AT_PROPERTY)
	})

	it("skips an entity geo-db has no rows for, and one Postgres has no time for", async () => {
		const w = setup({[ent(1)]: {created_at: "1", updated_at: "2"}, [ent(2)]: {created_at: null, updated_at: "bad"}}, heldName(2), {[SPACE]: [ent(1), ent(2), ent(3)]})
		const stats = await runTimesBackfill(w.deps, OPTS)
		expect(stats).toMatchObject({entities: 3, absent: 1, noTime: 2, createdWritten: 0, updatedWritten: 0})
		expect(w.applied).toEqual([])
	})

	it("stamps the same value into every Space the entity is in", async () => {
		const e = cid("entity", hex(1))
		const w = setup(
			{[ent(1)]: {created_at: "10", updated_at: "20"}},
			{[`${GEO}|${e}`]: [name(1)], [`${GEO2}|${e}`]: [name(1)]},
			{[SPACE]: [ent(1)], [SPACE2]: [ent(1)]},
		)
		await runTimesBackfill(w.deps, OPTS)
		expect(w.applied.map((a) => a.geo).sort()).toEqual([GEO, GEO2].sort())
	})

	it("a rerun after it finished writes nothing and reads nothing", async () => {
		const held: Record<string, GeoDbRow[]> = {}
		for (let i = 1; i <= 5; i++) held[`${GEO}|${cid("entity", hex(i))}`] = [name(i)]
		const times: Times = {}
		for (let i = 1; i <= 5; i++) times[ent(i)] = {created_at: "10", updated_at: "20"}
		const w = setup(times, held)
		const first = await runTimesBackfill(w.deps, OPTS)
		expect(first.createdWritten).toBe(5)
		const reads = w.reader.reads
		const n = w.applied.length
		const again = await runTimesBackfill(w.deps, OPTS)
		expect(again.entities).toBe(0)
		expect(w.reader.reads).toBe(reads)
		expect(w.applied.length).toBe(n)
	})

	it("a rerun after a crash re-reads and writes each time once", async () => {
		const held: Record<string, GeoDbRow[]> = {}
		for (let i = 1; i <= 5; i++) held[`${GEO}|${cid("entity", hex(i))}`] = [name(i)]
		const times: Times = {}
		for (let i = 1; i <= 5; i++) times[ent(i)] = {created_at: "10", updated_at: "20"}
		const w = setup(times, held)
		// geo-db keeps what a first, interrupted run wrote: entity 1 already has both cells.
		const e1 = cid("entity", hex(1))
		held[`${GEO}|${e1}`] = [name(1), row(e1, CREATED_AT_PROPERTY), row(e1, UPDATED_AT_PROPERTY)]
		const stats = await runTimesBackfill(w.deps, OPTS)
		expect(stats.createdWritten).toBe(4)
		expect(stats.updatedWritten).toBe(4)
		expect(w.applied.map((a) => a.zon).join("\n")).not.toContain(`.entity = "${e1}"`)
	})

	it("honours the entity limit and a Space list", async () => {
		const held: Record<string, GeoDbRow[]> = {}
		for (let i = 1; i <= 5; i++) held[`${GEO}|${cid("entity", hex(i))}`] = [name(i)]
		const times: Times = {}
		for (let i = 1; i <= 5; i++) times[ent(i)] = {created_at: "10", updated_at: "20"}
		const w = setup(times, held)
		const limited = await runTimesBackfill(w.deps, {...OPTS, maxEntities: 2})
		expect(limited.stoppedAtLimit).toBe(true)
		expect(limited.entities).toBe(2)
		const none = await runTimesBackfill(setup(times, held).deps, {...OPTS, spaces: ["c39c345a-b986-6679-b001-d7d2138d88a3"]})
		expect(none.spaces).toBe(0)
	})
})
