import {describe, expect, it} from "vitest"
import type {GeoDbRow} from "../geodbPipe.js"
import {RECORD_COLUMNS} from "../recordKinds.js"
import {
	type ReplayDeadLettersDeps,
	countTags,
	groupDocuments,
	keyedFamily,
	parseArgs,
	parseDeadLetters,
	parseDone,
	replayKey,
	runReplayDeadLetters,
	selectEntries,
} from "../replayDeadLetters.js"
import {zonDoc} from "../translate.js"

// Synthetic ids only: the record's shape (fields, intent forms, per-document counts) follows the feeder's
// errors.jsonl of 2026-11-12 23:01Z, none of its payload.
const h = (n: number) => n.toString(16).padStart(32, "0")
const EDIT = h(0x742e)
const GAIA_SPACE = h(0xc9f2)
const GEO_GAIA = h(0x9001)
const GEO_GOV = h(0x9002)
const GEO_VOTES = h(0x9003)
const REFUSED = "error: Refused\nconnect: Refused\n"
const IOERROR = "--binding /geodb/binding.txt: connect: IoError\n"

const col = (kind: keyof typeof RECORD_COLUMNS, i: number): string => {
	const ids = Object.values(RECORD_COLUMNS[kind] as Record<string, string>)
	return ids[i % ids.length] as string
}
const add = (e: string, p: string, v: string, id: string) =>
	`.{ .add = .{ .id = "${id}", .entity = "${e}", .property = "${p}", .value = .{ .reference = "${v}" } } }`
const retract = (s: string) => `.{ .set_active = .{ .statement = "${s}", .active = false } }`
const activate = (s: string) => `.{ .set_active = .{ .statement = "${s}", .active = true } }`
const unset = (e: string, p: string) => `.{ .unset = .{ .entity = "${e}", .property = "${p}" } }`
const put = (id: string, e: string, p: string, v = "1") => `.{ .put = .{ .id = "${id}", .entity = "${e}", .property = "${p}", .value = .{ .instant = "${v}" } } }`
const create = (e: string) => `.{ .create_entity = "${e}" }`

type Line = Record<string, unknown>
const editLine = (offset: number, intent: string, err = REFUSED, t = 1_762_988_493_000): Line => ({
	t,
	partition: 0,
	offset: String(offset),
	space: GAIA_SPACE,
	edit: EDIT,
	idempotent: true,
	intent,
	err,
})
const keyedLine = (offset: number, label: string, intent: string, err = REFUSED): Line => ({t: 1_762_988_500_000, space: label, partition: 0, offset: String(offset), intent, err})

/** A keyed record of `n` intents in the order keyedRecord.ts writes them: create, key column (add + activate), puts. */
function keyedRecord(kind: keyof typeof RECORD_COLUMNS, seed: number, n: number): string[] {
	const e = h(seed)
	const out = [create(e), add(e, col(kind, 0), h(seed + 1), h(seed + 2)), activate(h(seed + 2))]
	for (let i = 0; out.length < n; i++) out.push(put(h(seed + 10 + i), e, col(kind, 1 + i)))
	return out.slice(0, n)
}

