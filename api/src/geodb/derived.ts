/**
 * geo-db read client for the derived class (plan 10.14): `get` one row of a collection by key, and `scan` one page of a group.
 *
 * One held `geodb-client derived pipe` process: it opens one authenticated connection, prints `ready`, then answers one
 * tab-separated request line at a time with a frame, `+<n>\n<n bytes>` or `-<n>\n<n bytes>` (a refusal). A get answers
 * `<group hex>\t<score>\t<value hex>` or `none`. Needs only an authenticated session (the writer grant is for put and
 * delete), so no new key is needed. Requests run one at a time in order; a timeout, a refusal or a process exit kills the
 * worker (its connection may owe a reply) and rejects the request, and the next request starts a fresh worker. No retry: the
 * only caller is a background shadow, which counts a failure and moves on.
 *
 * Reuses the card client's config, `PipeChild` seam and `GeoDbError`. No wire change: DerivedGet (0x3B) is already served.
 */

import {spawn as nodeSpawn} from "node:child_process"
import {GeoDbError, type PipeChild, type SpawnPipe} from "./client"
import type {GeoDbCardConfig} from "./config"

/** One derived row as `get` answers it. */
export type DerivedRow = {
	/** The row's group, 32 hex digits. */
	group: string
	score: number
	value: Buffer
}

/** One row of a `scan`: the score, the key and the value, as hex. */
export type DerivedScanRow = {score: number; keyHex: string; valueHex: string}

/** What a `scan` read: the rows in score order (descending, then key ascending) and whether the group held more. */
export type DerivedScan = {rows: DerivedScanRow[]; more: boolean}

export type DerivedReader = {
	/** The row at `key` (hex) of `collection`, or null when the collection holds none. */
	get(collection: number, keyHex: string): Promise<DerivedRow | null>
	/** Stop the worker and reject what is queued. */
	close(): void
}

/** A reader that can also scan; `createDerivedReader` returns one. Kept apart so a get-only fake still types as a `DerivedReader`. */
export type DerivedScanReader = DerivedReader & {
	/**
	 * The first `first` rows (1 to 512) of `group` (hex, up to 32 digits) in `collection`, score descending. One page:
	 * `more` says the group holds rows past them.
	 */
	scan(collection: number, groupHex: string, first: number): Promise<DerivedScan>
}

type ReaderConfig = Pick<GeoDbCardConfig, "clientBin" | "keyPath" | "bindingPath" | "timeoutMs">

const defaultSpawn: SpawnPipe = (config) =>
	nodeSpawn(config.clientBin, ["--key", config.keyPath, "--binding", config.bindingPath, "derived", "pipe"], {
		stdio: ["pipe", "pipe", "pipe"],
	}) as unknown as PipeChild

/** The most rows one scan page holds (geo-db `wire_derived.page_max`). */
export const MAX_SCAN_ROWS = 512
const FRAME = /^([+-])(\d{1,9})$/
const STDERR_TAIL = 300
const START_BACKOFF_MS = 1000

/** The row a `get` answer body names: `none`, or `<group hex>\t<score>\t<value hex>`. Throws on anything else. */
export function parseDerivedGet(body: string): DerivedRow | null {
	const text = body.replace(/\n$/, "")
	if (text === "none") return null
	const parts = text.split("\t")
	const [group = "", score = "", value = ""] = parts
	if (parts.length !== 3 || !/^[0-9a-f]{32}$/.test(group) || !/^([0-9a-f]{2})*$/.test(value))
		throw new GeoDbError("client_failed", `derived get answer is malformed: ${text.slice(0, 80)}`)
	const n = Number(score)
	if (score === "" || !Number.isFinite(n))
		throw new GeoDbError("client_failed", `derived get score is not a number: ${score.slice(0, 40)}`)
	return {group, score: n, value: Buffer.from(value, "hex")}
}

/** The rows a `scan` answer body names: one `<score>\t<key hex>\t<value hex>` a line, then `more` when the page was cut. */
export function parseDerivedScan(body: string): DerivedScan {
	const rows: DerivedScanRow[] = []
	let more = false
	for (const line of body.split("\n")) {
		if (line === "") continue
		if (line === "more") {
			more = true
			continue
		}
		const [score = "", keyHex = "", valueHex = "", ...rest] = line.split("\t")
		const n = Number(score)
		if (
			rest.length > 0 ||
			score === "" ||
			!Number.isFinite(n) ||
			!/^([0-9a-f]{2})+$/.test(keyHex) ||
			!/^([0-9a-f]{2})*$/.test(valueHex)
		)
			throw new GeoDbError("client_failed", `derived scan row is malformed: ${line.slice(0, 80)}`)
		rows.push({score: n, keyHex, valueHex})
	}
	return {rows, more}
}

type Pending = {
	line: string
	resolve: (body: string) => void
	reject: (error: GeoDbError) => void
}

