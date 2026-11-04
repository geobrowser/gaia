/**
 * geo-db read client for entity cards.
 *
 * Reaches geo-db through a small pool of long-lived `geodb-client pipe` processes. Each holds one
 * authenticated connection to the server and answers one request line at a time (`<space>\t<geoql>`)
 * with one length-prefixed frame (`+<n>\n<n bytes>` rows, or `-<n>\n<n bytes>` a refusal), so a card
 * costs a round trip on a warm connection rather than a process start and a handshake. The client is
 * a seam (`GeoDbCardClient`): the route and the fallback logic do not know how geo-db is reached.
 *
 * Failure handling is per request. A request that times out kills its worker (its connection cannot
 * be trusted with a reply still owed) and is rejected `timeout`; a worker that exits, or answers a
 * refusal, is replaced by a fresh one. A worker that dies because its connection or token did (the
 * server closes a connection idle for a minute and `pipe` never pings, so `TransportClosed`; a token
 * whose epoch or expiry passed, so `CapabilityTokenExpired`; or the process simply exiting while it
 * owed a reply) says nothing about the request, so that request is retried once on the fresh worker.
 * Any other refusal is real and rejects its request `client_failed` without a retry. Replacing dead
 * workers is rate-capped (`MAX_RETRIES_PER_WINDOW`), past which requests fail rather than retry. A
 * worker that cannot start (geo-db down) backs the whole client off for a second, so an outage answers
 * every request at once from Postgres instead of starting a process per request.
 */

import {type ChildProcess, spawn as nodeSpawn} from "node:child_process"
import {readFile, stat} from "node:fs/promises"
import type {Readable, Writable} from "node:stream"
import {type GeoCardRow, rowsFromGeoDbOutput} from "./card"
import type {GeoDbCardConfig} from "./config"

const HEX32 = /^[0-9a-f]{32}$/

/** Rows of one entity's card as geo-db answered them, in geo-db's id space. */
export type GeoDbCardClient = {
	/** `names` also asks for the display names of each row's property and target (rows then carry them). */
	readCard(args: {geoSpaceId: string; entityId: string; names?: boolean}): Promise<GeoCardRow[]>
}

export class GeoDbError extends Error {
	constructor(
		readonly reason: "timeout" | "unmapped_space" | "client_failed" | "bad_request" | "overloaded",
		message: string,
	) {
		super(message)
		this.name = "GeoDbError"
	}
}

/** The geoql document for one entity's card. Ids are 32 hex digits, so nothing needs escaping. */
export function cardDocument(entityId: string, options: {names?: boolean} = {}): string {
	const names = options.names ? ", .names = true" : ""
	return `.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .id = "${entityId}"${names} } } } }`
}

/**
 * Parse a space map file: `{"<gaia space id>": {"geo": "<geo-db space id>"}}`. Every key and `geo`
 * must be 32 hex digits (a dashed uuid is accepted and compacted); any other entry makes the whole
 * file refused, so a damaged map falls back to Postgres rather than naming a wrong Space.
 */
export function parseSpaceMap(text: string): Map<string, string> {
	const raw: unknown = JSON.parse(text)
	if (typeof raw !== "object" || raw === null || Array.isArray(raw)) throw new Error("space map is not an object")
	const map = new Map<string, string>()
	for (const [key, entry] of Object.entries(raw)) {
		const gaia = key.replaceAll("-", "").toLowerCase()
		const geo = String((entry as {geo?: unknown} | null)?.geo ?? "")
			.replaceAll("-", "")
			.toLowerCase()
		if (!HEX32.test(gaia) || !HEX32.test(geo)) throw new Error(`space map entry ${key} is not a pair of space ids`)
		map.set(gaia, geo)
	}
	return map
}

/** How often the space map file is checked for a newer version. */
const SPACE_MAP_REFRESH_MS = 10_000

/**
 * gaia space id (compact) -> geo-db space id, from the file `GEODB_SPACE_MAP_PATH` names. The file is
 * re-checked (one `stat`) at most every `refreshMs` and re-read when its mtime or size changed, because
 * the geodb-feeder adds a Space to it whenever gaia founds one. A failed reload keeps the map already
 * held, so a briefly unreadable file never blinds a running pod; only a pod that has never read a good
 * map answers `client_failed`.
 */