/** The 192 never-sent dead letters, plus the record's other lines: 28 IoError, two decode dead letters, a torn line. */
function realShapedRecord(): {text: string; editIntents: string[]} {
	const editIntents: string[] = []
	for (let i = 0; i < 84; i++) editIntents.push(add(h(0x1000 + (i % 20)), h(0x2000 + i), h(0x3000 + i), h(0x4000 + i)))
	for (let i = 0; i < 48; i++) editIntents.push(retract(h(0x5000 + i)))
	for (let i = 0; i < 8; i++) editIntents.push(unset(h(0x1000 + i), h(0x6000 + i)))
	const lines: Line[] = [
		{t: 1, partition: -1, offset: "5", space: GAIA_SPACE, edit: h(0xbad), err: "MalformedEdit"},
		{t: 2, partition: -1, offset: "6", space: GAIA_SPACE, edit: h(0xbad), err: "MalformedEdit"},
		...keyedRecord("proposalEscalation", 0x7000, 13).map((i) => keyedLine(165613, h(0xa1), i, IOERROR)),
		...keyedRecord("proposalEscalation", 0x7100, 13).map((i) => keyedLine(165615, h(0xa2), i, IOERROR)),
		...keyedRecord("proposalVersion", 0x7200, 2).map((i) => keyedLine(165616, h(0xa3), i, IOERROR)),
		...editIntents.map((i) => editLine(98656, i)),
		...keyedRecord("editMeta", 0x8000, 6).map((i) => keyedLine(98657, h(0x9168), i)),
		...keyedRecord("proposalStatus", 0x8100, 4).map((i) => keyedLine(172829, h(0x47a), i)),
		...keyedRecord("proposalEscalation", 0x8200, 13).map((i) => keyedLine(172830, h(0x47a), i)),
		...keyedRecord("proposalVote", 0x8300, 12).map((i) => keyedLine(172831, h(0x52c7), i)),
		...keyedRecord("proposalStatus", 0x8400, 4).map((i) => keyedLine(172832, h(0x1101), i)),
		...keyedRecord("proposalEscalation", 0x8500, 13).map((i) => keyedLine(172833, h(0x1101), i)),
	]
	return {text: `${lines.map((l) => JSON.stringify(l)).join("\n")}\n{"t": 3, "par`, editIntents}
}

type Sent = {text: string}
function harness(over: Partial<ReplayDeadLettersDeps> & {rows?: Record<string, GeoDbRow[]>; answers?: {rc: number; err: string}[]} = {}) {
	const sent: Sent[] = []
	const done: string[] = []
	const sleeps: number[] = []
	const reads: string[] = []
	const answers = [...(over.answers ?? [])]
	const deps: ReplayDeadLettersDeps = {
		record: realShapedRecord().text,
		done: "",
		spaces: (s) => (s === GAIA_SPACE ? {geo: GEO_GAIA, nonce: h(1), keyed: true} : undefined),
		system: (f) => (f === "governance" ? {geo: GEO_GOV, nonce: h(2), keyed: true} : f === "votes" ? {geo: GEO_VOTES, nonce: h(3), keyed: true} : undefined),
		reader: {
			entityRows: async (_g, e) => {
				reads.push(e)
				return over.rows?.[e] ?? []
			},
			reset: () => {},
		},
		db: {
			apply: async (text: string) => {
				sent.push({text})
				return {rc: 0, out: "", err: "", ...(answers.shift() ?? {})}
			},
		},
		appendDone: (t) => done.push(t),
		chunk: 200,
		sleep: async (ms) => {
			sleeps.push(ms)
		},
		now: () => 1_763_000_000_000,
		...over,
	}
	return {deps, sent, done, sleeps, reads}
}

const docCounts = (docs: {offset: string; pending: number}[]) => docs.map((d) => [d.offset, d.pending])

describe("parseDeadLetters", () => {
	it("keeps every entry with an intent, counts decode dead letters and torn lines, ids stable by line", () => {
		const {text} = realShapedRecord()
		const a = parseDeadLetters(text)
		expect(a.entries).toHaveLength(28 + 192)
		expect(a.noIntent).toBe(2)
		expect(a.notJson).toBe(1)
		expect(a.entries[0]?.line).toBe(3)
		expect(a.entries[0]?.id).toMatch(/^3:[0-9a-f]{16}$/)
		expect(parseDeadLetters(text).entries.map((e) => e.id)).toEqual(a.entries.map((e) => e.id))
		expect(a.entries.at(-1)?.edit).toBeNull()
		expect(a.entries.find((e) => e.offset === "98656")?.edit).toBe(EDIT)
	})
})

describe("selectEntries", () => {
	const {entries} = parseDeadLetters(realShapedRecord().text)
	it("keeps only the default allowlist's errors (connect: Refused): the 192", () => {
		const s = selectEntries(entries, {errors: ["connect: Refused"]})
		expect(s.selected).toHaveLength(192)
		expect(s.skipped).toEqual({"error-not-allowed": 28})
	})
	it("filters by edit, source, space and offsets", () => {
		const errors = ["connect: Refused"]
		expect(selectEntries(entries, {errors, edit: EDIT}).selected).toHaveLength(140)
		expect(selectEntries(entries, {errors, source: "keyed"}).selected).toHaveLength(52)
		expect(selectEntries(entries, {errors, source: "edit"}).selected).toHaveLength(140)
		expect(selectEntries(entries, {errors, space: h(0x47a)}).selected).toHaveLength(17)
		expect(selectEntries(entries, {errors, offsets: {from: 172829, to: 172831}}).selected).toHaveLength(29)
		expect(selectEntries(entries, {errors, partition: 1}).selected).toHaveLength(0)
	})
})

