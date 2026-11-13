/**
 * The chain block the feeder has applied, mirrored into geo-db's derived class for search-indexer's retire sweep
 * (`kg_applied_block`, which reads `max(edit_versions.block_number)` from Postgres today). Design: geo-db plan 10.22
 * section 19.
 *
 * One row of `cursors` (collection 10), key `geodb-feeder-applied`, value `<decimal block>:<per-partition json>`, the
 * shape `geodb-cursor`'s `block_of` parses. The block is the lowest, over the partitions applied from, of each
 * partition's last applied edit's chain block: kafka orders within a partition only, so across partitions the highest
 * is not a watermark and the lowest is. The row is written after all of an edit's chunks have applied, so it
 * can lag the store and never lead it. A replay moves it back with the store. Best effort: a failed put is counted and
 * logged, never fails the apply path.
 *
 * An edit the feeder skipped any part of (an intent geo-db refused, an undecodable message, a Space-cap dead letter)
 * is not in the store, yet later edits of its partition apply and move the partition on. So a dead letter holds its
 * partition at the block below the dead-lettered edit's (`hold`), and the row never claims that block until the hold
 * is released. Holds are written to `holdsPath` (applied-holds.json beside offsets.json) before the dead letter's
 * progress is saved, so a restart keeps them. Nothing releases one automatically: after a replay applies the edit,
 * remove its partition from that file with the feeder at 0 (RUNBOOK "Applied-block holds").
 *
 * Off unless GEODB_APPLIED_BLOCK is set (config.ts). The feeder's own key has no `derived=` grant, so it writes with the
 * `geodb-cursor-key` (`derived=10`), mounted for it (GEODB_APPLIED_BLOCK_KEY_PATH).
 */
import {existsSync, readFileSync} from "node:fs"
import type {Runner} from "./geodb.js"
import {log} from "./log.js"
import {writeFileDurable} from "./state.js"

/** The writer id the row is keyed by. */
export const APPLIED_BLOCK_KEY = "geodb-feeder-applied"
/** `cursors`, the derived collection for an indexer's chain position. */
export const CURSORS_COLLECTION = 10
/** The `cursors` collection's value bound, bytes. */
const VALUE_MAX = 1024

const validBlock = (b: number | null): b is number => b !== null && Number.isSafeInteger(b) && b >= 0

export class AppliedBlockTracker {
	private readonly blocks = new Map<number, number>()
	/** Per partition, the highest block the row may claim: one below its oldest unreplayed dead letter's. */
	private readonly caps = new Map<number, number>()

	constructor(caps: Record<string, number> = {}) {
		for (const [p, c] of Object.entries(caps)) if (Number.isSafeInteger(Number(p)) && validBlock(c)) this.caps.set(Number(p), c)
	}

	/** An edit of `partition` was applied; `block` is its chain block, null when the edit carries none. */
	observe(partition: number, block: number | null): void {
		if (!validBlock(block)) return
		this.blocks.set(partition, block)
	}

	/**
	 * An edit of `partition` was dead-lettered in whole or part; `block` is its chain block, null when unknown (then
	 * the partition is held where it is). Returns whether the hold moved.
	 */
	hold(partition: number, block: number | null): boolean {
		const at = validBlock(block) ? Math.max(0, block - 1) : (this.blocks.get(partition) ?? 0)
		const cap = this.caps.get(partition)
		if (cap !== undefined && cap <= at) return false
		this.caps.set(partition, at)
		return true
	}

	/** The holds, for persisting. */
	holds(): Record<string, number> {
		return Object.fromEntries([...this.caps.entries()].sort(([a], [b]) => a - b).map(([p, c]) => [String(p), c]))
	}

	/** Each partition's claim: its last applied block, never above its hold. */
	private claims(): [number, number][] {
		const out = new Map<number, number>(this.blocks)
		for (const [p, c] of this.caps) out.set(p, Math.min(out.get(p) ?? c, c))
		return [...out.entries()].sort(([a], [b]) => a - b)
	}

