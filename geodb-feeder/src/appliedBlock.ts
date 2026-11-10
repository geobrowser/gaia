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
 * Off unless GEODB_APPLIED_BLOCK is set (config.ts). The feeder's own key has no `derived=` grant, so it writes with the
 * `geodb-cursor-key` (`derived=10`), mounted for it (GEODB_APPLIED_BLOCK_KEY_PATH).
 */
import type {Runner} from "./geodb.js"
import {log} from "./log.js"

/** The writer id the row is keyed by. */
export const APPLIED_BLOCK_KEY = "geodb-feeder-applied"
/** `cursors`, the derived collection for an indexer's chain position. */
export const CURSORS_COLLECTION = 10
/** The `cursors` collection's value bound, bytes. */
const VALUE_MAX = 1024

export class AppliedBlockTracker {
	private readonly blocks = new Map<number, number>()

	/** An edit of `partition` was applied; `block` is its chain block, null when the edit carries none. */
	observe(partition: number, block: number | null): void {
		if (block === null || !Number.isSafeInteger(block) || block < 0) return
		this.blocks.set(partition, block)
	}

	/** The lowest partition's last applied block; null before any edit with a block. */
	watermark(): number | null {
		let low: number | null = null
		for (const b of this.blocks.values()) if (low === null || b < low) low = b
		return low
	}

	/** The row's value, or null before there is a watermark. */
	value(): string | null {
		const w = this.watermark()
		if (w === null) return null
		const parts = [...this.blocks.entries()].sort(([a], [b]) => a - b)
		return `${w}:${JSON.stringify(Object.fromEntries(parts.map(([p, b]) => [String(p), b])))}`
	}
}

export type AppliedBlockWriter = {
	/** An edit was applied (offset already marked done). Writes at most once per interval. Never throws. */
	note(partition: number, block: number | null): Promise<void>
	/** Writes the latest value if it has not been written, e.g. at the end of a batch. Never throws. */
	flush(): Promise<void>
}

export function createAppliedBlockWriter(opts: {
	run: Runner
	collection?: number
	minIntervalMs?: number
	now?: () => number
	/** Called once per attempted put with whether it applied. */
	onResult?: (ok: boolean) => void
}): AppliedBlockWriter {
	const tracker = new AppliedBlockTracker()
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
	}
}
