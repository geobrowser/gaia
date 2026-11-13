/**
 * The feeder's hands on geo-db: `geodb-client` for founding Spaces (`found`), applies and reads, over TCP.
 * Every call is its own connection, so a call after a founding already holds grants over the new Space.
 */
import {spawn} from "node:child_process"
import {mkdirSync, unlinkSync, writeFileSync} from "node:fs"

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
 * Sorts a failed apply. `exhausted` is geo-db out of a per-process resource (only a serve restart frees it), `transient`
 * is geo-db not reachable right now, and everything else is a verdict on the edit itself.
 */
export function classify(message: string): Failure {
	if (/ResourceExhausted/.test(message)) return "exhausted"
	if (/ConnectionRefused|connection refused|ConnectionResetByPeer|BrokenPipe|FileNotFound|Timeout|timed out|Unavailable|AddressNotAvailable|NetworkUnreachable|SessionExpired|\bkilled\b/i.test(message))
		return "transient"
	// geodb-client reports a failed connect as `--binding <file>: connect: IoError` (serve restarting: the binding's
	// address is gone or not listening yet). Nothing was sent, so the apply is safe to repeat.
	if (/connect: IoError/.test(message)) return "transient"
	// The same failed connect when nothing is listening at the binding's address (serve down or rolling): geodb-client
	// prints `error: Refused\nconnect: Refused`. 192 intents were dead-lettered on this text on 2026-11-12. Anchored on
	// `connect:` because a bare `Refused` can also be geo-db's verdict on an intent.
	if (/connect: Refused/.test(message)) return "transient"
	// The rest of what geodb-client can print that leaves an apply safe to repeat (the intent carries an idempotency
	// key). Enumerated against the pinned client in __tests__/errorTable.test.ts, which fails on any name it lacks:
	// a connection that dropped or timed out mid-apply, and geo-db's own retryable refusals (`refused: <category>
	// <code>`: facade `retryable`, plus OutcomeUnknown, which its callers resolve by re-sending the transaction).
	if (/TransportFailed|TransportClosed|DeadlineExceeded|TimedOut|EndOfStream|UnknownHostName|TemporaryNameServerFailure/.test(message))
		return "transient"
	if (/StoreLocked|PredecessorVersionMismatch|WatermarkNotYetReached|OutcomeUnknown|refused: \w+ IoError/.test(message))
		return "transient"
	return "permanent"
}

export const spaceIds = (listing: string): string[] =>
	listing
		.split("\n")
		.filter(Boolean)
		.map((l) => l.split("\t")[0] ?? "")
		.filter((s) => /^[0-9a-f]{32}$/.test(s))

/** geo-db was not open to answer a founding, so a refusal of it says nothing about the Space cap. Retry it. */
export class NodeOpeningError extends Error {}

export const SPACES_PROBE_TIMEOUT_MS = 3_000

export class GeoDb {
	private seq = 0

	constructor(
		private readonly opts: {runDir: string},
		private readonly run: Runner,
		private readonly sleep: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms)),
	) {
		mkdirSync(opts.runDir, {recursive: true})
	}

	/**
	 * The Space ids geo-db serves, or null while it does not answer. The default kill timeout is short: a healthy
	 * serve answers in well under a second, and after a roll a connect to a stale pod IP hangs until killed, which
	 * keeps `waitReady` (and so apply) dark for the whole timeout past Ready.
	 */
	async spaces(timeoutMs = SPACES_PROBE_TIMEOUT_MS): Promise<string[] | null> {
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

	/**
	 * Found the Space `nonce` names under the node's sealer, online (wire FoundSpace through `geodb-client found`).
	 * Idempotent: an already-founded Space answers the same id. Needs the writer principal's `publish` policy to
	 * cover the id (the binding gives it `publish=*`).
	 *
	 * The caller reads a `ResourceExhausted` failure as the Space cap (`ensureSpace` refuses the Space for good), but
	 * the client prints the same `refused: ResourceExhausted ResourceExhausted` for a node still opening its store
	 * after its own ~30 s of dials. So an exhausted answer is believed only once `spaces` shows the node open and a
	 * second founding is refused again; a node that is not open yet raises `NodeOpeningError`, which names no
	 * `ResourceExhausted` and so is retried like any outage.
	 */
	async found(sealer: string, nonce: string): Promise<string> {
		const first = await this.foundOnce(sealer, nonce)
		if (typeof first !== "object") return first
		if (classify(first.message) !== "exhausted") throw first
		if ((await this.spaces()) === null) {
			throw new NodeOpeningError(`found ${nonce}: geo-db is not answering yet (still opening its store); not the Space cap`)
		}
		const second = await this.foundOnce(sealer, nonce)
		if (typeof second !== "object") return second
		throw second
	}

	private async foundOnce(sealer: string, nonce: string): Promise<string | Error> {
		const r = await this.run(["found", "--sealer", sealer, nonce], {timeoutMs: 60_000})
		const id = /^founded: ([0-9a-f]{32})\s*$/m.exec(r.out)?.[1]
		if (r.rc !== 0 || !id) return new Error(`found ${nonce}: rc=${r.rc} ${(r.err + r.out).slice(-300)}`)
		return id
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
