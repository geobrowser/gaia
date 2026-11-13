/**
 * replay-deadletters: send the feeder's dead-lettered intents again, verbatim, from its own record (`errors.jsonl` on
 * the state volume, `Store.deadLetter`). For dead letters that never reached geo-db: serve was unreachable, and the
 * feeder of the time skipped the intent instead of retrying (#1566 retries that class from now on).
 *
 * Why verbatim and not re-translated. The edits consumer commits its overlay (the journals of known entities,
 * statements and relations) after `applyIntents` returns, also when every intent of the message was dead-lettered
 * (handle.ts). So the journals already claim what the dead-lettered intents would have created, and a re-translation
 * of the same edit (replay-edit) skips them as creates of known relations. The intent texts in the record are what
 * the consumer meant to send; this sends exactly those, in the record's order.
 *
 * What a run does:
 * - reads the record, keeps the entries that carry an intent and whose error is on the allowlist (default
 *   `connect: Refused`), narrowed by `--edit`, `--space`, `--offsets`, `--partition` and `--source`;
 * - groups them per original document (partition, offset, Space label, edit), in the record's order;
 * - resolves each document's target geo-db Space: an edits-consumer entry (it names its `edit`) goes to the gaia
 *   Space's geo-db Space (`spaces.json`); a keyed record (no `edit`; the `space` field is a label such as a proposal
 *   or edit id) goes to its family's system Space, the family read from the record columns its intents write
 *   (recordKinds.ts);
 * - checks each document's intents for geo-db ISS-679-318's six non-idempotent pairs (replayHazard.ts);
 * - reads the target's current state (read only): for an edit document, the `updatedAt` of every entity it names
 *   against `--created-at` (replayEdit.ts `findConflicts`); for a keyed record, the cell of every `put`;
 * - prints one line per document, and with `--apply` sends the pending entries and records each sent entry in a
 *   durable done-file, so a rerun skips it.
 *
 * Read-only on the record: nothing here appends to `errors.jsonl`. A refused apply stops the run; it is not bisected
 * or dead-lettered again.
 */
import {createHash} from "node:crypto"
import {blockInstant} from "./entityTimes.js"
import {type GeoDb, classify} from "./geodb.js"
import type {EntityReader, GeoDbRow} from "./geodbPipe.js"
import {readWithRetry} from "./geodbPipe.js"
import {RECORD_COLUMNS, RECORD_KINDS, type SystemFamily} from "./recordKinds.js"
import {type Conflict, findConflicts} from "./replayEdit.js"
import {type PairHit, findHazardPairs} from "./replayHazard.js"
import {parseRange} from "./replayKeyed.js"
import {zonDoc} from "./translate.js"

/** Errors that mean the intent never reached geo-db: the client could not connect, so nothing was sent. */
export const NEVER_SENT_ERRORS: readonly string[] = ["connect: Refused", "connect: IoError"]

/** The default allowlist: the 2026-11-12 23:01Z event's error. */
export const DEFAULT_ERRORS: readonly string[] = ["connect: Refused"]

export type DeadLetterEntry = {
	/** 1-based line number in the record (append-only, so stable). */
	line: number
	/** `<line>:<sha256 of the line, 16 hex>`: what the done-file records. */
	id: string
	t: number | null
	partition: number
	offset: string
	/** The meta `space` field: a gaia Space id for the edits consumer, a label for a keyed record. */
	space: string
	edit: string | null
	intent: string
	err: string
}

const sha = (s: string) => createHash("sha256").update(s).digest("hex")

export type ParsedRecord = {entries: DeadLetterEntry[]; lines: number; notJson: number; noIntent: number}

