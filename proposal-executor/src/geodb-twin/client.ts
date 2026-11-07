/**
 * A minimal geo-db read client for the twin: one `geodb-client pipe` worker, one request at a time, a timeout that
 * kills the worker, and a cool-down after any failure.
 *
 * It follows the api's client (api/src/geodb/client.ts) on the wire: a request line is `<space>\t<geoql document>`,
 * the answer one frame, `+<n>\n<n bytes>` for rows or `-<n>\n<n bytes>` for a refusal. It is smaller on purpose.
 * The executor is a one-shot CronJob, so there is no pool and no keep-warm: the worker starts on the first read and
 * `close()` ends it. A failure (timeout, refusal, worker exit) kills the worker and opens a cool-down; reads inside it
 * are refused at once, so a geo-db outage costs the twin one timeout and not one a read. A process lives one tick, so
 * the cool-down only ever spans the rest of that tick; the twin also stops at the first failure.
 */
import {spawn} from "node:child_process"
import type {Readable, Writable} from "node:stream"

import type {CardRow} from "./records.js"

export class GeoDbTwinError extends Error {
	constructor(
		readonly reason: "timeout" | "client_failed" | "cooling_down" | "bad_request" | "closed",
		message: string,
	) {
		super(message)
		this.name = "GeoDbTwinError"
	}
}

/** Most entities one batch card read names (geo-db's bound). */
export const MAX_CARD_IDS = 32
/** Most backlinks one read asks for: geo-db's `MAX_TOPK_LIMIT`. */
export const MAX_BACKLINKS = 4096

const HEX32 = /^[0-9a-f]{32}$/
const FRAME = /^([+-])(\d{1,9})$/
const STDERR_TAIL = 300

/** The part of a child process the client uses; tests substitute their own. */
export type PipeChild = {
	stdin: Writable
	stdout: Readable
	stderr: Readable | null
	kill(signal?: NodeJS.Signals): boolean
	on(event: "exit" | "error", listener: (...args: unknown[]) => void): unknown
}

export type GeoDbTwinClientConfig = {
	clientBin: string
	keyPath: string
	bindingPath: string
	/** Per read, ms. */
	timeoutMs: number
	/** After a failure, how long reads are refused, ms. */
	cooldownMs: number
}

export type SpawnPipe = (config: GeoDbTwinClientConfig) => PipeChild

const defaultSpawn: SpawnPipe = (config) =>
	spawn(config.clientBin, ["--key", config.keyPath, "--binding", config.bindingPath, "pipe"], {
		stdio: ["pipe", "pipe", "pipe"],
	}) as unknown as PipeChild

/** The geoql document for one entity's card (with backlinks when asked). Ids are 32 hex digits. */
export function cardDocument(entityId: string, options: {backlinks?: number} = {}): string {
	const backlinks = options.backlinks ? `, .backlinks = ${options.backlinks}` : ""
	return `.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .id = "${entityId}"${backlinks} } } } }`
}

/** The geoql document for the cards of 1..32 entities. Ids are 32 hex digits. */
export function cardsDocument(entityIds: string[]): string {
	const ids = entityIds.map((id) => `"${id}"`).join(", ")
	return `.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .ids = .{ ${ids} } } } } }`
}

const unescapeText = (s: string): string =>
	s.replace(/\\(t|n|\\)/g, (_, c: string) => (c === "t" ? "\t" : c === "n" ? "\n" : "\\"))

const cellsOf = (output: string): string[][] =>
	output
		.split("\n")
		.filter((line) => line !== "")
		.map((line) => line.split("\t"))

/** Card rows of one entity from the answer (a card read also returns the entities its backlinks name). */
export function cardRowsFromOutput(output: string, entityId: string): CardRow[] {
	return cellsOf(output)
		.filter((cells) => cells[3] === entityId)
		.map((cells) => ({prop: cells[0] ?? "", raw: unescapeText(cells[1] ?? "")}))
}

/** Card rows of several entities from a batch answer, grouped by owning entity. */
export function batchRowsFromOutput(output: string): Map<string, CardRow[]> {
	const by = new Map<string, CardRow[]>()
	for (const cells of cellsOf(output)) {
		const entity = cells[3] ?? ""
		if (!HEX32.test(entity)) continue
		const rows = by.get(entity) ?? []
		rows.push({prop: cells[0] ?? "", raw: unescapeText(cells[1] ?? "")})
		by.set(entity, rows)
	}
	return by
}

/** The rows whose value references `entityId`, each with the referring entity. */
export function backlinkRowsFromOutput(output: string, entityId: string): {prop: string; from: string}[] {
	return cellsOf(output)
		.filter((cells) => cells[1] === entityId && HEX32.test(cells[3] ?? ""))
		.map((cells) => ({prop: cells[0] ?? "", from: cells[3] as string}))
}

/** The reads the evaluator needs. A test fixture implements this directly. */
export type GeoDbTwinReader = {
	readCard(geoSpaceId: string, entityId: string): Promise<CardRow[]>
	/** Cards of any number of entities (batched by the implementation), keyed by entity; absent entities have no key. */
	readCards(geoSpaceId: string, entityIds: string[]): Promise<Map<string, CardRow[]>>
	/** Referring statements to `entityId`; `limit` rows back means there may be more. */
	readBacklinks(geoSpaceId: string, entityId: string, limit: number): Promise<{prop: string; from: string}[]>
}