export function createSpaceMap(
	path: string,
	options: {refreshMs?: number; now?: () => number} = {},
): (gaiaSpaceId: string) => Promise<string | null> {
	const refreshMs = options.refreshMs ?? SPACE_MAP_REFRESH_MS
	const now = options.now ?? Date.now
	let held: {map: Map<string, string>; stamp: string} | null = null
	let checkedAt = Number.NEGATIVE_INFINITY
	let inflight: Promise<void> | null = null

	const refresh = async () => {
		const info = await stat(path)
		const stamp = `${info.mtimeMs}:${info.size}`
		if (held?.stamp === stamp) return
		held = {map: parseSpaceMap(await readFile(path, "utf8")), stamp}
	}

	return async (gaiaSpaceId) => {
		if (!path) return null
		if (now() - checkedAt >= refreshMs) {
			checkedAt = now()
			inflight ??= refresh()
				.catch((error) => {
					if (held === null) {
						checkedAt = Number.NEGATIVE_INFINITY // retry on the next request
						throw error
					}
				})
				.finally(() => {
					inflight = null
				})
		}
		try {
			if (inflight) await inflight
		} catch (error) {
			throw new GeoDbError("client_failed", `space map unreadable: ${String(error)}`)
		}
		return held?.map.get(gaiaSpaceId) ?? null
	}
}

/** The part of a child process the pool uses; tests substitute their own. */
export type PipeChild = {
	stdin: Writable
	stdout: Readable
	stderr: Readable | null
	kill(signal?: NodeJS.Signals): boolean
	on(event: "exit" | "error", listener: (...args: unknown[]) => void): unknown
}

export type SpawnPipe = (config: GeoDbCardConfig) => PipeChild

const defaultSpawn: SpawnPipe = (config) =>
	nodeSpawn(config.clientBin, ["--key", config.keyPath, "--binding", config.bindingPath, "pipe"], {
		stdio: ["pipe", "pipe", "pipe"],
	}) as ChildProcess as unknown as PipeChild

/** How long the client refuses work after a worker failed to start. */
const START_BACKOFF_MS = 1000
const STDERR_TAIL = 300
/** Retries on a fresh worker allowed per window; past it a dead worker's request just fails. */
const MAX_RETRIES_PER_WINDOW = 8
const RETRY_WINDOW_MS = 10_000
/** A refusal naming a dead connection or token, not a verdict on the request. */
const DEAD_SESSION = /\b(TransportClosed|TransportFailed|CapabilityTokenExpired)\b/
const FRAME = /^([+-])(\d{1,9})$/

type Request = {
	line: string
	entityId: string
	names: boolean
	resolve: (rows: GeoCardRow[]) => void
	reject: (error: GeoDbError) => void
	retried: boolean
	timer: ReturnType<typeof setTimeout>
	slot: Slot | null
}

type Slot = {
	child: PipeChild | null
	ready: boolean
	buffer: Buffer
	request: Request | null
	stderr: string
}

export type PipeGeoDbClient = GeoDbCardClient & {
	/** Stop every worker. */
	close(): void
}

