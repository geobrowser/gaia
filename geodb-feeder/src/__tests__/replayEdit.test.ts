import {mkdtempSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {UPDATED_AT_PROPERTY} from "../entityTimes.js"
import type {EntityReader, GeoDbRow} from "../geodbPipe.js"
import {cid} from "../ids.js"
import {createMetrics} from "../metrics.js"
import {gatewayUrl, loadPayload, parseArgs, runReplayEdit, type ReplayEditArgs, type ReplayEditDeps} from "../replayEdit.js"
import {Overlay, Store} from "../state.js"
import {type GrcOp, translateOps} from "../translate.js"

const GAIA = "a19c345ab9866679b001d7d2138d88a1"
const GEO = "daf0bc83ca744e1a9ee0377967846846"
const id = (n: number) => new Uint8Array(Array.from({length: 16}, (_, i) => (i === 0 ? n : i === 6 ? 0x40 : i === 8 ? 0x80 : i)))
const ent = (n: number) => cid("entity", id(n))
const PROP = id(200)
const EDIT_SECONDS = 1_752_781_200
const micros = (s: number) => String(BigInt(s) * 1_000_000n)

const args = (over: Partial<ReplayEditArgs> = {}): ReplayEditArgs => ({
	uri: "ipfs://bafyexample",
	space: GAIA,
	block: 162685,
	sequence: 0,
	createdAt: EDIT_SECONDS,
	apply: false,
	...over,
})

const ops: GrcOp[] = [
	{type: "updateEntity", id: id(1), set: [{property: PROP, value: {type: "text", value: "renamed"}}]},
	{type: "createEntity", id: id(2), values: [{property: PROP, value: {type: "text", value: "new"}}]},
]

/** geo-db holds entity 1 (with the given updatedAt, if any) and nothing else. */
function world(updatedAt: string | null | "garbage") {
	const rows = new Map<string, GeoDbRow[]>()
	const held: GeoDbRow[] = [{prop: cid("entity", PROP), value: "old", statement: "s1", entity: ent(1)}]
	if (updatedAt !== null) held.push({prop: UPDATED_AT_PROPERTY, value: updatedAt === "garbage" ? "2025-01-01" : updatedAt, statement: "s2", entity: ent(1)})
	rows.set(`${GEO}|${ent(1)}`, held)
	const reads: string[] = []
	const reader: EntityReader = {
		entityRows: async (g, e) => {
			reads.push(e)
			return rows.get(`${g}|${e}`) ?? []
		},
		reset: () => {},
	}
	const applied: string[] = []
	const db = {
		apply: async (zon: string) => {
			applied.push(zon)
			return {rc: 0, out: "", err: ""}
		},
		waitReady: async () => [GEO],
	}
	const store = new Store(mkdtempSync(join(tmpdir(), "replay-edit-")))
	store.spaces[GAIA] = {geo: GEO, nonce: "n", keyed: true}
	const deps: ReplayEditDeps = {
		db,
		store,
		metrics: createMetrics(),
		beat: async () => {},
		sleep: async () => {},
		decode: async () => ({ops}),
		reader,
		spaceRecord: (s) => store.spaces[s],
		payload: new Uint8Array([1, 2, 3]),
		chunk: 200,
		entityTimes: true,
	}
	return {deps, store, applied, reads}
}

describe("replay-edit dry run", () => {
	it("translates and checks but writes nothing, journal included", async () => {
		const w = world(micros(EDIT_SECONDS - 60))
		const r = await runReplayEdit(w.deps, args(), w.store)
		expect(r.outcome).toBe("dry-run")
		if (r.outcome !== "dry-run") return
		expect(r.intents).toBeGreaterThan(0)
		expect(r.touched).toBe(2)
		expect(r.absent).toBe(1) // entity 2 is a creation
		expect(w.applied).toEqual([])
		expect(w.store.hasEntity(GAIA, ent(2))).toBe(false)
	})

	it("--apply writes the translated intents and journals them", async () => {
		const w = world(micros(EDIT_SECONDS - 60))
		const r = await runReplayEdit(w.deps, args({apply: true}), w.store)
		expect(r).toMatchObject({outcome: "applied", deadLettered: 0})
		expect(w.applied).toHaveLength(1)
		expect(w.applied[0]).toContain(`"${ent(1)}"`)
		expect(w.applied[0]).toContain(`"${ent(2)}"`)
		expect(w.store.hasEntity(GAIA, ent(2))).toBe(true)
	})

	it("an equal updatedAt is the edit itself and passes", async () => {
		const w = world(micros(EDIT_SECONDS))
		expect((await runReplayEdit(w.deps, args(), w.store)).outcome).toBe("dry-run")
	})
})

describe("replay-edit refusal", () => {
	it("refuses an edit older than a touched entity's updatedAt, even with --apply, and writes nothing", async () => {
		const w = world(micros(EDIT_SECONDS + 1))
		const r = await runReplayEdit(w.deps, args({apply: true}), w.store)
		expect(r.outcome).toBe("refused")
		if (r.outcome !== "refused") return
		expect(r.conflicts).toEqual([{entity: ent(1), reason: "newer", heldMicros: micros(EDIT_SECONDS + 1)}])
		expect(w.applied).toEqual([])
		expect(w.store.hasEntity(GAIA, ent(2))).toBe(false)
	})

	it("refuses an entity geo-db holds with no readable updatedAt", async () => {
		for (const held of [null, "garbage"] as const) {
			const w = world(held)
			const r = await runReplayEdit(w.deps, args({apply: true}), w.store)
			expect(r).toMatchObject({outcome: "refused", conflicts: [{entity: ent(1), reason: "no-updated-at"}]})
			expect(w.applied).toEqual([])
		}
	})

	it("refuses a Space geo-db does not hold, and founds nothing", async () => {
		const w = world(null)
		w.deps.spaceRecord = () => undefined
		const r = await runReplayEdit(w.deps, args({apply: true}), w.store)
		expect(r).toMatchObject({outcome: "refused", intents: 0})
		expect(w.applied).toEqual([])
		expect(w.reads).toEqual([])
	})
})

describe("replay-edit ISS-679-318 chunk check", () => {
	const EDIT = "742e60d91fc0490ea4cd3579bd19c72c"
	// Edit A made a positioned relation from entity 1 and applied; edit B moves it, then deletes entity 1: one chunk
	// holding relation_move;delete_entity on entity 1.
	const opsA: GrcOp[] = [{type: "createRelation", id: id(9), from: id(1), to: id(4), relationType: id(5), position: "a0"}]
	const opsB: GrcOp[] = [
		{type: "updateRelation", id: id(9), unset: [], position: "b0"},
		{type: "deleteEntity", id: id(1)},
	]

	async function afterA() {
		const w = world(micros(EDIT_SECONDS - 60))
		w.deps.positions = true
		w.deps.decode = async () => ({ops: opsA})
		expect(await runReplayEdit(w.deps, args({apply: true}), w.store)).toMatchObject({outcome: "applied"})
		w.applied.length = 0
		w.deps.decode = async () => ({ops: opsB})
		// What runReplayEdit will translate edit B to, without touching the journals.
		const intentsB = translateOps(opsB, new Overlay(w.store, GAIA), {positions: true, blockSeconds: EDIT_SECONDS}).intents
		const letters = (texts: string[]) =>
			texts.map((intent) => JSON.stringify({t: 1, partition: 0, offset: "98656", space: GAIA, edit: EDIT, idempotent: true, intent, err: "connect: Refused"})).join("\n")
		return {w, intentsB, letters}
	}

	it("the fixture really holds the pair", async () => {
		const {intentsB} = await afterA()
		expect(intentsB.some((i) => i.startsWith(".{ .set_position"))).toBe(true)
		expect(intentsB.some((i) => i.startsWith(".{ .delete_entity"))).toBe(true)
	})

	it("refuses --apply when the chunk already applied (nothing dead-lettered for the edit), says why, and writes nothing", async () => {
		const {w} = await afterA()
		w.deps.deadLetters = ""
		const r = await runReplayEdit(w.deps, args({apply: true, edit: EDIT}), w.store)
		expect(r.outcome).toBe("refused")
		if (r.outcome !== "refused") return
		expect(r.reason).toMatch(/1 chunk already applied and hold an ISS-679-318 pair: chunk \[0,\d+\) already applied .*relation_move;delete_entity/)
		expect(r.chunks).toMatchObject({chunks: 1, firstApply: 0, applied: 1, deadLetteredMatched: 0, deadLettersInRecord: 0})
		expect(r.chunks?.withPairs[0]?.pairs.map((p) => p.pair)).toEqual(["relation_move;delete_entity"])
		expect(w.applied).toEqual([])
	})

	it("refuses in the dry run too, so the operator sees it before stopping the feeder", async () => {
		const {w} = await afterA()
		w.deps.deadLetters = ""
		expect((await runReplayEdit(w.deps, args({edit: EDIT}), w.store)).outcome).toBe("refused")
	})

	it("refuses when a single intent of the chunk applied (a bisected chunk)", async () => {
		const {w, intentsB, letters} = await afterA()
		w.deps.deadLetters = letters(intentsB.slice(1))
		const r = await runReplayEdit(w.deps, args({apply: true, edit: EDIT}), w.store)
		expect(r).toMatchObject({outcome: "refused", chunks: {applied: 1, deadLetteredMatched: intentsB.length - 1}})
		expect(w.applied).toEqual([])
	})

	it("without --edit no chunk can be shown never applied, so a chunk with a pair refuses", async () => {
		const {w, intentsB, letters} = await afterA()
		w.deps.deadLetters = letters(intentsB)
		const r = await runReplayEdit(w.deps, args({apply: true}), w.store)
		expect(r.outcome).toBe("refused")
		if (r.outcome === "refused") expect(r.reason).toMatch(/no --edit given/)
	})

	it("passes, and applies, when every intent of the chunk was dead-lettered: a first apply", async () => {
		const {w, intentsB, letters} = await afterA()
		w.deps.deadLetters = letters(intentsB)
		const dry = await runReplayEdit(w.deps, args({edit: EDIT}), w.store)
		expect(dry).toMatchObject({outcome: "dry-run", chunks: {chunks: 1, firstApply: 1, applied: 0, deadLetteredMatched: intentsB.length}})
		const r = await runReplayEdit(w.deps, args({apply: true, edit: EDIT}), w.store)
		expect(r).toMatchObject({outcome: "applied", deadLettered: 0})
		expect(w.applied).toHaveLength(1)
	})

	it("ignores another edit's dead letters", async () => {
		const {w, intentsB, letters} = await afterA()
		w.deps.deadLetters = letters(intentsB)
		expect((await runReplayEdit(w.deps, args({edit: "0".repeat(32)}), w.store)).outcome).toBe("refused")
	})
})

describe("replay-edit arguments and payload", () => {
	const good = ["--uri", "ipfs://bafyexample", "--space", "a19c345a-b986-6679-b001-d7d2138d88a1", "--block", "7", "--created-at", "9"]

	it("parses the flags; dry run unless --apply", () => {
		expect(parseArgs(good)).toMatchObject({space: GAIA, block: 7, sequence: 0, createdAt: 9, apply: false})
		expect(parseArgs([...good, "--apply"]).apply).toBe(true)
		expect(parseArgs(good).edit).toBeUndefined()
		expect(parseArgs([...good, "--edit", "742E60D9-1FC0-490E-A4CD-3579BD19C72C"]).edit).toBe("742e60d91fc0490ea4cd3579bd19c72c")
		expect(() => parseArgs([...good, "--edit", "742e60d9"])).toThrow(/--edit/)
	})

	it("rejects missing, unknown, conflicting and placeholder arguments", () => {
		expect(() => parseArgs(good.slice(2))).toThrow(/--uri/)
		expect(() => parseArgs([...good, "--nope"])).toThrow(/unknown flag/)
		expect(() => parseArgs([...good, "--apply", "--dry-run"])).toThrow(/exclusive/)
		expect(() => parseArgs(good.map((a) => (a === "7" ? "__BLOCK__" : a)))).toThrow(/placeholder/)
		expect(() => parseArgs(good.filter((a) => a !== "--created-at" && a !== "9"))).toThrow(/--created-at/)
	})

	it("builds gateway URLs and refuses non-https gateways and non-ipfs URIs", () => {
		expect(gatewayUrl("ipfs://bafyx", "https://gw.example/ipfs/")).toBe("https://gw.example/ipfs/bafyx")
		expect(() => gatewayUrl("ipfs://bafyx", "http://gw.example/ipfs")).toThrow(/https/)
		expect(() => gatewayUrl("https://evil.example/x", "https://gw.example/ipfs")).toThrow(/ipfs:\/\//)
	})

	it("reads a file when given, else the gateway, and fails on a non-2xx answer", async () => {
		const io = {
			gateway: "https://gw.example/ipfs",
			readFile: () => new Uint8Array([9]),
			fetch: async (url: string) => ({ok: url.endsWith("/bafyx"), status: 404, arrayBuffer: async () => new Uint8Array([7, 7]).buffer as ArrayBuffer}),
		}
		expect(await loadPayload({uri: "ipfs://bafyx", file: "/x"}, io)).toEqual(new Uint8Array([9]))
		expect(await loadPayload({uri: "ipfs://bafyx"}, io)).toEqual(new Uint8Array([7, 7]))
		await expect(loadPayload({uri: "ipfs://bafyy"}, io)).rejects.toThrow(/404/)
	})
})
