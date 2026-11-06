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
 *
 * A read that lands on a dead worker pays for a fresh process and handshake (25 to 100 ms against 2 ms
 * warm), and a worker dies three ways while idle: the server closes a connection idle for a minute, the
 * token its handshake got expires (`grantTtlMs`, geo-db's `grant_ttl_s`), and any refusal ends it. So once
 * the pool has served a read, a background pass keeps every worker ready ahead of need:
 * - an idle worker is sent one read of a card no entity has (an empty answer) once it has been idle
 *   `keepWarmMs`, so its connection never idles out;
 * - an idle worker is replaced before its token expires, so no read finds the token dead;
 * - a slot whose worker died is given a fresh one, so the next burst finds every slot ready.
 * Requests go to a ready idle worker first, then one still starting, then an empty slot.
 */

import {type ChildProcess, spawn as nodeSpawn} from "node:child_process"
import {readFile, stat} from "node:fs/promises"
import type {Readable, Writable} from "node:stream"
import {
	backlinkRowsFromGeoDbOutput,
	type GeoBacklinkRow,
	type GeoCardRow,
	type GeoPageRow,
	pageRowsFromGeoDbOutput,
	rowsFromGeoDbOutput,
} from "./card"
import type {GeoDbCardConfig} from "./config"
import {observeReadStage, type ReadShape, recordWorkerEnd, recordWorkerStart, type WorkerEndReason} from "./metrics"

const HEX32 = /^[0-9a-f]{32}$/

/** Most entities one `.args.ids` card read names (geo-db's bound). */
export const MAX_CARD_IDS = 32

/** Rows of one entity's card as geo-db answered them, in geo-db's id space. */
export type GeoDbCardClient = {
	/** `names` also asks for the display names of each row's property and target (rows then carry them). */
	readCard(args: {geoSpaceId: string; entityId: string; names?: boolean}): Promise<GeoCardRow[]>
	/**
	 * The card across several Spaces in one read (`pipe` line `<space>,<space>\t<geoql>`): the rows of every
	 * Space, each carrying its `space`. A Space the node does not hold refuses the whole read, so the caller
	 * names only Spaces it knows geo-db holds. Absent on a client that cannot do it.
	 */
	readCardSpaces?(args: {
		geoSpaceIds: string[]
		entityId: string
		names?: boolean
		/** This read's timeout in ms, in place of the pool's: a shadow read wider than the serving path's budget. */
		timeoutMs?: number
	}): Promise<GeoCardRow[]>
	/**
	 * The cards of several entities in the Spaces named (`.args.ids`, geo-db ISS-625-317): the rows of every card, each
	 * carrying its `entity`, and its `space` on a multi-Space read. At most `MAX_CARD_IDS` entities go in one read;
	 * more are read as several, in parallel. Needs a store from geo-db 8df213cbac or later, which an older one refuses.
	 * Absent on a client that cannot do it.
	 */
	readCards?(args: {geoSpaceIds: string[]; entityIds: string[]; names?: boolean}): Promise<GeoCardRow[]>
	/**
	 * The entities referring to `entityId` (its card's backlinks, `.args.backlinks = limit`) in one or several
	 * Spaces, one row a referring statement. At most `limit` rows come back, so `limit` rows means there may be
	 * more. Absent on a client that cannot do it.
	 */
	readBacklinks?(args: {geoSpaceIds: string[]; entityId: string; limit: number}): Promise<GeoBacklinkRow[]>
	/**
	 * One page of the entities of a type ordered by a property (`entities` root, `.order` with `.pick` and `.ties = .entity`),
	 * across the Spaces named: the rows of the order's property, the page's entities in order, `first` of them at most.
	 * Needs a store that reads the order's `.pick`/`.ties` (geo-db d3967dffbf), which an older one refuses. Absent on a
	 * client that cannot do it.
	 */
	readEntitiesPage?(args: EntitiesPageRequest): Promise<GeoPageRow[]>
}

export type EntitiesPageRequest = {
	geoSpaceIds: string[]
	typeId: string
	/** The property the page is ordered by (32 hex digits). */
	field: string
	direction: "asc" | "desc"
	/** The value an entity is placed by across Spaces: `least` (earliest) or `greatest` (latest). */
	pick: "least" | "greatest"
	/** Objects on the page, 1 to `MAX_PAGE`. */
	first: number
}

/** Most objects one page read asks for. */
export const MAX_PAGE = 100

/** Most backlinks one read asks for: geo-db's `MAX_TOPK_LIMIT`. */
export const MAX_BACKLINKS = 4096

/** Most Spaces one multi-Space line may name (geo-db's vector bound). */
export const MAX_LINE_SPACES = 1024

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
export function cardDocument(entityId: string, options: {names?: boolean; backlinks?: number} = {}): string {
	const names = options.names ? ", .names = true" : ""
	const backlinks = options.backlinks ? `, .backlinks = ${options.backlinks}` : ""
	return `.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .id = "${entityId}"${names}${backlinks} } } } }`
}

/** The geoql document for the cards of 1..32 entities (`.args.ids`). Ids are 32 hex digits, so nothing needs escaping. */
export function cardsDocument(entityIds: string[], options: {names?: boolean} = {}): string {
	const names = options.names ? ", .names = true" : ""
	const ids = entityIds.map((id) => `"${id}"`).join(", ")
	return `.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .ids = .{ ${ids} }${names} } } } }`
}

/**
 * The geoql document for a page of a type's entities ordered by `field`: the order's `.pick` places each entity by its
 * least or greatest value across the Spaces, equal keys follow by entity identity (`.ties = .entity`), and the rows the
 * page selects are the order property's own, so the caller reads each object's key. Ids are 32 hex digits.
 */
export function entitiesPageDocument(request: Omit<EntitiesPageRequest, "geoSpaceIds">): string {
	const order = `.{ .field = "${request.field}", .direction = .${request.direction}, .pick = .${request.pick}, .ties = .entity }`
	return `.{ .version = 1, .select = .{ .p = .{ .field = "entities", .args = .{ .type = "${request.typeId}", .first = ${request.first}, .order = ${order} }, .select = .{ .k = .{ .field = "${request.field}" } } } } }`
}

/**
 * Parse a space map file: `{"<gaia space id>": {"geo": "<geo-db space id>", "loaded"?: false}}`. Every key
 * and `geo` must be 32 hex digits (a dashed uuid is accepted and compacted); any other entry makes the
 * whole file refused, so a damaged map falls back to Postgres rather than naming a wrong Space.
 *
 * `loaded: false` marks a Space geo-db has founded but whose history has not been backfilled: it holds
 * nothing (or only live edits), so reading it would only report a gap. Such an entry is left out of the
 * map, which makes the Space "not loaded" everywhere (`geoSpaceOf` answers null). An absent field means
 * loaded, so a map from an older feeder behaves as before.
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
		if ((entry as {loaded?: unknown} | null)?.loaded === false) continue
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
export type SpaceMap = ((gaiaSpaceId: string) => Promise<string | null>) & {
	/** Every geo-db space id (32 hex digits) in the map, distinct; empty without a path. */
	all(): Promise<string[]>
}