export function createPipeGeoDbClient(
	config: GeoDbCardConfig,
	options: {spawn?: SpawnPipe; now?: () => number} = {},
): PipeGeoDbClient {
	const spawn = options.spawn ?? defaultSpawn
	const now = options.now ?? Date.now
	const slots: Slot[] = Array.from({length: Math.max(1, config.poolSize)}, () => ({
		child: null,
		ready: false,
		buffer: Buffer.alloc(0),
		request: null,
		stderr: "",
	}))
	const queue: Request[] = []
	const maxQueue = slots.length * 64
	let backoffUntil = 0
	let retryTimes: number[] = []

	/** Whether another retry fits the rate cap; counts it when it does. */
	function retryBudget(): boolean {
		const t = now()
		retryTimes = retryTimes.filter((at) => t - at < RETRY_WINDOW_MS)
		if (retryTimes.length >= MAX_RETRIES_PER_WINDOW) return false
		retryTimes.push(t)
		return true
	}

	/**
	 * Drop the slot's worker, rejecting the request it owed a reply, or with `retry` (the worker's
	 * connection or token died, not the request) putting it back at the head of the queue for a fresh
	 * worker, once.
	 */
	function reset(slot: Slot, failure: GeoDbError | null, retry = false) {
		const child = slot.child
		const request = slot.request
		const neverReady = !slot.ready
		slot.child = null
		slot.ready = false
		slot.buffer = Buffer.alloc(0)
		slot.request = null
		if (child) {
			child.kill("SIGKILL")
		}
		if (neverReady && child) backoffUntil = now() + START_BACKOFF_MS
		if (request && failure) {
			if (retry && !neverReady && !request.retried && retryBudget()) {
				request.retried = true
				request.slot = null
				queue.unshift(request)
			} else settle(request, failure)
		}
		pump()
	}

	function settle(request: Request, outcome: GeoDbError | GeoCardRow[]) {
		clearTimeout(request.timer)
		request.slot = null
		if (outcome instanceof GeoDbError) request.reject(outcome)
		else request.resolve(outcome)
	}

	function start(slot: Slot) {
		let child: PipeChild
		try {
			child = spawn(config)
		} catch (error) {
			backoffUntil = now() + START_BACKOFF_MS
			const request = slot.request
			slot.request = null
			if (request) settle(request, new GeoDbError("client_failed", `cannot start geodb-client: ${String(error)}`))
			return
		}
		slot.child = child
		slot.ready = false
		slot.buffer = Buffer.alloc(0)
		slot.stderr = ""
		const owns = () => slot.child === child
		child.stdin.on("error", () => {})
		child.stderr?.on("data", (chunk: Buffer) => {
			slot.stderr = (slot.stderr + chunk.toString("utf8")).slice(-STDERR_TAIL)
		})
		child.stdout.on("data", (chunk: Buffer) => {
			if (owns()) onData(slot, chunk)
		})
		const gone = (what: string) => {
			if (!owns()) return
			const detail = slot.stderr.trim() || what
			// Exiting with a reply owed, on a worker that had started, is a dead connection.
			reset(slot, new GeoDbError("client_failed", `geodb-client ${what}: ${detail}`.slice(0, 400)), true)
		}
		child.on("exit", (code, signal) => gone(`exited (${String(signal ?? code)})`))
		child.on("error", (error) => gone(`failed: ${String(error)}`))
	}

	function onData(slot: Slot, chunk: Buffer) {
		slot.buffer = slot.buffer.length === 0 ? chunk : Buffer.concat([slot.buffer, chunk])
		if (!slot.ready) {
			const end = slot.buffer.indexOf(10)
			if (end < 0) return
			const first = slot.buffer.subarray(0, end).toString("utf8")
			if (first !== "ready") {
				const rest = slot.buffer.toString("utf8").slice(0, 300)
				reset(slot, new GeoDbError("client_failed", `geodb-client did not start: ${rest}`))
				return
			}
			slot.ready = true
			slot.buffer = slot.buffer.subarray(end + 1)
			if (slot.request) slot.child?.stdin.write(slot.request.line)
		}
		// Frames: `+<n>\n<n bytes>` or `-<n>\n<n bytes>`, one per request sent.
		for (;;) {
			const end = slot.buffer.indexOf(10)
			if (end < 0) return
			const header = FRAME.exec(slot.buffer.subarray(0, end).toString("utf8"))
			const request = slot.request
			if (!header || !request) {
				reset(slot, new GeoDbError("client_failed", "geodb-client broke its framing"))
				return
			}
			const size = Number(header[2])
			if (slot.buffer.length < end + 1 + size) return
			const payload = slot.buffer.subarray(end + 1, end + 1 + size).toString("utf8")
			slot.buffer = slot.buffer.subarray(end + 1 + size)
			if (header[1] === "-") {
				// A refusal ends the worker's session; the next request starts a fresh one. Only a refusal
				// that names a dead connection or token is retried; the rest are the request's own.
				reset(slot, new GeoDbError("client_failed", payload.slice(0, 300)), DEAD_SESSION.test(payload))
				return
			}
			slot.request = null
			try {
				settle(request, rowsFromGeoDbOutput(payload, request.entityId, {names: request.names}))
			} catch (error) {
				settle(request, new GeoDbError("client_failed", String(error)))
			}
			pump()
		}
	}

	/** Hand queued requests to idle slots. */
	function pump() {
		while (queue.length > 0) {
			const slot = slots.find((s) => s.request === null)
			if (!slot) return
			const request = queue.shift() as Request
			assign(slot, request)
		}
	}

	function assign(slot: Slot, request: Request) {
		slot.request = request
		request.slot = slot
		if (!slot.child) start(slot)
		else if (slot.ready) slot.child.stdin.write(request.line)
	}

	function onTimeout(request: Request) {
		const queued = queue.indexOf(request)
		if (queued >= 0) queue.splice(queued, 1)
		const slot = request.slot
		const failure = new GeoDbError("timeout", `geo-db read exceeded ${config.timeoutMs} ms`)
		if (slot && slot.request === request) reset(slot, failure)
		else settle(request, failure)
	}

	return {
		readCard({geoSpaceId, entityId, names = false}) {
			if (!HEX32.test(geoSpaceId) || !HEX32.test(entityId)) {
				return Promise.reject(new GeoDbError("bad_request", "ids must be 32 hex digits"))
			}
			if (now() < backoffUntil) {
				return Promise.reject(new GeoDbError("client_failed", "geodb-client is not starting; backing off"))
			}
			return new Promise((resolve, reject) => {
				const request: Request = {
					line: `${geoSpaceId}\t${cardDocument(entityId, {names})}\n`,
					entityId,
					names,
					resolve,
					reject,
					retried: false,
					timer: setTimeout(() => onTimeout(request), config.timeoutMs),
					slot: null,
				}
				const idle = slots.find((s) => s.request === null)
				if (idle) assign(idle, request)
				else if (queue.length >= maxQueue) {
					clearTimeout(request.timer)
					reject(new GeoDbError("overloaded", `${queue.length} geo-db reads already waiting`))
				} else queue.push(request)
			})
		},
		close() {
			for (const slot of slots) reset(slot, new GeoDbError("client_failed", "client closed"))
			for (const request of queue.splice(0)) settle(request, new GeoDbError("client_failed", "client closed"))
		},
	}
}
