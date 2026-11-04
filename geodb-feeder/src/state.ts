/**
 * Durable feeder state on the geo-db volume, so a crash or a pod move never loses what geo-db already took:
 *
 * - `offsets.json`: per partition, the last fully applied offset and the progress inside the message being
 *   applied (`intentsDone`). It is the source of truth for resuming; the Kafka group commit is only for lag.
 * - `known-entities.log`, `known-statements.log`: append-only journals of what geo-db holds. They change only
 *   when a whole message has applied, so replaying a half-applied message translates to the same intents and
 *   resumes at `intentsDone`.
 * - `spaces.json`: gaia Space -> geo-db Space (`geo` is null while a founding is in flight).
 */
import {appendFileSync, closeSync, existsSync, fsyncSync, mkdirSync, openSync, readdirSync, readFileSync, readSync, renameSync, writeSync} from "node:fs"
import type {TranslateContext} from "./translate.js"

export type SpaceRecord = {geo: string | null; nonce: string; keyed: boolean}
export type Inflight = {offset: number; intentsDone: number}
type OffsetsFile = {done: Record<string, number>; inflight: Record<string, Inflight>}

/**
 * Call `each` for every non-empty line of `path`, reading 1 MiB at a time. The journals reach hundreds of
 * MB after a backfill; reading one as a single string and splitting it would hold the text twice over.
 */
export function eachLine(path: string, each: (line: string) => void): void {
	if (!existsSync(path)) return
	const fd = openSync(path, "r")
	try {
		const buf = Buffer.alloc(1 << 20)
		let rest = ""
		for (;;) {
			const n = readSync(fd, buf, 0, buf.length, null)
			if (n === 0) break
			const text = rest + buf.toString("utf8", 0, n)
			const parts = text.split("\n")
			rest = parts.pop() ?? ""
			for (const l of parts) if (l) each(l)
		}
		if (rest) each(rest)
	} finally {
		closeSync(fd)
	}
}

function loadSet(path: string): Set<string> {
	const set = new Set<string>()
	eachLine(path, (l) => set.add(l))
	return set
}

function writeFileDurable(path: string, text: string): void {
	const tmp = `${path}.tmp`
	const fd = openSync(tmp, "w")
	try {
		writeSync(fd, text)
		fsyncSync(fd)
	} finally {
		closeSync(fd)
	}
	renameSync(tmp, path)
}

function appendDurable(path: string, text: string): void {
	if (!text) return
	const fd = openSync(path, "a")
	try {
		writeSync(fd, text)
		fsyncSync(fd)
	} finally {
		closeSync(fd)
	}
}

export class Store {
	readonly dir: string
	readonly hadOffsets: boolean
	spaces: Record<string, SpaceRecord>
	private offsets: OffsetsFile
	private readonly entities: Set<string>
	private readonly statements: Set<string>

	constructor(dir: string) {
		this.dir = dir
		mkdirSync(dir, {recursive: true})
		const spacesPath = `${dir}/spaces.json`
		this.spaces = existsSync(spacesPath) ? JSON.parse(readFileSync(spacesPath, "utf8")) : {}
		const offsetsPath = `${dir}/offsets.json`
		this.hadOffsets = existsSync(offsetsPath)
		this.offsets = this.hadOffsets
			? {done: {}, inflight: {}, ...JSON.parse(readFileSync(offsetsPath, "utf8"))}
			: {done: {}, inflight: {}}
		this.entities = loadSet(`${dir}/known-entities.log`)
		this.statements = loadSet(`${dir}/known-statements.log`)
	}

	saveSpaces(): void {
		writeFileDurable(`${this.dir}/spaces.json`, JSON.stringify(this.spaces))
	}

	hasEntity = (space: string, entity: string) => this.entities.has(`${space}|${entity}`)
	hasStatement = (statement: string) => this.statements.has(statement)
	get entityCount(): number {
		return this.entities.size
	}
	get statementCount(): number {
		return this.statements.size
	}

	/** Journal what a fully applied message added. */
	commitMarks(entities: string[], statements: string[]): void {
		appendDurable(`${this.dir}/known-entities.log`, entities.map((e) => `${e}\n`).join(""))
		appendDurable(`${this.dir}/known-statements.log`, statements.map((s) => `${s}\n`).join(""))
		for (const e of entities) this.entities.add(e)
		for (const s of statements) this.statements.add(s)
	}