/** Every entry of the record that carries an intent. A decode dead letter (no intent) and a non-JSON line are counted. */
export function parseDeadLetters(jsonl: string): ParsedRecord {
	const entries: DeadLetterEntry[] = []
	let lines = 0
	let notJson = 0
	let noIntent = 0
	const all = jsonl.split("\n")
	for (let i = 0; i < all.length; i++) {
		const raw = all[i] as string
		if (!raw.trim()) continue
		lines++
		let r: Record<string, unknown>
		try {
			r = JSON.parse(raw)
		} catch {
			notJson++
			continue
		}
		if (typeof r.intent !== "string" || typeof r.space !== "string") {
			noIntent++
			continue
		}
		entries.push({
			line: i + 1,
			id: `${i + 1}:${sha(raw).slice(0, 16)}`,
			t: typeof r.t === "number" ? r.t : null,
			partition: typeof r.partition === "number" ? r.partition : Number(r.partition),
			offset: String(r.offset),
			space: r.space,
			edit: typeof r.edit === "string" ? r.edit : null,
			intent: r.intent,
			err: typeof r.err === "string" ? r.err : "",
		})
	}
	return {entries, lines, notJson, noIntent}
}

export type Source = "edit" | "keyed"

export type Filter = {
	edit?: string
	space?: string
	partition?: number
	offsets?: {from: number; to: number}
	source?: Source
	/** Substrings of `err`; an entry is kept when its error holds one. */
	errors: readonly string[]
}

export const sourceOf = (e: Pick<DeadLetterEntry, "edit">): Source => (e.edit === null ? "keyed" : "edit")

export type Selection = {selected: DeadLetterEntry[]; skipped: Record<string, number>}

/** The entries the filter keeps, in record order, and how many each rule dropped. */
export function selectEntries(entries: readonly DeadLetterEntry[], f: Filter): Selection {
	const skipped: Record<string, number> = {}
	const skip = (why: string) => {
		skipped[why] = (skipped[why] ?? 0) + 1
	}
	const selected: DeadLetterEntry[] = []
	for (const e of entries) {
		if (!f.errors.some((x) => e.err.includes(x))) skip("error-not-allowed")
		else if (f.edit !== undefined && e.edit !== f.edit) skip("edit")
		else if (f.space !== undefined && e.space !== f.space) skip("space")
		else if (f.partition !== undefined && e.partition !== f.partition) skip("partition")
		else if (f.offsets !== undefined && !(/^\d+$/.test(e.offset) && Number(e.offset) >= f.offsets.from && Number(e.offset) <= f.offsets.to)) skip("offset")
		else if (f.source !== undefined && sourceOf(e) !== f.source) skip("source")
		else selected.push(e)
	}
	return {selected, skipped}
}

export type DeadLetterDoc = {
	/** `<partition>:<offset>:<space label>:<edit or ->`. */
	id: string
	source: Source
	partition: number
	offset: string
	space: string
	edit: string | null
	entries: DeadLetterEntry[]
}

/** One document per original message, in order of first appearance; entries keep the record's order. */
export function groupDocuments(entries: readonly DeadLetterEntry[]): DeadLetterDoc[] {
	const docs = new Map<string, DeadLetterDoc>()
	for (const e of entries) {
		const id = `${e.partition}:${e.offset}:${e.space}:${e.edit ?? "-"}`
		let d = docs.get(id)
		if (!d) {
			d = {id, source: sourceOf(e), partition: e.partition, offset: e.offset, space: e.space, edit: e.edit, entries: []}
			docs.set(id, d)
		}
		d.entries.push(e)
	}
	return [...docs.values()]
}

/** Record column property id -> its kind's name, built from recordKinds.ts. */
const COLUMN_KIND = (() => {
	const m = new Map<string, string>()
	for (const [kind, cols] of Object.entries(RECORD_COLUMNS)) for (const id of Object.values(cols as Record<string, string>)) m.set(id, kind)
	return m
})()

const propertyOf = (intent: string): string | null => /\.property\s*=\s*"([0-9a-f]{32})"/.exec(intent)?.[1] ?? null

/**
 * The system family a keyed record's intents write, from the record columns they name. Every `add` and `put` must
 * name a column of one known kind's family; anything else is an error, never a guess.
 */