export type GeoDbTwinClient = GeoDbTwinReader & {
	close(): void
	/** True from a failure until `cooldownMs` has passed. */
	coolingDown(): boolean
}

export function createGeoDbTwinClient(
	config: GeoDbTwinClientConfig,
	deps: {spawnPipe?: SpawnPipe; now?: () => number} = {},
): GeoDbTwinClient {
	const spawnPipe = deps.spawnPipe ?? defaultSpawn
	const now = deps.now ?? Date.now
	let coolUntil = Number.NEGATIVE_INFINITY
	let closed = false
	let worker: {child: PipeChild; stderrTail: string; buf: Buffer; waiter: Waiter | null; dead: boolean} | null = null
	let chain: Promise<unknown> = Promise.resolve()

	type Waiter = {resolve: (text: string) => void; reject: (e: GeoDbTwinError) => void}

	const fail = (reason: GeoDbTwinError["reason"], message: string): GeoDbTwinError => {
		coolUntil = now() + config.cooldownMs
		return new GeoDbTwinError(reason, message)
	}

	const killWorker = () => {
		const w = worker
		worker = null
		if (!w) return
		w.dead = true
		try {
			w.child.kill("SIGKILL")
		} catch {
			// already gone
		}
	}

	/** Take whole frames off the worker's buffer and hand the first one to whoever waits. */
	const drain = (w: NonNullable<typeof worker>) => {
		const waiter = w.waiter
		if (!waiter) return
		const nl = w.buf.indexOf(0x0a)
		if (nl < 0) return
		const header = w.buf.subarray(0, nl).toString("utf8")
		const m = FRAME.exec(header)
		if (!m) {
			w.waiter = null
			killWorker()
			waiter.reject(fail("client_failed", `unreadable frame header: ${header.slice(0, 40)}`))
			return
		}
		const n = Number(m[2])
		if (w.buf.length < nl + 1 + n) return
		const body = w.buf.subarray(nl + 1, nl + 1 + n).toString("utf8")
		w.buf = w.buf.subarray(nl + 1 + n)
		w.waiter = null
		if (m[1] === "+") return waiter.resolve(body)
		killWorker() // a refusal ends the session
		waiter.reject(fail("client_failed", `refused: ${body.slice(0, STDERR_TAIL)}`))
	}

	const start = (): NonNullable<typeof worker> => {
		const child = spawnPipe(config)
		const w = {child, stderrTail: "", buf: Buffer.alloc(0), waiter: null as Waiter | null, dead: false}
		child.stdout.on("data", (chunk: Buffer) => {
			w.buf = Buffer.concat([w.buf, chunk])
			drain(w)
		})
		child.stderr?.on("data", (chunk: Buffer) => {
			w.stderrTail = (w.stderrTail + chunk.toString("utf8")).slice(-STDERR_TAIL)
		})
		const died = (what: string) => {
			const owed = w.waiter
			w.waiter = null
			if (worker === w) worker = null
			if (!w.dead) w.dead = true
			if (owed) owed.reject(fail("client_failed", `worker ${what}: ${w.stderrTail.trim()}`))
		}
		child.stdin.on("error", () => {})
		child.on("exit", () => died("exited"))
		child.on("error", () => died("failed to start"))
		worker = w
		return w
	}

	const roundTrip = (space: string, doc: string): Promise<string> =>
		new Promise((resolve, reject) => {
			if (closed) return reject(new GeoDbTwinError("closed", "client closed"))
			if (now() < coolUntil) return reject(new GeoDbTwinError("cooling_down", "geo-db twin cooling down"))
			if (!HEX32.test(space)) return reject(new GeoDbTwinError("bad_request", "space is not 32 hex digits"))
			const w = worker ?? start()
			const timer = setTimeout(() => {
				w.waiter = null
				killWorker()
				reject(fail("timeout", `no answer in ${config.timeoutMs} ms`))
			}, config.timeoutMs)
			w.waiter = {
				resolve: (text) => {
					clearTimeout(timer)
					resolve(text)
				},
				reject: (e) => {
					clearTimeout(timer)
					reject(e)
				},
			}
			w.child.stdin.write(`${space}\t${doc}\n`)
		})

	/** Reads go one at a time: the worker answers one line, then the next. */
	const send = (space: string, doc: string): Promise<string> => {
		const next = chain.then(() => roundTrip(space, doc))
		chain = next.catch(() => {})
		return next
	}

	return {
		coolingDown: () => now() < coolUntil,
		close: () => {
			closed = true
			killWorker()
		},
		async readCard(space, entityId) {
			return cardRowsFromOutput(await send(space, cardDocument(entityId)), entityId)
		},
		async readCards(space, entityIds) {
			const out = new Map<string, CardRow[]>()
			for (let i = 0; i < entityIds.length; i += MAX_CARD_IDS) {
				const batch = entityIds.slice(i, i + MAX_CARD_IDS)
				const wanted = new Set(batch)
				for (const [entity, rows] of batchRowsFromOutput(await send(space, cardsDocument(batch)))) {
					if (wanted.has(entity)) out.set(entity, rows)
				}
			}
			return out
		},
		async readBacklinks(space, entityId, limit) {
			const capped = Math.min(limit, MAX_BACKLINKS)
			return backlinkRowsFromOutput(await send(space, cardDocument(entityId, {backlinks: capped})), entityId)
		},
	}
}
