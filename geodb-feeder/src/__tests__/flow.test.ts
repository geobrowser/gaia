import {mkdtempSync, readFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {applyIntents} from "../apply.js"
import {classify} from "../geodb.js"
import {type Decode, handleMessage} from "../handle.js"
import {createMetrics} from "../metrics.js"
import {ensureSpace} from "../spaces.js"
import {Overlay, Store, renderSpaceMap} from "../state.js"
import type {GrcOp} from "../translate.js"

const GAIA = "a19c345ab9866679b001d7d2138d88a1"
const GEO = "daf0bc83ca744e1a9ee0377967846846"
const id = (n: number) => new Uint8Array(Array.from({length: 16}, (_, i) => (i === 0 ? n : i === 6 ? 0x40 : i === 8 ? 0x80 : i)))
const hex = (n: number) => Buffer.from(id(n)).toString("hex")

const SEALER = "5".repeat(64)
const tmp = () => mkdtempSync(join(tmpdir(), "feeder-"))

type Result = {rc: number | null; out: string; err: string}
const ok: Result = {rc: 0, out: "", err: ""}

function fakeDb(script: (zon: string, call: number) => Result | Promise<Result>) {
	const applied: string[] = []
	const founds: {sealer: string; nonce: string}[] = []
	let calls = 0
	const spaces = new Set<string>()
	return {
		applied,
		founds,
		get calls() {
			return calls
		},
		db: {
			apply: async (zon: string) => {
				const r = await script(zon, calls++)
				if (r.rc === 0) applied.push(zon)
				return r
			},
			waitReady: async () => [...spaces],
			found: async (sealer: string, nonce: string) => {
				founds.push({sealer, nonce})
				spaces.add(GEO)
				return GEO
			},
		},
	}
}

const base = (store: Store, fake: ReturnType<typeof fakeDb>) => ({
	db: fake.db,
	sealer: async () => SEALER,
	store,
	metrics: createMetrics(),
	beat: async () => {},
	sleep: async () => {},
	publish: () => {},
})

const intents = (n: number) => Array.from({length: n}, (_, i) => `.{ .create_entity = "e${i}" }`)

describe("classify", () => {
	it("separates resource exhaustion, outages and verdicts", () => {
		expect(classify("error: ResourceExhausted")).toBe("exhausted")
		expect(classify("error: ConnectionRefused")).toBe("transient")
		expect(classify("error: FileNotFound")).toBe("transient")
		expect(classify("error: StreamTooLong")).toBe("permanent")
		expect(classify("error: InvalidIntent")).toBe("permanent")
	})
})

describe("applyIntents", () => {
	const space = {geo: GEO, nonce: "n", keyed: false}

	it("chunks, sends the founding key once, and reports progress", async () => {
		const store = new Store(tmp())
		const fake = fakeDb(() => ok)
		const done: number[] = []
		const sp = {...space}
		const r = await applyIntents(base(store, fake), sp, {space: GAIA, partition: 0, offset: "1"}, intents(5), 2, 0, (n) => done.push(n))
		expect(r).toEqual({applied: 5, deadLettered: 0})
		expect(done).toEqual([2, 4, 5])
		expect(fake.applied[0]).toContain('.key = "n"')
		expect(fake.applied[1]).not.toContain(".key")
		expect(sp.keyed).toBe(true)
	})

	it("resumes from the stored progress", async () => {
		const store = new Store(tmp())
		const fake = fakeDb(() => ok)
		await applyIntents(base(store, fake), {...space, keyed: true}, {space: GAIA, partition: 0, offset: "1"}, intents(5), 2, 4, () => {})
		expect(fake.applied).toHaveLength(1)
		expect(fake.applied[0]).toContain('"e4"')
		expect(fake.applied[0]).not.toContain('"e3"')
	})

	it("retries an outage and an exhausted serve forever instead of skipping", async () => {
		const store = new Store(tmp())
		const fake = fakeDb((_, call) =>
			call === 0 ? {rc: 1, out: "", err: "ConnectionRefused"} : call === 1 ? {rc: 1, out: "", err: "ResourceExhausted"} : ok,
		)
		const r = await applyIntents(base(store, fake), {...space, keyed: true}, {space: GAIA, partition: 0, offset: "1"}, intents(2), 10, 0, () => {})
		expect(r.applied).toBe(2)
	})

	it("bisects a refused chunk down to the one bad intent and dead-letters only that", async () => {
		const dir = tmp()
		const store = new Store(dir)
		const fake = fakeDb((zon) => (zon.includes('"e3"') ? {rc: 1, out: "", err: "InvalidIntent"} : ok))
		const r = await applyIntents(base(store, fake), {...space, keyed: true}, {space: GAIA, partition: 0, offset: "7"}, intents(6), 6, 0, () => {})
		expect(r).toEqual({applied: 5, deadLettered: 1})
		const line = JSON.parse(readFileSync(`${dir}/errors.jsonl`, "utf8").trim())
		expect(line).toMatchObject({offset: "7", intent: '.{ .create_entity = "e3" }'})
	})
})

describe("ensureSpace", () => {
	it("founds a new Space once, journals it, and publishes the map", async () => {
		const store = new Store(tmp())
		const fake = fakeDb(() => ok)
		const published: string[] = []
		const deps = {...base(store, fake), publish: (t: string) => published.push(t)}
		const a = await ensureSpace(deps, GAIA)
		const b = await ensureSpace(deps, GAIA)
		expect(a.geo).toBe(GEO)
		expect(b).toBe(a)
		expect(fake.founds).toHaveLength(1)
		expect(fake.founds[0]?.sealer).toBe(SEALER)
		expect(JSON.parse(published[0] ?? "")).toEqual({[GAIA]: {geo: GEO}})
	})

	it("recovers a founding that crashed before the Space id was saved", async () => {
		const dir = tmp()
		const first = new Store(dir)
		first.spaces[GAIA] = {geo: null, nonce: "pending", keyed: false}
		first.saveSpaces()
		const store = new Store(dir)
		const fake = fakeDb(() => ok)
		const rec = await ensureSpace(base(store, fake), GAIA)
		expect(rec.geo).toBe(GEO)
		expect(rec.nonce).toBe("pending")
	})

	it("does not found again for a Space it already holds", async () => {
		const store = new Store(tmp())
		store.spaces[GAIA] = {geo: GEO, nonce: "n", keyed: true}
		const fake = fakeDb(() => ok)
		await ensureSpace(base(store, fake), GAIA)
		expect(fake.founds).toEqual([])
	})

	it("surfaces a refused founding and keeps the record for the retry", async () => {
		const store = new Store(tmp())
		const fake = fakeDb(() => ok)
		fake.db.found = async () => {
			throw new Error("found: refused: NotFoundOrRestricted")
		}
		await expect(ensureSpace(base(store, fake), GAIA)).rejects.toThrow(/NotFoundOrRestricted/)
		expect(store.spaces[GAIA]?.geo).toBeNull()
	})
})

describe("state", () => {
	it("keeps journal marks away from a message until it completes", () => {
		const dir = tmp()
		const store = new Store(dir)
		const o = new Overlay(store, GAIA)
		o.markEntity("e1")
		o.markStmt("s1")
		expect(o.knownEntity("e1")).toBe(true)
		expect(new Store(dir).hasEntity(GAIA, "e1")).toBe(false)
		o.commit()
		const reopened = new Store(dir)
		expect(reopened.hasEntity(GAIA, "e1")).toBe(true)
		expect(reopened.hasStatement("s1")).toBe(true)
	})

	it("persists offsets and in-message progress", () => {
		const dir = tmp()
		const store = new Store(dir)
		expect(store.hadOffsets).toBe(false)
		store.saveProgress(0, 10, 400)
		store.markDone(0, 9)
		store.saveProgress(0, 10, 400)
		const re = new Store(dir)
		expect(re.hadOffsets).toBe(true)
		expect(re.lastDone(0)).toBe(9)
		expect(re.progress(0, 10)).toBe(400)
		expect(re.progress(0, 11)).toBe(0)
		re.markDone(0, 10)
		expect(new Store(dir).progress(0, 10)).toBe(0)
	})

	it("renders only founded Spaces, in the shape the api parses", () => {
		const text = renderSpaceMap({[GAIA]: {geo: GEO, nonce: "n", keyed: true}, [GEO]: {geo: null, nonce: "m", keyed: false}})
		expect(JSON.parse(text)).toEqual({[GAIA]: {geo: GEO}})
	})
})

describe("handleMessage", () => {
	// A HermesEdit with field 3 (payload) = [1] and field 6 (space) = GAIA.
	const message = (offset: string) => ({
		offset,
		value: Uint8Array.from([0x1a, 1, 1, 0x32, 16, ...Buffer.from(GAIA, "hex")]),
	})
	const ops: GrcOp[] = [
		{type: "createEntity", id: id(1), values: [{property: id(2), value: {type: "integer", value: 1}}]},
		{type: "createRelation", id: id(3), from: id(1), to: id(4), relationType: id(5)},
	]
	const decode: Decode = async () => ({ops})
	const deps = (store: Store, fake: ReturnType<typeof fakeDb>, d: Decode = decode) => ({...base(store, fake), decode: d, chunk: 200, signal: undefined})

	it("applies a message and journals it", async () => {
		const store = new Store(tmp())
		const fake = fakeDb(() => ok)
		expect(await handleMessage(deps(store, fake), 0, message("5"))).toBe("applied")
		expect(fake.applied).toHaveLength(1)
		expect(store.hasEntity(GAIA, hex(1))).toBe(true)
		expect(store.hasEntity(GAIA, hex(4))).toBe(false)
		expect(store.hasStatement(hex(3))).toBe(true)
	})

	it("a replay after a crash mid-message resumes at the stored progress", async () => {
		const store = new Store(tmp())
		// The message translates to 3 intents (create_entity, set, add); a crashed attempt got 2 of them in.
		store.saveProgress(0, 5, 2)
		const fake = fakeDb(() => ok)
		await handleMessage({...deps(store, fake), chunk: 1}, 0, message("5"))
		expect(fake.applied).toHaveLength(1)
		expect(fake.applied[0]).toContain(".add")
		expect(store.hasStatement(hex(3))).toBe(true)
	})

	it("dead-letters an undecodable message and moves on", async () => {
		const dir = tmp()
		const store = new Store(dir)
		const fake = fakeDb(() => ok)
		const bad: Decode = async () => {
			throw new Error("bad payload")
		}
		expect(await handleMessage(deps(store, fake, bad), 0, message("6"))).toBe("dead_letter")
		expect(fake.calls).toBe(0)
		expect(readFileSync(`${dir}/errors.jsonl`, "utf8")).toContain("bad payload")
	})

	it("dead-letters an edit for a Space geo-db cannot found as space_cap, and skips the founding after", async () => {
		const dir = tmp()
		const store = new Store(dir)
		const fake = fakeDb(() => ok)
		let asks = 0
		fake.db.found = async () => {
			asks++
			throw new Error("found n: rc=1 Refused: ResourceExhausted")
		}
		const d = deps(store, fake)
		expect(await handleMessage(d, 0, message("7"))).toBe("dead_letter")
		expect(await handleMessage(d, 0, message("8"))).toBe("dead_letter")
		expect(asks).toBe(1)
		expect(fake.calls).toBe(0)
		const lines = readFileSync(`${dir}/errors.jsonl`, "utf8").trim().split("\n").map((l) => JSON.parse(l))
		expect(lines).toHaveLength(2)
		expect(lines[1]).toMatchObject({kind: "space_cap", space: GAIA, offset: "8"})
	})

	it("a transient founding failure still throws, so the offset is retried", async () => {
		const store = new Store(tmp())
		const fake = fakeDb(() => ok)
		fake.db.found = async () => {
			throw new Error("found n: rc=1 ConnectionRefused")
		}
		await expect(handleMessage(deps(store, fake), 0, message("9"))).rejects.toThrow(/ConnectionRefused/)
	})

	it("treats a tombstone as empty", async () => {
		const store = new Store(tmp())
		expect(await handleMessage(deps(store, fakeDb(() => ok)), 0, {offset: "1", value: null})).toBe("empty")
	})
})

describe("SpaceMapPublisher", () => {
	it("sends the map and re-sends until the put lands", async () => {
		const {SpaceMapPublisher} = await import("../publish.js")
		const dir = mkdtempSync(join(tmpdir(), "pub-"))
		const sent: Record<string, string>[] = []
		let fail = true
		const put = async (d: Record<string, string>) => {
			if (fail) throw new Error("down")
			sent.push(d)
		}
		const pub = new SpaceMapPublisher(join(dir, "m.json"), put, createMetrics())
		pub.publish("{}")
		await pub.flush()
		expect(sent).toEqual([])
		fail = false
		await pub.flush()
		expect(sent.at(-1)).toEqual({"space-map.json": "{}"})
		const n = sent.length
		await pub.flush()
		expect(sent.length).toBe(n)
	})
})