describe("groupDocuments", () => {
	it("one document per original message, in record order, entries in record order", () => {
		const {entries} = parseDeadLetters(realShapedRecord().text)
		const docs = groupDocuments(selectEntries(entries, {errors: ["connect: Refused"]}).selected)
		expect(docs.map((d) => [d.offset, d.source, d.entries.length])).toEqual([
			["98656", "edit", 140],
			["98657", "keyed", 6],
			["172829", "keyed", 4],
			["172830", "keyed", 13],
			["172831", "keyed", 12],
			["172832", "keyed", 4],
			["172833", "keyed", 13],
		])
		expect(docs[0]?.entries.map((e) => e.line)).toEqual([...docs[0]!.entries.map((e) => e.line)].sort((a, b) => a - b))
	})
})

describe("keyedFamily", () => {
	it("reads the family from the record columns", () => {
		expect(keyedFamily(keyedRecord("editMeta", 1, 6))).toEqual({family: "governance", kinds: ["editMeta"]})
		expect(keyedFamily(keyedRecord("proposalVote", 1, 12))).toEqual({family: "votes", kinds: ["proposalVote"]})
	})
	it("refuses a property that is no record column, a mix of families, and no column at all", () => {
		expect(keyedFamily([put(h(1), h(2), h(0xdead))])).toHaveProperty("error")
		expect(keyedFamily([...keyedRecord("proposalStatus", 1, 4), ...keyedRecord("proposalVote", 9, 4)])).toHaveProperty("error")
		expect(keyedFamily([create(h(1))])).toHaveProperty("error")
	})
})

describe("countTags", () => {
	it("splits set_active by value", () => {
		expect(countTags(realShapedRecord().editIntents)).toEqual({add: 84, set_active_false: 48, unset: 8})
	})
})

describe("runReplayDeadLetters dry run", () => {
	it("prints the 7 documents, their targets and counts, and writes nothing", async () => {
		const t = harness()
		const r = await runReplayDeadLetters(t.deps, {apply: false})
		expect(r.outcome).toBe("dry-run")
		expect(r.selected).toBe(192)
		expect(r.skipped).toEqual({"error-not-allowed": 28, "no-intent": 2, "not-json": 1})
		expect(docCounts(r.docs)).toEqual([
			["98656", 140],
			["98657", 6],
			["172829", 4],
			["172830", 13],
			["172831", 12],
			["172832", 4],
			["172833", 13],
		])
		expect(r.docs.map((d) => d.family)).toEqual([null, "governance", "governance", "governance", "votes", "governance", "governance"])
		expect(r.docs[0]?.tags).toEqual({add: 84, set_active_false: 48, unset: 8})
		expect(r.docs.every((d) => d.neverSent && d.pairs.length === 0)).toBe(true)
		// Without --edit/--created-at the edit document's state is unchecked; a dry run says so, --apply refuses it.
		expect(r.docs[0]?.check.kind).toBe("unchecked")
		expect(r.docs[1]?.check).toEqual({kind: "put-cells", cells: {absent: 3}, other: []})
		expect(t.sent).toHaveLength(0)
		expect(t.done).toHaveLength(0)
	})

	it("--error 'connect: IoError' adds the 28 IoError entries as three more governance documents", async () => {
		const t = harness()
		const r = await runReplayDeadLetters(t.deps, {apply: false, errors: ["connect: Refused", "connect: IoError"]})
		expect(r.selected).toBe(220)
		expect(docCounts(r.docs).slice(0, 3)).toEqual([
			["165613", 13],
			["165615", 13],
			["165616", 2],
		])
		expect(r.docs.slice(0, 3).every((d) => d.family === "governance" && d.neverSent)).toBe(true)
	})

	it("nothing matches: outcome nothing", async () => {
		const r = await runReplayDeadLetters(harness().deps, {apply: false, edit: h(0xffff)})
		expect(r.outcome).toBe("nothing")
	})

	it("refuses a document whose target is not mapped, and a keyed record of no known family", async () => {
		const t = harness({system: () => undefined})
		const r = await runReplayDeadLetters(t.deps, {apply: false, source: "keyed"})
		expect(r.outcome).toBe("refused")
		expect(r.docs.every((d) => d.state === "refused" && /not founded/.test(d.refuse ?? ""))).toBe(true)
		const odd = harness({record: `${JSON.stringify(keyedLine(9, h(0xa), put(h(1), h(2), h(0xdead))))}\n`})
		const r2 = await runReplayDeadLetters(odd.deps, {apply: false})
		expect(r2.docs[0]?.refuse).toMatch(/no record column/)
	})
})