export function createDerivedReader(
	config: ReaderConfig,
	options: {spawn?: SpawnPipe; now?: () => number} = {},
): DerivedScanReader {
	const spawn = options.spawn ?? defaultSpawn
	const now = options.now ?? Date.now
	const queue: Pending[] = []
	let child: PipeChild | null = null
	let ready = false
	let buffer: Buffer = Buffer.alloc(0)
	let current: Pending | null = null
	let timer: ReturnType<typeof setTimeout> | null = null
	let stderr = ""
	let backoffUntil = 0

	/** Kill the worker and reject the request it owed. `started` false (it never became ready) also rejects the queue. */
	function stop(error: GeoDbError, started: boolean) {
		const dead = child
		child = null
		ready = false
		buffer = Buffer.alloc(0)
		if (timer) clearTimeout(timer)
		timer = null
		try {
			dead?.kill("SIGKILL")
		} catch {}
		if (!started) backoffUntil = now() + START_BACKOFF_MS
		const failed = current
		current = null
		failed?.reject(error)
		if (!started) for (const p of queue.splice(0)) p.reject(error)
		else pump()
	}

	function start() {
		stderr = ""
		const c = spawn(config as GeoDbCardConfig)
		child = c
		c.stdout.on("data", (chunk: Buffer) => onData(c, chunk))
		c.stderr?.on("data", (chunk: Buffer) => {
			stderr = (stderr + chunk.toString("utf8")).slice(-STDERR_TAIL)
		})
		const gone = (what: string) => {
			if (child !== c) return
			stop(new GeoDbError("client_failed", `geodb-client derived pipe ${what}: ${stderr}`.slice(0, 400)), ready)
		}
		c.on("exit", (code, signal) => gone(`exited (${String(signal ?? code)})`))
		c.on("error", (error) => gone(`failed: ${String(error)}`))
	}

	function send() {
		if (!child || !ready || current || queue.length === 0) return
		current = queue.shift() as Pending
		child.stdin.write(`${current.line}\n`)
		timer = setTimeout(() => {
			stop(new GeoDbError("timeout", `derived read took longer than ${config.timeoutMs} ms`), true)
		}, config.timeoutMs)
	}

	function pump() {
		if (queue.length === 0) return
		if (!child) {
			if (now() < backoffUntil) {
				const error = new GeoDbError(
					"client_failed",
					"geodb-client derived pipe is backing off after a failed start",
				)
				for (const p of queue.splice(0)) p.reject(error)
				return
			}
			start()
		}
		send()
	}

	function onData(from: PipeChild, chunk: Buffer) {
		if (child !== from) return
		buffer = buffer.length === 0 ? chunk : Buffer.concat([buffer, chunk])
		if (!ready) {
			const end = buffer.indexOf(10)
			if (end < 0) return
			const first = buffer.subarray(0, end).toString("utf8")
			if (first !== "ready") {
				stop(
					new GeoDbError(
						"client_failed",
						`geodb-client did not start: ${buffer.toString("utf8").slice(0, 300)}`,
					),
					false,
				)
				return
			}
			ready = true
			buffer = buffer.subarray(end + 1)
			send()
		}
		for (;;) {
			const end = buffer.indexOf(10)
			if (end < 0) return
			const head = FRAME.exec(buffer.subarray(0, end).toString("utf8"))
			if (!head) {
				stop(new GeoDbError("client_failed", "derived pipe answered a malformed frame"), true)
				return
			}
			const length = Number(head[2])
			if (buffer.length < end + 1 + length) return
			const body = buffer.subarray(end + 1, end + 1 + length).toString("utf8")
			buffer = buffer.subarray(end + 1 + length)
			const answered = current
			if (!answered) {
				stop(new GeoDbError("client_failed", "derived pipe answered with no request owed"), true)
				return
			}
			if (head[1] === "-") {
				// A refusal ends the session on the server's side; do not reuse it.
				stop(new GeoDbError("client_failed", `derived read refused: ${body.slice(0, 200)}`), true)
				return
			}
			current = null
			if (timer) clearTimeout(timer)
			timer = null
			answered.resolve(body)
			send()
		}
	}

	return {
		get(collection, keyHex) {
			if (!Number.isInteger(collection) || collection < 0 || !/^[0-9a-f]+$/.test(keyHex))
				return Promise.reject(
					new GeoDbError("bad_request", "derived get wants a collection number and a hex key"),
				)
			return new Promise<string>((resolve, reject) => {
				queue.push({line: `get\t${collection}\thex:${keyHex}`, resolve, reject})
				pump()
			}).then(parseDerivedGet)
		},
		scan(collection, groupHex, first) {
			if (
				!Number.isInteger(collection) ||
				collection < 0 ||
				!/^[0-9a-f]{1,32}$/.test(groupHex) ||
				!Number.isInteger(first) ||
				first < 1 ||
				first > MAX_SCAN_ROWS
			)
				return Promise.reject(
					new GeoDbError("bad_request", "derived scan wants a collection, a hex group and 1 to 512 rows"),
				)
			return new Promise<string>((resolve, reject) => {
				queue.push({line: `scan\t${collection}\t0x${groupHex}\t${first}\t1`, resolve, reject})
				pump()
			}).then(parseDerivedScan)
		},
		close() {
			const error = new GeoDbError("client_failed", "derived reader closed")
			for (const p of queue.splice(0)) p.reject(error)
			stop(error, false)
		},
	}
}