export function keyedFamily(intents: readonly string[]): {family: SystemFamily; kinds: string[]} | {error: string} {
	const kinds = new Set<string>()
	for (const i of intents) {
		const p = propertyOf(i)
		if (p === null) continue
		const kind = COLUMN_KIND.get(p)
		if (!kind) return {error: `intent names property ${p}, which is no record column of recordKinds.ts`}
		kinds.add(kind)
	}
	if (kinds.size === 0) return {error: "no intent names a record column, so the family is unknown"}
	const families = new Set([...kinds].map((k) => (RECORD_KINDS as Record<string, {family: SystemFamily}>)[k]?.family))
	if (families.size !== 1 || families.has(undefined as never)) return {error: `record columns of more than one family: ${[...kinds].join(", ")}`}
	return {family: [...families][0] as SystemFamily, kinds: [...kinds].sort()}
}

/** An intent's tag, with `set_active` split by its value: what the per-document counts are keyed by. */
export function intentTag(intent: string): string {
	const head = /^\.\{\s*\.(\w+)\s*=/.exec(intent)?.[1] ?? "unknown"
	if (head !== "set_active") return head
	return /\.active\s*=\s*false/.test(intent) ? "set_active_false" : "set_active_true"
}

export function countTags(intents: readonly string[]): Record<string, number> {
	const out: Record<string, number> = {}
	for (const i of intents) out[intentTag(i)] = (out[intentTag(i)] ?? 0) + 1
	return out
}

/**
 * The idempotency key of one apply: UUIDv4-shaped, from the document and the exact entries sent. A rerun after a crash
 * between geo-db's commit and the done-file write sends the same pending set under the same key and gets the first
 * apply's receipt.
 */
export function replayKey(docId: string, entryIds: readonly string[]): string {
	const h = createHash("sha256").update(`geodb-feeder:deadletter-replay:v1\0${docId}\0`).update(entryIds.join("\0")).digest().subarray(0, 16)
	h[6] = ((h[6] ?? 0) & 0x0f) | 0x40
	h[8] = ((h[8] ?? 0) & 0x3f) | 0x80
	return h.toString("hex")
}

/** The ids the done-file records. Lines that are not JSON (a torn last line) are ignored: that entry is resent. */
export function parseDone(text: string): Set<string> {
	const out = new Set<string>()
	for (const line of text.split("\n")) {
		if (!line.trim()) continue
		try {
			const r = JSON.parse(line) as {entry?: unknown}
			if (typeof r.entry === "string") out.add(r.entry)
		} catch {}
	}
	return out
}

/**
 * The entry ids a chunk was about to send when the process died: `startedLine` is appended before each apply. geo-db
 * may or may not have committed that chunk, so a rerun cannot tell its own write from a later one by reading; it
 * exempts these entries' entities from the newer-state rule (the resend is under the same key, so a committed chunk
 * answers its first receipt and writes nothing).
 */
export function parseStarted(text: string): Set<string> {
	const out = new Set<string>()
	for (const line of text.split("\n")) {
		if (!line.trim()) continue
		try {
			const r = JSON.parse(line) as {inflight?: unknown; entries?: unknown}
			if (r.inflight === true && Array.isArray(r.entries)) for (const id of r.entries) if (typeof id === "string") out.add(id)
		} catch {}
	}
	return out
}

export const startedLine = (doc: DeadLetterDoc, chunk: readonly DeadLetterEntry[], key: string, now: number): string =>
	`${JSON.stringify({t: now, inflight: true, doc: doc.id, key, entries: chunk.map((e) => e.id)})}\n`

export const doneLines = (doc: DeadLetterDoc, sent: readonly DeadLetterEntry[], key: string, now: number): string =>
	sent.map((e) => `${JSON.stringify({t: now, entry: e.id, doc: doc.id, key})}\n`).join("")

export type SpaceTarget = {geo: string | null; nonce: string; keyed: boolean}

export type Target = {geo: string; keyed: boolean; family: SystemFamily | null; kinds: string[]}

/** The geo-db Space a document is written to, or why it cannot be named. */
export function resolveTarget(
	doc: DeadLetterDoc,
	maps: {spaces: (space: string) => SpaceTarget | undefined; system: (family: SystemFamily) => SpaceTarget | undefined},
): Target | {error: string} {
	if (doc.source === "edit") {
		const rec = maps.spaces(doc.space)
		if (!rec?.geo) return {error: `space ${doc.space} is not mapped in spaces.json; this mode founds no Space`}
		return {geo: rec.geo, keyed: rec.keyed, family: null, kinds: []}
	}
	const fam = keyedFamily(doc.entries.map((e) => e.intent))
	if ("error" in fam) return fam
	const rec = maps.system(fam.family)
	if (!rec?.geo) return {error: `system space ${fam.family} is not founded (system-spaces.json); this mode founds no Space`}
	return {geo: rec.geo, keyed: rec.keyed, family: fam.family, kinds: fam.kinds}
}

export type PutCell = {entity: string; property: string; statement: string}
export type CellState = PutCell & {state: "absent" | "held-same" | "held-other"; held?: string[]}

const putCellOf = (intent: string): PutCell | null => {
	if (!/^\.\{\s*\.put\s*=/.test(intent)) return null
	const f = (n: string) => new RegExp(`\\.${n}\\s*=\\s*"([0-9a-f]{32})"`).exec(intent)?.[1] ?? ""
	return {entity: f("entity"), property: f("property"), statement: f("id")}
}

/**
 * The state of every cell a keyed record's `put`s write. A put's statement id includes its value (recordKinds.ts), so
 * a cell holding the put's own id already holds this value (`held-same`, resending changes nothing), and a cell that
 * holds another statement holds another value (`held-other`): a later message may have written it, and resending an
 * older value would revert it, so that refuses.
 */
export async function putCellStates(reader: EntityReader, geo: string, intents: readonly string[], sleep: (ms: number) => Promise<void>): Promise<CellState[]> {
	const rowsOf = new Map<string, GeoDbRow[]>()
	const out: CellState[] = []
	for (const i of intents) {
		const c = putCellOf(i)
		if (!c) continue
		let rows = rowsOf.get(c.entity)
		if (!rows) {
			rows = await readWithRetry(reader, geo, c.entity, sleep)
			rowsOf.set(c.entity, rows)
		}
		const inCell = rows.filter((r) => r.prop === c.property && (r.entity === "" || r.entity === c.entity)).map((r) => /([0-9a-f]{32})$/.exec(r.statement)?.[1] ?? r.statement)
		if (inCell.length === 0) out.push({...c, state: "absent"})
		else if (inCell.includes(c.statement)) out.push({...c, state: "held-same"})
		else out.push({...c, state: "held-other", held: inCell})
	}
	return out
}

/** The entities an edit document names (adds and unsets); a retract names only its statement. */
export const editEntities = (intents: readonly string[]): string[] => [
	...new Set(intents.map((i) => (/^\.\{\s*\.(add|unset)\s*=/.test(i) ? (/\.entity\s*=\s*"([0-9a-f]{32})"/.exec(i)?.[1] ?? null) : null)).filter((x): x is string => x !== null)),
]

export type DocPlan = {
	doc: string
	source: Source
	partition: number
	offset: string
	space: string
	edit: string | null
	family: SystemFamily | null
	kinds: string[]
	entries: number
	done: number
	pending: number
	tags: Record<string, number>
	/** Every error of the document is one that proves nothing was sent. */
	neverSent: boolean
	pairs: PairHit[]
	/** `pending`, `done`, or `refused`. */
	state: "pending" | "done" | "refused"
	check: {kind: "updated-at"; checked: number; absent: number; conflicts: Conflict[]} | {kind: "put-cells"; cells: Record<string, number>; other: CellState[]} | {kind: "unchecked"; why: string}
	refuse: string | null
}

export type ReplayDeadLettersArgs = Omit<Filter, "errors"> & {
	errors?: readonly string[]
	/** The edit's block time, unix seconds: the newer-state rule of an edit document. Only with `--edit`. */
	createdAt?: number
	apply: boolean
}

export type ReplayDeadLettersDeps = {
	/** The record's text (`errors.jsonl`). */
	record: string
	/** The done-file's text ("" when absent). */
	done: string
	spaces: (space: string) => SpaceTarget | undefined
	system: (family: SystemFamily) => SpaceTarget | undefined
	/** Read only; absent means no state check can run (dry run: unchecked; --apply: refused). */
	reader?: EntityReader
	db?: Pick<GeoDb, "apply">
	/** Appends to the done-file durably (fsync) before the next apply. */
	appendDone?: (text: string) => void
	chunk: number
	sleep?: (ms: number) => Promise<void>
	now?: () => number
	/** Transient failures retried per chunk before the run stops (default 5). */
	attempts?: number
	log?: (level: "info" | "warn" | "error", msg: string, fields: Record<string, unknown>) => void
}

export type ReplayDeadLettersResult = {
	outcome: "dry-run" | "applied" | "refused" | "failed" | "nothing"
	selected: number
	skipped: Record<string, number>
	docs: DocPlan[]
	sent: number
	reason?: string
}

/** Every entity an intent creates, deletes or names as its subject (set, put, add, unset, set_position). */
export const touchedEntities = (intents: readonly string[]): string[] => [
	...new Set(
		intents.flatMap((i) => {
			const created = /^\.\{\s*\.(?:create_entity|delete_entity)\s*=\s*"([0-9a-f]{32})"/.exec(i)?.[1]
			const subject = /\.entity\s*=\s*"([0-9a-f]{32})"/.exec(i)?.[1]
			return [created, subject].filter((x): x is string => x !== undefined)
		}),
	),
]

async function stateCheck(
	deps: ReplayDeadLettersDeps,
	args: ReplayDeadLettersArgs,
	doc: DeadLetterDoc,
	target: Target,
	intents: readonly string[],
	sleep: (ms: number) => Promise<void>,
	ours: ReadonlySet<string> = new Set(),
): Promise<DocPlan["check"]> {
	if (!deps.reader) return {kind: "unchecked", why: "no reader"}
	if (doc.source === "edit") {
		if (args.createdAt === undefined || args.edit === undefined) return {kind: "unchecked", why: "an edit document needs --edit and --created-at for the newer-state rule"}
		const micros = BigInt(blockInstant(args.createdAt) ?? "0")
		const r = await findConflicts(deps.reader, target.geo, editEntities(intents).filter((e) => !ours.has(e)), micros, sleep)
		return {kind: "updated-at", ...r}
	}
	const cells = await putCellStates(deps.reader, target.geo, intents, sleep)
	const counts: Record<string, number> = {}
	for (const c of cells) counts[c.state] = (counts[c.state] ?? 0) + 1
	return {kind: "put-cells", cells: counts, other: cells.filter((c) => c.state === "held-other")}
}

function refusalOf(p: Omit<DocPlan, "refuse" | "state">, target: Target | {error: string}, apply: boolean): string | null {
	if ("error" in target) return target.error
	if (!target.keyed) return `geo-db Space ${target.geo} has never been applied to (not keyed); this mode sends only under a key`
	if (p.pairs.length > 0 && !p.neverSent)
		return `holds ${[...new Set(p.pairs.map((h) => h.pair))].join(", ")} (ISS-679-318) and its errors do not prove it was never sent`
	if (p.check.kind === "updated-at" && p.check.conflicts.length > 0)
		return `${p.check.conflicts.length} entit${p.check.conflicts.length === 1 ? "y holds" : "ies hold"} a newer or unreadable updatedAt; resending would stomp later state`
	if (p.check.kind === "put-cells" && p.check.other.length > 0)
		return `${p.check.other.length} put cell${p.check.other.length === 1 ? " holds" : "s hold"} another value; resending would revert it`
	if (apply && p.check.kind === "unchecked") return `state unchecked (${p.check.why}); --apply needs the check`
	return null
}

/** Plan every selected document, and with `args.apply` send the pending ones. */
export async function runReplayDeadLetters(deps: ReplayDeadLettersDeps, args: ReplayDeadLettersArgs): Promise<ReplayDeadLettersResult> {
	const log = deps.log ?? (() => {})
	const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)))
	if (!(deps.chunk > 0)) throw new Error("chunk must be positive")
	if (args.createdAt !== undefined && args.edit === undefined) throw new Error("--created-at is the block time of one edit: give --edit with it")
	const errors = args.errors ?? DEFAULT_ERRORS
	const parsed = parseDeadLetters(deps.record)
	const {selected, skipped} = selectEntries(parsed.entries, {...args, errors})
	if (parsed.noIntent) skipped["no-intent"] = parsed.noIntent
	if (parsed.notJson) skipped["not-json"] = parsed.notJson
	const docs = groupDocuments(selected)
	const done = parseDone(deps.done)
	const started = parseStarted(deps.done)

	const plans: {doc: DeadLetterDoc; target: Target | {error: string}; plan: DocPlan}[] = []
	for (const doc of docs) {
		const target = resolveTarget(doc, deps)
		const pendingEntries = doc.entries.filter((e) => !done.has(e.id))
		const intents = doc.entries.map((e) => e.intent)
		const base = {
			doc: doc.id,
			source: doc.source,
			partition: doc.partition,
			offset: doc.offset,
			space: doc.space,
			edit: doc.edit,
			family: "error" in target ? null : target.family,
			kinds: "error" in target ? [] : target.kinds,
			entries: doc.entries.length,
			done: doc.entries.length - pendingEntries.length,
			pending: pendingEntries.length,
			tags: countTags(intents),
			neverSent: doc.entries.every((e) => NEVER_SENT_ERRORS.some((x) => e.err.includes(x))),
			// The whole document, as the original message held it: a pair across a done/pending split still counts.
			pairs: findHazardPairs(intents),
		}
		// Entities an earlier run of this tool wrote (or was writing when it died) now carry a newer updatedAt: its own work,
		// not later state to stomp. An entity only a later writer touched is still checked.
		const ours = new Set(touchedEntities(doc.entries.filter((e) => done.has(e.id) || started.has(e.id)).map((e) => e.intent)))
		const check: DocPlan["check"] =
			"error" in target || pendingEntries.length === 0
				? {kind: "unchecked", why: "error" in target ? "no target" : "nothing pending"}
				: await stateCheck(deps, args, doc, target, pendingEntries.map((e) => e.intent), sleep, ours)
		const refuse = pendingEntries.length === 0 ? null : refusalOf({...base, check}, target, args.apply)
		const plan: DocPlan = {...base, check, refuse, state: pendingEntries.length === 0 ? "done" : refuse ? "refused" : "pending"}
		plans.push({doc, target, plan})
		log(refuse ? "error" : "info", "replay-deadletters document", {...plan, pairs: plan.pairs.slice(0, 20)})
	}
	const result = (outcome: ReplayDeadLettersResult["outcome"], sent: number, reason?: string): ReplayDeadLettersResult => ({
		outcome,
		selected: selected.length,
		skipped,
		docs: plans.map((p) => p.plan),
		sent,
		...(reason ? {reason} : {}),
	})
	const summary = {
		lines: parsed.lines,
		selected: selected.length,
		skipped,
		documents: plans.length,
		pending: plans.reduce((n, p) => n + p.plan.pending, 0),
		done: plans.reduce((n, p) => n + p.plan.done, 0),
		refused: plans.filter((p) => p.plan.state === "refused").length,
	}
	log("info", "replay-deadletters plan", summary)

	if (selected.length === 0) return result("nothing", 0, "no dead letter matches the filter")
	const refused = plans.filter((p) => p.plan.state === "refused")
	if (refused.length > 0)
		return result("refused", 0, `${refused.length} document${refused.length === 1 ? "" : "s"} refused: ${refused.slice(0, 5).map((p) => `${p.plan.doc}: ${p.plan.refuse}`).join("; ")}`)
	if (!args.apply) return result("dry-run", 0)
	if (!deps.db || !deps.appendDone) throw new Error("--apply needs a geo-db writer and the done-file")

	const now = deps.now ?? Date.now
	const attempts = deps.attempts ?? 5
	let sent = 0
	for (const {doc, target, plan} of plans) {
		if (plan.state !== "pending" || "error" in target) continue
		const pending = doc.entries.filter((e) => !done.has(e.id))
		for (let lo = 0; lo < pending.length; lo += deps.chunk) {
			const slice = pending.slice(lo, lo + deps.chunk)
			const key = replayKey(doc.id, slice.map((e) => e.id))
			const text = zonDoc(target.geo, key, slice.map((e) => e.intent))
			deps.appendDone(startedLine(doc, slice, key, now()))
			let delay = 1000
			for (let attempt = 1; ; attempt++) {
				const r = await deps.db.apply(text)
				if (r.rc === 0) break
				const message = (r.err + r.out).slice(-600)
				// A failed connect sent nothing: retry it here even where this base's classify calls `connect: Refused` permanent.
				const kind = NEVER_SENT_ERRORS.some((x) => message.includes(x)) ? "transient" : classify(r.rc === null ? `${message} killed` : message)
				if (kind !== "transient" || attempt >= attempts) {
					log("error", "replay-deadletters apply refused", {doc: doc.id, lo, size: slice.length, kind, attempt, error: message.slice(0, 300)})
					return result("failed", sent, `${doc.id} [${lo},${lo + slice.length}): geo-db answered ${kind}: ${message.slice(0, 200)}`)
				}
				log("warn", "replay-deadletters apply failed, retrying", {doc: doc.id, kind, attempt, delayMs: delay})
				await sleep(delay)
				delay = Math.min(delay * 2, 30_000)
			}
			deps.appendDone(doneLines(doc, slice, key, now()))
			for (const e of slice) done.add(e.id)
			sent += slice.length
			log("info", "replay-deadletters sent", {doc: doc.id, lo, size: slice.length, key})
		}
	}
	return result("applied", sent)
}

const VALUE_FLAGS = ["--edit", "--space", "--partition", "--offsets", "--source", "--error", "--created-at"]
const hexId = (raw: string, flag: string): string => {
	const id = raw.replaceAll("-", "").toLowerCase()
	if (!/^[0-9a-f]{32}$/.test(id)) throw new Error(`${flag} must be an id (32 hex digits, dashes allowed)`)
	return id
}
const uint = (raw: string, flag: string): number => {
	if (!/^\d+$/.test(raw) || !Number.isSafeInteger(Number(raw))) throw new Error(`${flag} must be a non-negative integer`)
	return Number(raw)
}

/**
 * Arguments: [--edit <edit id>] [--space <space label>] [--partition <n>] [--offsets N|N-M] [--source edit|keyed|all]
 *            [--error <substring>]... [--created-at <unix seconds>] [--dry-run|--apply]
 * A value of `-` means the flag is not given (so one Job manifest serves every run). `--error` adds to the default
 * allowlist (`connect: Refused`); it is repeatable.
 */
export function parseArgs(argv: string[]): ReplayDeadLettersArgs {
	if (argv.some((a) => a.includes("__"))) throw new Error("an argument still holds a manifest placeholder (__X__)")
	if (argv.includes("--apply") && argv.includes("--dry-run")) throw new Error("--apply and --dry-run are exclusive")
	const out: ReplayDeadLettersArgs = {apply: argv.includes("--apply")}
	const errors = [...DEFAULT_ERRORS]
	for (let i = 0; i < argv.length; i++) {
		const flag = argv[i] as string
		if (flag === "--apply" || flag === "--dry-run") continue
		if (!VALUE_FLAGS.includes(flag)) throw new Error(`unknown argument ${flag.slice(0, 40)}`)
		const raw = argv[++i]
		if (raw === undefined || raw.startsWith("--")) throw new Error(`${flag} needs a value`)
		if (raw === "-") continue
		if (flag === "--edit") out.edit = hexId(raw, flag)
		else if (flag === "--space") out.space = hexId(raw, flag)
		else if (flag === "--partition") out.partition = uint(raw, flag)
		else if (flag === "--offsets") out.offsets = parseRange(raw, 1_000_000)
		else if (flag === "--created-at") out.createdAt = uint(raw, flag)
		else if (flag === "--error") {
			if (!raw.trim()) throw new Error("--error needs a non-empty substring")
			errors.push(raw)
		} else if (raw === "edit" || raw === "keyed") out.source = raw
		else if (raw !== "all") throw new Error("--source must be edit, keyed or all")
	}
	out.errors = [...new Set(errors)]
	if (out.createdAt !== undefined && out.edit === undefined) throw new Error("--created-at is the block time of one edit: give --edit with it")
	return out
}
