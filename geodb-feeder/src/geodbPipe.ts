/**
 * Reads one entity's rows from geo-db over a long-lived `geodb-client pipe` (the api's card read path, see
 * api/src/geodb/client.ts): one request line `<geo space>\t<geoql>\n`, one frame back, `+<n>\n<n bytes>` rows
 * or `-<n>\n<n bytes>` a refusal, which also ends the session. A warm connection answers in milliseconds,
 * where `geodb-client query` would start a process and handshake for each entity.
 *
 * One request at a time. A connection holds grants only for the Spaces that existed when it opened, so the
 * caller `reset()`s after founding a Space.
 */
import {type ChildProcessWithoutNullStreams, spawn} from "node:child_process"

export type GeoDbRow = {prop: string; value: string; statement: string; entity: string}

/** geodb-client query output: one row a line, cells (property, value, statement id, entity, weight). */
export function parseRows(output: string): GeoDbRow[] {
	return output
		.split("\n")
		.filter((l) => l !== "")
		.map((l) => l.split("\t"))
		.map((c) => ({prop: c[0] ?? "", value: c[1] ?? "", statement: c[2] ?? "", entity: c[3] ?? ""}))
}

export const cardDocument = (entity: string) =>
	`.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .id = "${entity}" } } } }`

export type EntityReader = {
	/** The rows geo-db holds for `entity` itself (a card query also answers the entities its backlinks name). */
	entityRows(geoSpace: string, entity: string): Promise<GeoDbRow[]>
	reset(): void
}

type Pending = {resolve: (s: string) => void; reject: (e: Error) => void}

export class PipeReader implements EntityReader {
	private child: ChildProcessWithoutNullStreams | null = null
	private buffer: Buffer = Buffer.alloc(0)
	private ready: Promise<void> | null = null
	private pending: Pending | null = null

	constructor(
		private readonly opts: {clientBin: string; keyPath: string; bindingPath: string; timeoutMs?: number},
	) {}

	reset(): void {
		const c = this.child
		this.child = null
		this.ready = null
		this.buffer = Buffer.alloc(0)
		const p = this.pending
		this.pending = null
		p?.reject(new Error("geodb-client pipe reset"))
		c?.kill("SIGKILL")
	}

	private start(): Promise<void> {
		if (this.ready) return this.ready
		const child = spawn(this.opts.clientBin, ["--key", this.opts.keyPath, "--binding", this.opts.bindingPath, "pipe"], {
			stdio: ["pipe", "pipe", "pipe"],
		})
		this.child = child
		let stderr = ""
		child.stderr.on("data", (d: Buffer) => {
			stderr = (stderr + d.toString("utf8")).slice(-300)
		})
		child.stdin.on("error", () => {})
		this.ready = new Promise<void>((resolve, reject) => {
			let started = false
			const fail = (why: string) => {
				if (this.child !== child) return
				const err = new Error(`geodb-client pipe ${why}: ${stderr.trim()}`.slice(0, 400))
				const p = this.pending
				this.reset()
				if (!started) reject(err)
				p?.reject(err)
			}
			child.on("exit", (code, signal) => fail(`exited (${String(signal ?? code)})`))
			child.on("error", (e) => fail(`failed: ${String(e)}`))
			child.stdout.on("data", (d: Buffer) => {
				if (this.child !== child) return
				this.buffer = this.buffer.length ? Buffer.concat([this.buffer, d]) : d
				if (!started) {
					const end = this.buffer.indexOf(10)
					if (end < 0) return
					const first = this.buffer.subarray(0, end).toString("utf8")
					this.buffer = this.buffer.subarray(end + 1)
					if (first !== "ready") return fail(`did not start (${first.slice(0, 100)})`)
					started = true
					resolve()
				}
				this.drain()
			})
		})
		return this.ready
	}

	private drain(): void {
		const end = this.buffer.indexOf(10)
		if (end < 0) return
		const header = /^([+-])(\d{1,9})$/.exec(this.buffer.subarray(0, end).toString("utf8"))
		const p = this.pending
		if (!header || !p) {
			const err = new Error("geodb-client pipe broke its framing")
			this.reset()
			p?.reject(err)
			return
		}
		const size = Number(header[2])
		if (this.buffer.length < end + 1 + size) return
		const payload = this.buffer.subarray(end + 1, end + 1 + size).toString("utf8")
		this.buffer = this.buffer.subarray(end + 1 + size)
		this.pending = null
		if (header[1] === "-") {
			this.reset()
			p.reject(new Error(`geo-db refused the read: ${payload.slice(0, 300)}`))
		} else p.resolve(payload)
	}

	async entityRows(geoSpace: string, entity: string): Promise<GeoDbRow[]> {
		await this.start()
		const child = this.child
		if (!child) throw new Error("geodb-client pipe is not running")
		const timeoutMs = this.opts.timeoutMs ?? 30_000
		const payload = await new Promise<string>((resolve, reject) => {
			const timer = setTimeout(() => {
				this.reset()
				reject(new Error(`geo-db read exceeded ${timeoutMs} ms`))
			}, timeoutMs)
			this.pending = {
				resolve: (s) => {
					clearTimeout(timer)
					resolve(s)
				},
				reject: (e) => {
					clearTimeout(timer)
					reject(e)
				},
			}
			child.stdin.write(`${geoSpace}\t${cardDocument(entity)}\n`)
		})
		return parseRows(payload).filter((r) => r.entity === entity)
	}
}

/** Retry a read a few times on a fresh connection before giving up (the backfill then stops; a rerun resumes). */
export async function readWithRetry(
	reader: EntityReader,
	geoSpace: string,
	entity: string,
	sleep: (ms: number) => Promise<void>,
	attempts = 4,
): Promise<GeoDbRow[]> {
	let last: unknown
	for (let i = 0; i < attempts; i++) {
		try {
			return await reader.entityRows(geoSpace, entity)
		} catch (e) {
			last = e
			reader.reset()
			await sleep(1000 * 2 ** i)
		}
	}
	throw last instanceof Error ? last : new Error(String(last))
}