	/** Every partition's last fully applied offset. */
	get done(): Record<string, number> {
		return {...this.offsets.done}
	}

	/** Partitions with a message part-way applied (the feeder stopped inside it). */
	get inflight(): Record<string, Inflight> {
		return {...this.offsets.inflight}
	}

	lastDone(partition: number): number | null {
		return this.offsets.done[String(partition)] ?? null
	}

	/** Progress inside `offset`, or 0 if that message has not started. */
	progress(partition: number, offset: number): number {
		const p = this.offsets.inflight[String(partition)]
		return p && p.offset === offset ? p.intentsDone : 0
	}

	saveProgress(partition: number, offset: number, intentsDone: number): void {
		this.offsets.inflight[String(partition)] = {offset, intentsDone}
		this.flushOffsets()
	}

	markDone(partition: number, offset: number): void {
		this.offsets.done[String(partition)] = offset
		delete this.offsets.inflight[String(partition)]
		this.flushOffsets()
	}

	private flushOffsets(): void {
		writeFileDurable(`${this.dir}/offsets.json`, JSON.stringify(this.offsets))
	}

	deadLetter(record: Record<string, unknown>): void {
		appendFileSync(`${this.dir}/errors.jsonl`, `${JSON.stringify({t: Date.now(), ...record})}\n`)
	}
}

/**
 * The translator's view of what geo-db holds: the journals plus what this message has added so far.
 * Nothing reaches the journals until `commit()`, after every chunk of the message has applied.
 */
export class Overlay implements TranslateContext {
	private readonly entities = new Set<string>()
	private readonly statements = new Set<string>()

	constructor(
		private readonly store: Store,
		private readonly space: string,
	) {}

	knownEntity = (e: string) => this.entities.has(e) || this.store.hasEntity(this.space, e)
	markEntity = (e: string) => void this.entities.add(e)
	knownStmt = (s: string) => this.statements.has(s) || this.store.hasStatement(s)
	markStmt = (s: string) => void this.statements.add(s)

	commit(): void {
		this.store.commitMarks([...this.entities].map((e) => `${this.space}|${e}`), [...this.statements])
	}
}

/**
 * The Spaces whose history the backfill has loaded in full: each has its entity list on the backfill volume and
 * a cursor at (or past) its end. Null when the backfill has never run here, which means "no Space is known to be
 * unloaded" (a feeder that only follows live edits has nothing to say). A Space the backfill never planned, such
 * as one the membership backfill founded empty, is not in the set until a backfill covers it.
 */
export function backfilledSpaces(dir: string): Set<string> | null {
	const cursorPath = `${dir}/backfill/cursor.json`
	if (!existsSync(cursorPath)) return null
	const cursor = JSON.parse(readFileSync(cursorPath, "utf8")) as {done?: Record<string, number>}
	const done = cursor.done ?? {}
	const loaded = new Set<string>()
	for (const name of readdirSync(`${dir}/backfill`)) {
		const match = /^entities-(.+)\.txt$/.exec(name)
		if (!match?.[1]) continue
		let count = 0
		eachLine(`${dir}/backfill/${name}`, () => void count++)
		if ((done[match[1]] ?? 0) >= count) loaded.add(match[1])
	}
	return loaded
}

/**
 * The file `GEODB_SPACE_MAP_PATH` names in the api: `{"<gaia space>": {"geo": "<geo-db space>"}}`. With `loaded`
 * (see `backfilledSpaces`) a founded Space outside it is written `"loaded": false`, which the api reads as "geo-db
 * has nothing to compare there" rather than as a gap; without it every Space is loaded, as before.
 */
export function renderSpaceMap(spaces: Record<string, SpaceRecord>, loaded: ReadonlySet<string> | null = null): string {
	const out: Record<string, {geo: string; loaded?: false}> = {}
	for (const [gaia, rec] of Object.entries(spaces).sort(([a], [b]) => a.localeCompare(b))) {
		if (!rec.geo) continue
		out[gaia] = loaded && !loaded.has(gaia) ? {geo: rec.geo, loaded: false} : {geo: rec.geo}
	}
	return `${JSON.stringify(out, null, 1)}\n`
}

export {writeFileDurable}
