import {type GeoDb, classify} from "./geodb.js"
import {log} from "./log.js"
import type {Metrics} from "./metrics.js"
import type {SpaceRecord, Store} from "./state.js"
import {zonDoc} from "./translate.js"

export type ApplyDeps = {
	db: Pick<GeoDb, "apply" | "waitReady" | "restart">
	store: Store
	metrics: Metrics
	/** Keeps the Kafka session alive while geo-db is slow or down. */
	beat: () => Promise<void>
	sleep?: (ms: number) => Promise<void>
	signal?: AbortSignal
}

export type Meta = {edit?: string; space: string; partition: number; offset: string}

const defaultSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

/**
 * Apply `intents[from..]` of one message in chunks of at most `chunk`, calling `progress(n)` after each
 * range that is done (applied, or dead-lettered because geo-db refused it on its merits).
 *
 * - Transient failures (geo-db down, restarting, out of a resource) retry forever with backoff; the feeder
 *   never skips an edit because geo-db was unavailable.
 * - A refused range is bisected until the refused intent is alone, which is written to errors.jsonl and skipped.
 */
export async function applyIntents(
	deps: ApplyDeps,
	space: SpaceRecord & {geo: string},
	meta: Meta,
	intents: string[],
	chunk: number,
	from: number,
	progress: (done: number) => void,
): Promise<{applied: number; deadLettered: number}> {
	let applied = 0
	let deadLettered = 0

	const range = async (lo: number, hi: number): Promise<void> => {
		const slice = intents.slice(lo, hi)
		const outcome = await tryApply(deps, space, slice)
		if (outcome === "ok") {
			applied += slice.length
			progress(hi)
			return
		}
		if (slice.length > 1) {
			const mid = lo + (slice.length >> 1)
			await range(lo, mid)
			await range(mid, hi)
			return
		}
		deps.store.deadLetter({...meta, intent: slice[0], err: outcome.error})
		deps.metrics.deadLetters.inc({kind: "intent"})
		deps.metrics.chunks.inc({result: "dead_letter"})
		log("error", "intent refused by geo-db, skipped", {...meta, error: outcome.error.slice(0, 300)})
		deadLettered += 1
		progress(hi)
	}

	for (let lo = from; lo < intents.length; lo += chunk) await range(lo, Math.min(lo + chunk, intents.length))
	return {applied, deadLettered}
}

async function tryApply(
	deps: ApplyDeps,
	space: SpaceRecord & {geo: string},
	slice: string[],
): Promise<"ok" | {error: string}> {
	const sleep = deps.sleep ?? defaultSleep
	let delay = 1000
	for (;;) {
		if (deps.signal?.aborted) throw new Error("shutting down")
		const t0 = Date.now()
		const r = await deps.db.apply(zonDoc(space.geo, space.keyed ? null : space.nonce, slice))
		if (r.rc === 0) {
			deps.metrics.apply.observe((Date.now() - t0) / 1000)
			deps.metrics.chunks.inc({result: "ok"})
			deps.metrics.intents.inc({}, slice.length)
			if (!space.keyed) {
				space.keyed = true
				deps.store.saveSpaces()
			}
			return "ok"
		}
		const message = (r.err + r.out).slice(-600)
		const kind = classify(r.rc === null ? `${message} killed` : message)
		if (kind === "permanent") return {error: message}
		deps.metrics.chunks.inc({result: "retry"})
		log("warn", "apply failed, retrying", {kind, error: message.slice(0, 300), delayMs: delay})
		if (kind === "exhausted") {
			deps.metrics.serveRestarts.inc({reason: "exhausted"})
			await deps.db.restart(null)
		} else {
			await sleep(delay)
			delay = Math.min(delay * 2, 30_000)
			await deps.db.waitReady()
		}
		await deps.beat()
	}
}
