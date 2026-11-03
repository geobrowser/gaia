/**
 * The feeder's hands on geo-db: `geodb-client` for applies and reads, and the control files the
 * supervisor next to `geodb-serve` watches (see geodb/supervisor.sh) to restart it. A restart is how a
 * Space is founded, because `geodb-serve` accepts one `--found` nonce per start.
 */
import {spawn} from "node:child_process"
import {existsSync, mkdirSync, unlinkSync, writeFileSync} from "node:fs"

export type RunResult = {rc: number | null; out: string; err: string}
export type Runner = (args: string[], opts?: {timeoutMs?: number}) => Promise<RunResult>

export function spawnRunner(opts: {clientBin: string; keyPath: string; bindingPath: string; cwd: string}): Runner {
	return (args, o = {}) =>
		new Promise((resolve) => {
			const p = spawn(opts.clientBin, ["--key", opts.keyPath, "--binding", opts.bindingPath, ...args], {cwd: opts.cwd})
			let out = ""
			let err = ""
			const timer = setTimeout(() => p.kill("SIGKILL"), o.timeoutMs ?? 300_000)
			p.stdout.on("data", (d) => {
				out += d
			})
			p.stderr.on("data", (d) => {
				err += d
			})
			p.on("error", (e) => {
				clearTimeout(timer)
				resolve({rc: null, out, err: `${err}${String(e)}`})
			})
			p.on("close", (rc) => {
				clearTimeout(timer)
				resolve({rc, out, err})
			})
		})
}

export type Failure = "exhausted" | "transient" | "permanent"

/**
 * Sorts a failed apply. `exhausted` is geo-db out of a per-process resource (a restart frees it), `transient`
 * is geo-db not reachable right now, and everything else is a verdict on the edit itself.
 */
export function classify(message: string): Failure {
	if (/ResourceExhausted/.test(message)) return "exhausted"
	if (/ConnectionRefused|connection refused|ConnectionResetByPeer|BrokenPipe|FileNotFound|Timeout|timed out|Unavailable|AddressNotAvailable|NetworkUnreachable|SessionExpired|\bkilled\b/i.test(message))
		return "transient"
	return "permanent"
}

export const spaceIds = (listing: string): string[] =>
	listing
		.split("\n")
		.filter(Boolean)
		.map((l) => l.split("\t")[0] ?? "")
		.filter((s) => /^[0-9a-f]{32}$/.test(s))

export class GeoDb {
	private seq = 0

	constructor(
		private readonly opts: {runDir: string; ctlDir: string},
		private readonly run: Runner,
		private readonly sleep: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms)),
	) {
		mkdirSync(opts.runDir, {recursive: true})
		mkdirSync(opts.ctlDir, {recursive: true})
	}

	/** The Space ids geo-db serves, or null while it does not answer. */
	async spaces(timeoutMs = 20_000): Promise<string[] | null> {
		const r = await this.run(["spaces"], {timeoutMs})
		return r.rc === 0 ? spaceIds(r.out) : null
	}

	async waitReady(timeoutMs = 300_000): Promise<string[]> {
		const deadline = Date.now() + timeoutMs
		for (;;) {
			const s = await this.spaces()
			if (s) return s
			if (Date.now() > deadline) throw new Error("geodb-serve did not come up")
			await this.sleep(1000)
		}
	}

	/** Ask the supervisor to restart geodb-serve, founding `nonce` on the way up, and wait until it answers. */
	async restart(nonce: string | null, timeoutMs = 300_000): Promise<string[]> {
		const pending = `${this.opts.ctlDir}/pending-found`
		if (nonce) writeFileSync(pending, `${nonce}\n`)
		else if (existsSync(pending)) unlinkSync(pending)
		const flag = `${this.opts.ctlDir}/restart`
		writeFileSync(flag, `${Date.now()}\n`)
		const deadline = Date.now() + timeoutMs
		while (existsSync(flag)) {
			if (Date.now() > deadline) throw new Error("supervisor did not take the restart request")
			await this.sleep(250)
		}
		return this.waitReady(timeoutMs)
	}

	async apply(zon: string): Promise<RunResult> {
		const file = `${this.opts.runDir}/edit-${process.pid}-${this.seq++}.zon`
		writeFileSync(file, zon)
		try {
			return await this.run(["apply", file])
		} finally {
			try {
				unlinkSync(file)
			} catch {}
		}
	}

	/** One entity's rows, for the read-back probe. */
	async readEntity(space: string, entity: string): Promise<boolean> {
		const doc = `.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .id = "${entity}" } } } }`
		const r = await this.run(["query", "--space", space, doc], {timeoutMs: 60_000})
		return r.rc === 0 && r.out.includes(entity)
	}
}
