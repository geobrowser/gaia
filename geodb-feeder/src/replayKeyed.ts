/**
 * Replay a few messages of a topic through the keyed-record handlers of the feeder, without moving any consumer's
 * offsets. For the case where a keyed consumer's intents were dead-lettered (errors.jsonl) while serve was
 * unreachable: the consumer's own offset file says those offsets are done, so a consumer-group rewind alone would
 * change nothing (keyedBatchHandler skips every offset at or below the file's last done offset).
 *
 * Safe to rerun: every keyed record is written to ids fixed by its key (keyedRecord.ts), so a replay of a message
 * that already landed re-adds the same ids. A handler ignores events that are not its own, so a range may be fed to
 * every handler that reads the topic. See replay-keyed-main.ts and geodb/RUNBOOK.md section 2b-quater-replay.
 */
import type {KeyedHandle} from "./keyedConsumer.js"
import type {KeyedRecordDeps} from "./keyedRecord.js"

export type ReplayMessage = {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>}
export type ReplayHandler<D extends KeyedRecordDeps> = {name: string; handle: KeyedHandle<D>; deps: D}
export type ReplayLine = {offset: string; handler: string; outcome: string}

const headerText = (h: unknown): string => (Buffer.isBuffer(h) || h instanceof Uint8Array ? Buffer.from(h).toString("utf8") : String(h ?? ""))

/** What a dry run prints for a message: its offset, event type and size. Writes nothing. */
export const describeMessage = (m: ReplayMessage) => ({
	offset: m.offset,
	eventType: headerText(m.headers?.["event-type"]) || null,
	bytes: m.value?.length ?? 0,
})

/** `"172829-172833"` or `"98657"` as an inclusive range; anything else (or more than `max` offsets) throws. */
export function parseRange(text: string, max = 100): {from: number; to: number} {
	const m = /^(\d+)(?:-(\d+))?$/.exec(text)
	if (!m) throw new Error(`--offsets must be N or N-M, not ${text.slice(0, 40)}`)
	const from = Number(m[1])
	const to = m[2] === undefined ? from : Number(m[2])
	if (to < from) throw new Error(`--offsets ${text}: the end is before the start`)
	if (to - from + 1 > max) throw new Error(`--offsets ${text}: ${to - from + 1} offsets; this mode is for a handful (at most ${max})`)
	return {from, to}
}

/**
 * Feed every message to every handler, in offset order, and say what each did. A handler that throws stops the
 * replay (the caller reports it; a rerun is safe). `dead_letter` outcomes are returned, not thrown.
 */
export async function replayKeyedMessages<D extends KeyedRecordDeps>(
	partition: number,
	messages: readonly ReplayMessage[],
	handlers: readonly ReplayHandler<D>[],
): Promise<ReplayLine[]> {
	const out: ReplayLine[] = []
	for (const m of [...messages].sort((a, b) => Number(a.offset) - Number(b.offset))) {
		for (const h of handlers) {
			const outcome = await h.handle(h.deps, partition, m)
			out.push({offset: m.offset, handler: h.name, outcome})
		}
	}
	return out
}