export function createSpaceMap(path: string, options: {refreshMs?: number; now?: () => number} = {}): SpaceMap {
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

	const current = async () => {
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
		return held?.map ?? null
	}

	const lookup = async (gaiaSpaceId: string) => (await current())?.get(gaiaSpaceId) ?? null
	return Object.assign(lookup, {all: async () => [...new Set((await current())?.values() ?? [])]})
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
/** Idle time after which a worker is sent a keep-warm read (the server's idle bound is a minute). */
const KEEP_WARM_MS = 25_000
/** geo-db's default `grant_ttl_s`, in ms: a worker's token expires this long after its handshake. */
const GRANT_TTL_MS = 300_000
/** How long before its token expires an idle worker is replaced. */
const RECYCLE_MARGIN_MS = 60_000
/**
 * An entity id nobody has, so the keep-warm read's card is empty. Not the all-zero id, which geo-db treats as
 * an unset field and may refuse, and a refusal ends the worker's session. A conforming id (version 4, variant 8).
 */
const NO_ENTITY = "0123456789ab4def8123456789abcdef"
/** A refusal naming a dead connection or token, not a verdict on the request. */
const DEAD_SESSION = /\b(TransportClosed|TransportFailed|CapabilityTokenExpired)\b/
const FRAME = /^([+-])(\d{1,9})$/

const shapeOf = (request: {spaces: boolean; names: boolean; backlinks?: boolean}): ReadShape =>
	request.backlinks
		? "backlinks"
		: request.spaces
			? request.names
				? "several_names"
				: "several"
			: request.names
				? "one_names"
				: "one"

type Request = {
	line: string
	entityId: string
	names: boolean
	/** The line named several Spaces, so each row's statement cell carries its Space. */
	spaces: boolean
	/** A backlinks read: the answer is the rows referring to `entityId`, not its own. */
	backlinks?: boolean
	/** An ordered `entities` page: the answer is the page's rows, whatever entity they name. */
	page?: boolean
	/** A read of several entities' cards: the answer is every row, each keeping its entity. */
	cards?: boolean
	resolve: (rows: GeoCardRow[]) => void
	reject: (error: GeoDbError) => void
	retried: boolean
	/** A keep-warm read: no caller waits on it, and it does not name the slot's Space. */
	keepWarm?: boolean
	timer: ReturnType<typeof setTimeout>
	slot: Slot | null
	/** When it was submitted, and when its line was written to a worker (null until then). */
	submittedAt: number
	sentAt: number | null
	/** This request's own timeout in ms, when it is not the pool's. */
	timeoutMs?: number
}

type Slot = {
	child: PipeChild | null
	ready: boolean
	buffer: Buffer
	request: Request | null
	stderr: string
	/** When the slot last finished a request (or became ready). */
	activeAt: number
	/** When the slot's worker became ready: its token expires `grantTtlMs` later. */
	readyAt: number
	/** The Space of the slot's last read, which the keep-warm read names. */
	lastSpace: string | null
}

export type PipeGeoDbClient = GeoDbCardClient & {
	/** Stop every worker. */
	close(): void
}

export function createPipeGeoDbClient(
	config: GeoDbCardConfig,
	options: {spawn?: SpawnPipe; now?: () => number; keepWarmMs?: number; grantTtlMs?: number} = {},
): PipeGeoDbClient {
	const spawn = options.spawn ?? defaultSpawn
	const keepWarmMs = options.keepWarmMs ?? KEEP_WARM_MS
	const grantTtlMs = options.grantTtlMs ?? (config.grantTtlS ? config.grantTtlS * 1000 : GRANT_TTL_MS)
	// Replace an idle worker this old; a short lifetime (tests) keeps at least half of it.
	const recycleAtMs = Math.max(grantTtlMs - RECYCLE_MARGIN_MS, grantTtlMs / 2)
	const now = options.now ?? Date.now
	const slots: Slot[] = Array.from({length: Math.max(1, config.poolSize)}, () => ({
		child: null,
		ready: false,
		buffer: Buffer.alloc(0),
		request: null,
		stderr: "",
		activeAt: 0,
		readyAt: 0,
		lastSpace: null,
	}))
	const queue: Request[] = []
	const maxQueue = slots.length * 64
	let backoffUntil = 0
	let retryTimes: number[] = []
	/** The Space of the pool's last read, which a worker that has read nothing yet warms with. */
	let lastSpace: string | null = null

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
	function reset(slot: Slot, failure: GeoDbError | null, retry = false, why: WorkerEndReason | null = null) {
		const child = slot.child
		const request = slot.request
		const neverReady = !slot.ready
		slot.child = null
		slot.ready = false
		slot.buffer = Buffer.alloc(0)
		slot.request = null
		if (child) {
			child.kill("SIGKILL")
			if (why) recordWorkerEnd(why)
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

	function start(slot: Slot, why: "demand" | "ahead") {
		let child: PipeChild
		recordWorkerStart(why)
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
			reset(slot, new GeoDbError("client_failed", `geodb-client ${what}: ${detail}`.slice(0, 400)), true, "exit")
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
				reset(slot, new GeoDbError("client_failed", `geodb-client did not start: ${rest}`), false, "broken")
				return
			}
			slot.ready = true
			slot.activeAt = now()
			slot.readyAt = slot.activeAt
			slot.buffer = slot.buffer.subarray(end + 1)
			if (slot.request) send(slot, slot.request)
		}
		// Frames: `+<n>\n<n bytes>` or `-<n>\n<n bytes>`, one per request sent.
		for (;;) {
			const end = slot.buffer.indexOf(10)
			if (end < 0) return
			const header = FRAME.exec(slot.buffer.subarray(0, end).toString("utf8"))
			const request = slot.request
			if (!header || !request) {
				reset(slot, new GeoDbError("client_failed", "geodb-client broke its framing"), false, "broken")
				return
			}
			const size = Number(header[2])
			if (slot.buffer.length < end + 1 + size) return
			const payload = slot.buffer.subarray(end + 1, end + 1 + size).toString("utf8")
			slot.buffer = slot.buffer.subarray(end + 1 + size)
			if (header[1] === "-") {
				// A refusal ends the worker's session; the next request starts a fresh one. Only a refusal
				// that names a dead connection or token is retried; the rest are the request's own.
				const dead = DEAD_SESSION.test(payload)
				reset(
					slot,
					new GeoDbError("client_failed", payload.slice(0, 300)),
					dead,
					dead ? "dead_session" : "refusal",
				)
				return
			}
			slot.request = null
			slot.activeAt = now()
			const answered = performance.now()
			let outcome: GeoCardRow[] | GeoDbError
			try {
				outcome = request.page
					? (pageRowsFromGeoDbOutput(payload) as unknown as GeoCardRow[])
					: request.cards
						? rowsFromGeoDbOutput(payload, null, {names: request.names, spaces: request.spaces})
						: request.backlinks
							? backlinkRowsFromGeoDbOutput(payload, request.entityId, {spaces: request.spaces})
							: rowsFromGeoDbOutput(payload, request.entityId, {
									names: request.names,
									spaces: request.spaces,
								})
			} catch (error) {
				outcome = new GeoDbError("client_failed", String(error))
			}
			if (!request.keepWarm && request.sentAt !== null) {
				const shape = shapeOf(request)
				observeReadStage("wait", shape, (request.sentAt - request.submittedAt) / 1000)
				observeReadStage("answer", shape, (answered - request.sentAt) / 1000)
				observeReadStage("parse", shape, (performance.now() - answered) / 1000)
			}
			settle(request, outcome)
			pump()
		}
	}

	function send(slot: Slot, request: Request) {
		request.sentAt = performance.now()
		slot.child?.stdin.write(request.line)
	}

	/** The idle slot a request should take: a ready worker, else one still starting, else an empty slot. */
	function idleSlot(): Slot | undefined {
		let starting: Slot | undefined
		let empty: Slot | undefined
		for (const s of slots) {
			if (s.request !== null) continue
			if (s.child && s.ready) return s
			if (s.child) starting ??= s
			else empty ??= s
		}
		return starting ?? empty
	}

	/** Hand queued requests to idle slots. */
	function pump() {
		while (queue.length > 0) {
			const slot = idleSlot()
			if (!slot) return
			const request = queue.shift() as Request
			assign(slot, request)
		}
	}

	function assign(slot: Slot, request: Request) {
		slot.request = request
		if (!request.keepWarm) {
			slot.lastSpace = request.line.slice(0, request.line.indexOf("\t")).split(",")[0] ?? null
			lastSpace = slot.lastSpace
		}
		request.slot = slot
		if (!slot.child) start(slot, "demand")
		else if (slot.ready) send(slot, request)
	}

	function onTimeout(request: Request) {
		const queued = queue.indexOf(request)
		if (queued >= 0) queue.splice(queued, 1)
		const slot = request.slot
		const failure = new GeoDbError("timeout", `geo-db read exceeded ${request.timeoutMs ?? config.timeoutMs} ms`)
		if (slot && slot.request === request) reset(slot, failure, false, "timeout")
		else settle(request, failure)
	}

	function submit(
		line: string,
		entityId: string,
		names: boolean,
		spaces: boolean,
		backlinks = false,
		page = false,
		cards = false,
		timeoutMs?: number,
	): Promise<GeoCardRow[]> {
		if (now() < backoffUntil) {
			return Promise.reject(new GeoDbError("client_failed", "geodb-client is not starting; backing off"))
		}
		return new Promise((resolve, reject) => {
			const request: Request = {
				line,
				entityId,
				names,
				spaces,
				...(backlinks ? {backlinks} : {}),
				...(page ? {page} : {}),
				...(cards ? {cards} : {}),
				...(timeoutMs ? {timeoutMs} : {}),
				resolve,
				reject,
				retried: false,
				timer: setTimeout(() => onTimeout(request), timeoutMs ?? config.timeoutMs),
				slot: null,
				submittedAt: performance.now(),
				sentAt: null,
			}
			const idle = idleSlot()
			if (idle) assign(idle, request)
			else if (queue.length >= maxQueue) {
				clearTimeout(request.timer)
				reject(new GeoDbError("overloaded", `${queue.length} geo-db reads already waiting`))
			} else queue.push(request)
		})
	}

	/**
	 * Keep every worker ready ahead of need, once the pool has served a read: warm an idle connection, replace
	 * an idle worker before its token expires, and start a worker in a slot whose worker died. One start per
	 * pass, so a pool whose workers keep dying grows back slowly rather than in a burst.
	 */
	function maintain() {
		if (lastSpace === null || now() < backoffUntil) return
		let started = false
		for (const slot of slots) {
			if (slot.request !== null) continue
			if (slot.child === null) {
				if (started) continue
				started = true
				start(slot, "ahead")
				continue
			}
			if (!slot.ready) continue
			if (now() - slot.readyAt >= recycleAtMs) {
				if (started) continue
				started = true
				reset(slot, null, false, "recycled")
				start(slot, "ahead")
				continue
			}
			if (now() - slot.activeAt >= keepWarmMs) warm(slot, slot.lastSpace ?? lastSpace)
		}
	}

	/** One read of a card no entity has down the slot's connection, so it does not idle out. */
	function warm(slot: Slot, space: string) {
		const request: Request = {
			line: `${space}\t${cardDocument(NO_ENTITY)}\n`,
			entityId: NO_ENTITY,
			names: false,
			spaces: false,
			resolve: () => {},
			reject: () => {},
			// No caller waits on it, so a dead worker under it is not worth a retry.
			retried: true,
			keepWarm: true,
			timer: setTimeout(() => onTimeout(request), config.timeoutMs),
			slot: null,
			submittedAt: performance.now(),
			sentAt: null,
		}
		assign(slot, request)
	}
	const warmTimer = setInterval(maintain, Math.max(1, Math.floor(keepWarmMs / 5)))
	warmTimer.unref()

	return {
		readCard({geoSpaceId, entityId, names = false}) {
			if (!HEX32.test(geoSpaceId) || !HEX32.test(entityId)) {
				return Promise.reject(new GeoDbError("bad_request", "ids must be 32 hex digits"))
			}
			return submit(`${geoSpaceId}\t${cardDocument(entityId, {names})}\n`, entityId, names, false)
		},
		readCardSpaces({geoSpaceIds, entityId, names = false, timeoutMs}) {
			if (!HEX32.test(entityId) || geoSpaceIds.length === 0 || geoSpaceIds.length > MAX_LINE_SPACES)
				return Promise.reject(new GeoDbError("bad_request", "one entity id and 1 to 1024 space ids"))
			if (!geoSpaceIds.every((id) => HEX32.test(id)) || new Set(geoSpaceIds).size !== geoSpaceIds.length)
				return Promise.reject(new GeoDbError("bad_request", "space ids must be distinct 32 hex digits"))
			// One Space prints a bare statement id, several print `<space>:<statement>`: only a list of two or
			// more is the multi-Space form.
			const spaces = geoSpaceIds.length > 1
			return submit(
				`${geoSpaceIds.join(",")}\t${cardDocument(entityId, {names})}\n`,
				entityId,
				names,
				spaces,
				false,
				false,
				false,
				timeoutMs,
			)
		},
		async readCards({geoSpaceIds, entityIds, names = false}) {
			if (entityIds.length === 0 || geoSpaceIds.length === 0 || geoSpaceIds.length > MAX_LINE_SPACES)
				throw new GeoDbError("bad_request", "entity ids and 1 to 1024 space ids")
			if (!entityIds.every((id) => HEX32.test(id)) || !geoSpaceIds.every((id) => HEX32.test(id)))
				throw new GeoDbError("bad_request", "ids must be 32 hex digits")
			if (new Set(geoSpaceIds).size !== geoSpaceIds.length)
				throw new GeoDbError("bad_request", "space ids must be distinct 32 hex digits")
			const distinct = [...new Set(entityIds)]
			const chunks: string[][] = []
			for (let at = 0; at < distinct.length; at += MAX_CARD_IDS)
				chunks.push(distinct.slice(at, at + MAX_CARD_IDS))
			const spaces = geoSpaceIds.length > 1
			const answers = await Promise.all(
				chunks.map((chunk) =>
					submit(
						`${geoSpaceIds.join(",")}\t${cardsDocument(chunk, {names})}\n`,
						NO_ENTITY,
						names,
						spaces,
						false,
						false,
						true,
					),
				),
			)
			return answers.flat()
		},
		readBacklinks({geoSpaceIds, entityId, limit}) {
			if (!HEX32.test(entityId) || geoSpaceIds.length === 0 || geoSpaceIds.length > MAX_LINE_SPACES)
				return Promise.reject(new GeoDbError("bad_request", "one entity id and 1 to 1024 space ids"))
			if (!geoSpaceIds.every((id) => HEX32.test(id)) || new Set(geoSpaceIds).size !== geoSpaceIds.length)
				return Promise.reject(new GeoDbError("bad_request", "space ids must be distinct 32 hex digits"))
			if (!Number.isInteger(limit) || limit < 1 || limit > MAX_BACKLINKS)
				return Promise.reject(new GeoDbError("bad_request", `backlinks limit must be 1 to ${MAX_BACKLINKS}`))
			const spaces = geoSpaceIds.length > 1
			const line = `${geoSpaceIds.join(",")}\t${cardDocument(entityId, {backlinks: limit})}\n`
			return submit(line, entityId, false, spaces, true) as Promise<GeoBacklinkRow[]>
		},
		readEntitiesPage({geoSpaceIds, ...request}) {
			if (geoSpaceIds.length === 0 || geoSpaceIds.length > MAX_LINE_SPACES)
				return Promise.reject(new GeoDbError("bad_request", "1 to 1024 space ids"))
			if (!geoSpaceIds.every((id) => HEX32.test(id)) || new Set(geoSpaceIds).size !== geoSpaceIds.length)
				return Promise.reject(new GeoDbError("bad_request", "space ids must be distinct 32 hex digits"))
			if (!HEX32.test(request.typeId) || !HEX32.test(request.field))
				return Promise.reject(new GeoDbError("bad_request", "type and field ids must be 32 hex digits"))
			if (!Number.isInteger(request.first) || request.first < 1 || request.first > MAX_PAGE)
				return Promise.reject(new GeoDbError("bad_request", `a page asks for 1 to ${MAX_PAGE} entities`))
			const line = `${geoSpaceIds.join(",")}\t${entitiesPageDocument(request)}\n`
			return submit(line, NO_ENTITY, false, geoSpaceIds.length > 1, false, true) as unknown as Promise<
				GeoPageRow[]
			>
		},
		close() {
			clearInterval(warmTimer)
			for (const slot of slots) reset(slot, new GeoDbError("client_failed", "client closed"))
			for (const request of queue.splice(0)) settle(request, new GeoDbError("client_failed", "client closed"))
		},
	}
}