	/** The lowest partition's claim; null before any edit with a block (or any hold). */
	watermark(): number | null {
		let low: number | null = null
		for (const [, b] of this.claims()) if (low === null || b < low) low = b
		return low
	}

	/** The row's value, or null before there is a watermark. */
	value(): string | null {
		const w = this.watermark()
		if (w === null) return null
		return `${w}:${JSON.stringify(Object.fromEntries(this.claims().map(([p, b]) => [String(p), b])))}`
	}
}

export type AppliedBlockWriter = {
	/** An edit was applied (offset already marked done). Writes at most once per interval. Never throws. */
	note(partition: number, block: number | null): Promise<void>
	/** Writes the latest value if it has not been written, e.g. at the end of a batch. Never throws. */
	flush(): Promise<void>
	/**
	 * An edit of `partition` at `block` was dead-lettered in whole or part: the row stays below it until the hold is
	 * released by hand. Persisted synchronously before returning, so call it before the dead letter's progress is saved.
	 */
	hold(partition: number, block: number | null): void
}

function loadHolds(path: string | undefined): Record<string, number> {
	if (!path || !existsSync(path)) return {}
	// A hold file that cannot be read must not silently release the holds: fail the start.
	return JSON.parse(readFileSync(path, "utf8")) as Record<string, number>
}

export function createAppliedBlockWriter(opts: {
	run: Runner
	collection?: number
	minIntervalMs?: number
	now?: () => number
	/** Called once per attempted put with whether it applied. */
	onResult?: (ok: boolean) => void
	/** Where holds persist (applied-holds.json in the feeder's state dir). Unset: holds live in memory only. */
	holdsPath?: string
}): AppliedBlockWriter {
	const tracker = new AppliedBlockTracker(loadHolds(opts.holdsPath))
	const collection = opts.collection ?? CURSORS_COLLECTION
	const minInterval = opts.minIntervalMs ?? 5000
	const now = opts.now ?? Date.now
	let written: string | null = null
	let lastAttempt = -Infinity
	let chain: Promise<void> = Promise.resolve()

	const put = async (): Promise<void> => {
		const value = tracker.value()
		if (value === null || value === written) return
		lastAttempt = now()
		const bytes = Buffer.from(value)
		if (bytes.length > VALUE_MAX) {
			log("warn", "applied block value over the cursors bound, skipped", {bytes: bytes.length})
			opts.onResult?.(false)
			return
		}
		try {
			const r = await opts.run(["derived", "put", String(collection), "0", "0", APPLIED_BLOCK_KEY, `hex:${bytes.toString("hex")}`], {timeoutMs: 10_000})
			if (r.rc === 0) {
				written = value
				opts.onResult?.(true)
			} else {
				log("warn", "applied block put failed", {rc: r.rc, err: r.err.slice(0, 200)})
				opts.onResult?.(false)
			}
		} catch (e) {
			log("warn", "applied block put failed", {error: String(e instanceof Error ? e.message : e).slice(0, 200)})
			opts.onResult?.(false)
		}
	}
	// One put at a time, so a slow serve never stacks children behind the apply path.
	const serial = (): Promise<void> => {
		chain = chain.then(put, put)
		return chain
	}

	return {
		note: async (partition, block) => {
			tracker.observe(partition, block)
			if (now() - lastAttempt >= minInterval) await serial()
		},
		flush: async () => {
			await serial()
		},
		hold: (partition, block) => {
			if (!tracker.hold(partition, block)) return
			const holds = tracker.holds()
			log("warn", "applied block held below a dead-lettered edit", {partition, block, holds})
			// Unlike a put, a hold that is not on disk would be lost on restart, so a failed write throws to the apply
			// path, which retries the message rather than skipping it.
			if (opts.holdsPath) writeFileDurable(opts.holdsPath, JSON.stringify(holds))
		},
	}
}