describe("ISS-679-318 pairs", () => {
	const pairDoc = (err: string) =>
		[put(h(0x51), h(0x1001), h(0x6001)), unset(h(0x1001), h(0x6001))].map((i) => JSON.stringify(editLine(98656, i, err))).join("\n")
	it("a pair whose errors prove it was never sent is a first apply: reported, not refused", async () => {
		const t = harness({record: pairDoc(REFUSED)})
		const r = await runReplayDeadLetters(t.deps, {apply: false})
		expect(r.docs[0]?.pairs.map((p) => p.pair)).toEqual(["put;unset"])
		expect(r.docs[0]?.refuse).toBeNull()
	})
	it("a pair under an error that may have been sent refuses, dry run included", async () => {
		const t = harness({record: pairDoc("Timeout")})
		const r = await runReplayDeadLetters(t.deps, {apply: false, errors: ["Timeout"]})
		expect(r.outcome).toBe("refused")
		expect(r.docs[0]?.refuse).toMatch(/ISS-679-318/)
	})
})

describe("runReplayDeadLetters --apply", () => {
	it("refuses the whole run when the edit document's state is unchecked (no --created-at), sending nothing", async () => {
		const t = harness()
		const r = await runReplayDeadLetters(t.deps, {apply: true})
		expect(r.outcome).toBe("refused")
		expect(r.reason).toMatch(/98656.*--created-at/)
		expect(t.sent).toHaveLength(0)
	})

	it("refuses the edit document when an entity it names holds a newer updatedAt", async () => {
		const {UPDATED_AT_PROPERTY} = await import("../entityTimes.js")
		const rows = {[h(0x1003)]: [{prop: UPDATED_AT_PROPERTY, value: "1762988500000000", statement: h(1), entity: h(0x1003)}]}
		const t = harness({rows})
		const r = await runReplayDeadLetters(t.deps, {apply: true, edit: EDIT, createdAt: 1_762_988_400})
		expect(r.outcome).toBe("refused")
		expect(r.docs[0]?.check).toMatchObject({kind: "updated-at", conflicts: [{entity: h(0x1003), reason: "newer"}]})
		expect(t.sent).toHaveLength(0)
	})

	it("sends the exact dead-lettered texts, in record order, chunked, under a stable key; records each entry done", async () => {
		const t = harness({chunk: 100})
		const r = await runReplayDeadLetters(t.deps, {apply: true, edit: EDIT, createdAt: 1_762_988_400})
		expect(r.outcome).toBe("applied")
		expect(r.sent).toBe(140)
		const {editIntents} = realShapedRecord()
		const entries = parseDeadLetters(t.deps.record).entries.filter((e) => e.edit === EDIT)
		const doc = `0:98656:${GAIA_SPACE}:${EDIT}`
		expect(t.sent.map((s) => s.text)).toEqual([
			zonDoc(GEO_GAIA, replayKey(doc, entries.slice(0, 100).map((e) => e.id)), editIntents.slice(0, 100)),
			zonDoc(GEO_GAIA, replayKey(doc, entries.slice(100).map((e) => e.id)), editIntents.slice(100)),
		])
		expect(parseDone(t.done.join("")).size).toBe(140)
	})

	it("a rerun skips every entry the done-file records", async () => {
		const first = harness()
		await runReplayDeadLetters(first.deps, {apply: true, source: "keyed"})
		expect(first.sent).toHaveLength(6)
		const again = harness({done: first.done.join("")})
		const r = await runReplayDeadLetters(again.deps, {apply: true, source: "keyed"})
		expect(r.outcome).toBe("applied")
		expect(r.sent).toBe(0)
		expect(again.sent).toHaveLength(0)
		expect(r.docs.every((d) => d.state === "done" && d.pending === 0)).toBe(true)
	})

	describe("a rerun after a kill (found on a real geo-db store: the store stamps every entity the run wrote with a newer updatedAt)", () => {
		const newer = async (entities: string[]) => {
			const {UPDATED_AT_PROPERTY} = await import("../entityTimes.js")
			return Object.fromEntries(entities.map((e) => [e, [{prop: UPDATED_AT_PROPERTY, value: "1763000000000000", statement: h(1), entity: e}]]))
		}
		const firstChunkEntities = Array.from({length: 20}, (_, i) => h(0x1000 + i))
		const args = {apply: true, edit: EDIT, createdAt: 1_762_988_400}

		it("resumes after the entries the done-file records, though the entities they wrote are now newer than --created-at", async () => {
			const first = harness({chunk: 100, answers: [{rc: 0, err: ""}, {rc: 1, err: "refused: DefiniteRefusal Other\n"}]})
			const r1 = await runReplayDeadLetters(first.deps, args)
			expect(r1.outcome).toBe("failed")
			const again = harness({chunk: 100, done: first.done.join(""), rows: await newer(firstChunkEntities)})
			const r2 = await runReplayDeadLetters(again.deps, args)
			expect(r2.outcome).toBe("applied")
			expect(r2.sent).toBe(40)
		})

		it("resends the chunk that was in flight when the process died, under the same key", async () => {
			const first = harness({chunk: 100, answers: [{rc: 0, err: ""}, {rc: 1, err: "refused: DefiniteRefusal Other\n"}]})
			await runReplayDeadLetters(first.deps, args)
			// the kill landed after geo-db committed chunk 2 and before its done lines: only the started marker survived
			const lines = first.done.join("").split("\n").filter(Boolean)
			const inFlight = Array.from({length: 40}, (_, i) => h(0x1000 + (i % 20)))
			const again = harness({chunk: 100, done: `${lines.join("\n")}\n`, rows: await newer([...firstChunkEntities, ...inFlight])})
			const r2 = await runReplayDeadLetters(again.deps, args)
			expect(r2.outcome).toBe("applied")
			expect(r2.sent).toBe(40)
			const entries = parseDeadLetters(again.deps.record).entries.filter((e) => e.edit === EDIT)
			expect(again.sent[0]?.text).toContain(replayKey(`0:98656:${GAIA_SPACE}:${EDIT}`, entries.slice(100).map((e) => e.id)))
		})
	})

	it("keyed records go to their family's system Space, verbatim", async () => {
		const t = harness()
		await runReplayDeadLetters(t.deps, {apply: true, source: "keyed"})
		expect(t.sent.map((s) => /\.space = "([0-9a-f]+)"/.exec(s.text)?.[1])).toEqual([GEO_GOV, GEO_GOV, GEO_GOV, GEO_VOTES, GEO_GOV, GEO_GOV])
		expect(t.sent[4]?.text).toContain(keyedRecord("proposalStatus", 0x8400, 4).join(",\n "))
	})

	it("refuses a keyed put whose cell holds another value; a cell holding the same statement is fine", async () => {
		const e = h(0x8000)
		const same = {[e]: [{prop: col("editMeta", 1), value: "1", statement: h(0x8000 + 10), entity: e}]}
		const ok = await runReplayDeadLetters(harness({rows: same}).deps, {apply: false, offsets: {from: 98657, to: 98657}})
		expect(ok.docs[0]?.check).toEqual({kind: "put-cells", cells: {"held-same": 1, absent: 2}, other: []})
		expect(ok.outcome).toBe("dry-run")
		const other = {[e]: [{prop: col("editMeta", 2), value: "9", statement: `${h(0x77)}`, entity: e}]}
		const t = harness({rows: other})
		const r = await runReplayDeadLetters(t.deps, {apply: true, offsets: {from: 98657, to: 98657}})
		expect(r.outcome).toBe("refused")
		expect(r.docs[0]?.refuse).toMatch(/another value/)
		expect(t.sent).toHaveLength(0)
	})

	it("stops, sending nothing, when a keyed record's state cannot be read", async () => {
		const t = harness()
		t.deps.reader = {
			entityRows: async () => {
				throw new Error("pipe closed")
			},
			reset: () => {},
		}
		await expect(runReplayDeadLetters(t.deps, {apply: true, source: "keyed"})).rejects.toThrow(/pipe closed/)
		expect(t.sent).toHaveLength(0)
	})

	it("retries a failed connect, then sends", async () => {
		const t = harness({answers: [{rc: 1, err: REFUSED}]})
		const r = await runReplayDeadLetters(t.deps, {apply: true, offsets: {from: 172829, to: 172829}})
		expect(r.outcome).toBe("applied")
		expect(t.sent).toHaveLength(2)
		expect(t.sent[0]?.text).toBe(t.sent[1]?.text)
		expect(t.sleeps).toEqual([1000])
	})

	it("stops at a permanent refusal: earlier documents stay done, the rest pending, nothing dead-lettered", async () => {
		const t = harness({answers: [{rc: 0, err: ""}, {rc: 1, err: "error: InvalidIntent\n"}]})
		const r = await runReplayDeadLetters(t.deps, {apply: true, source: "keyed"})
		expect(r.outcome).toBe("failed")
		expect(r.sent).toBe(6)
		expect(parseDone(t.done.join("")).size).toBe(6)
		const again = harness({done: t.done.join("")})
		const r2 = await runReplayDeadLetters(again.deps, {apply: true, source: "keyed"})
		expect(r2.sent).toBe(46)
		expect(again.sent).toHaveLength(5)
	})
})

describe("replayKey and parseDone", () => {
	it("a UUIDv4-shaped key that depends on the document and the entries", () => {
		const k = replayKey("d", ["1:a", "2:b"])
		expect(k).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
		expect(replayKey("d", ["1:a", "2:b"])).toBe(k)
		expect(replayKey("d", ["1:a"])).not.toBe(k)
		expect(replayKey("e", ["1:a", "2:b"])).not.toBe(k)
	})
	it("ignores a torn last line", () => {
		expect([...parseDone(`{"entry":"1:a"}\n{"entry":"2:b"}\n{"ent`)]).toEqual(["1:a", "2:b"])
	})
})

describe("parseArgs", () => {
	it("defaults to a dry run on the default allowlist", () => {
		expect(parseArgs([])).toEqual({apply: false, errors: ["connect: Refused"]})
	})
	it("reads every filter; `-` means not given", () => {
		expect(
			parseArgs(["--edit", "742e60d9-0000-0000-0000-000000000000", "--created-at", "1762988400", "--source", "edit", "--offsets", "98656", "--error", "connect: IoError", "--apply"]),
		).toEqual({
			apply: true,
			edit: "742e60d9000000000000000000000000",
			createdAt: 1762988400,
			source: "edit",
			offsets: {from: 98656, to: 98656},
			errors: ["connect: Refused", "connect: IoError"],
		})
		expect(parseArgs(["--source", "keyed", "--edit", "-", "--created-at", "-", "--dry-run"])).toEqual({apply: false, source: "keyed", errors: ["connect: Refused"]})
	})
	it("refuses placeholders, unknown flags, both modes, --created-at without --edit", () => {
		expect(() => parseArgs(["--edit", "__EDIT__"])).toThrow(/placeholder/)
		expect(() => parseArgs(["--force"])).toThrow(/unknown/)
		expect(() => parseArgs(["--apply", "--dry-run"])).toThrow(/exclusive/)
		expect(() => parseArgs(["--created-at", "5"])).toThrow(/--edit/)
		expect(() => parseArgs(["--source", "both"])).toThrow(/--source/)
		expect(() => parseArgs(["--edit"])).toThrow(/needs a value/)
	})
})
