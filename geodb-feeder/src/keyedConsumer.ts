/**
 * The consumer loop every keyed-record family shares (votesConsumer.ts, proposalVotesConsumer.ts): its own group on
 * its topic, reset to earliest the first time it runs (no offsets yet), one partition at a time. A message that fails
 * is retried in place, never skipped: a skipped event would leave its key at an older value until the key's next
 * event. A failure to start is the caller's to report (startOptionalConsumer).
 */
import type kafkajs from "kafkajs"
import {createGroupCommitter} from "./groupCommit.js"
import type {KeyedRecordDeps} from "./keyedRecord.js"
import {errorFields, log} from "./log.js"
import {type Histogram, observeFreshness, type Series} from "./metrics.js"

const realSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

type BatchMessage = {offset: string; value: Buffer | null; timestamp: string; headers?: Record<string, unknown>}
export type KeyedBatch = {
	batch: {partition: number; messages: BatchMessage[]}
	resolveOffset: (offset: string) => void
	heartbeat: () => Promise<void>
}

export type KeyedHandle<D extends KeyedRecordDeps> = (
	deps: D,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
) => Promise<string>

/** One family's stream: what it is called in logs, how a message is handled, and its metric series. */
export type KeyedStream<D extends KeyedRecordDeps> = {
	name: string
	handle: KeyedHandle<D>
	events: Series
	freshness: Histogram
	lag: Series
	up: Series
}

/**
 * The eachBatch body. A message is applied, retried in place on any throw (every `retryMs`, heartbeating) until it
 * succeeds or the feeder shuts down, and only then marked done and resolved: the partition stalls, it never skips.
 * Offsets already marked done (a redelivery after a crash) are resolved unapplied.
 */
export function keyedBatchHandler<D extends KeyedRecordDeps>(opts: {
	deps: D
	stream: Pick<KeyedStream<D>, "name" | "handle" | "events" | "freshness">
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	const {deps, stream, abort, commits} = opts
	const {offsets} = deps
	const retryMs = opts.retryMs ?? 5000
	return async ({batch, resolveOffset, heartbeat}) => {
		for (const m of batch.messages) {
			if (abort.signal.aborted) return
			const offset = Number(m.offset)
			const last = offsets.lastDone(batch.partition)
			if (last !== null && offset <= last) {
				resolveOffset(m.offset)
				await commits.done(batch.partition, offset)
				continue
			}
			let outcome: string
			for (;;) {
				try {
					outcome = await stream.handle({...deps, beat: heartbeat, signal: abort.signal}, batch.partition, m)
					break
				} catch (e) {
					if (abort.signal.aborted) return
					log("error", `${stream.name} message failed, retrying (never skipped)`, {partition: batch.partition, offset, ...errorFields(e)})
					await opts.sleep(retryMs)
					await heartbeat()
				}
			}
			offsets.markDone(batch.partition, offset)
			stream.events.inc({outcome})
			observeFreshness(stream.freshness, m.timestamp)
			resolveOffset(m.offset)
			await heartbeat()
			await commits.done(batch.partition, offset)
		}
		await commits.flush()
	}
}

export async function startKeyedConsumer<D extends KeyedRecordDeps>(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	topic: string
	groupId: string
	stream: KeyedStream<D>
	deps: D
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {kafka, topic, groupId, stream, deps, abort} = opts
	const {offsets} = deps
	const admin = kafka.admin()
	await admin.connect()
	if (!offsets.hadOffsets) {
		try {
			await admin.resetOffsets({groupId, topic, earliest: true})
		} catch (e) {
			log("warn", `${stream.name}: could not reset group offsets`, errorFields(e))
		}
	}
	const consumer = kafka.consumer({groupId, sessionTimeout: 120_000, heartbeatInterval: 5000})
	await consumer.connect()
	await consumer.subscribe({topic, fromBeginning: true})
	consumer.on(consumer.events.CRASH, (e) => {
		stream.up.set({}, 0)
		log("error", `${stream.name} consumer crashed (edits consumer unaffected)`, {error: String(e.payload?.error).slice(0, 300)})
	})
	const commits = createGroupCommitter((partition, offset) =>
		consumer.commitOffsets([{topic, partition, offset}]).catch((e) => log("warn", `${stream.name} group commit failed`, errorFields(e))),
	)
	await consumer.run({
		autoCommit: false,
		partitionsConsumedConcurrently: 1,
		eachBatch: keyedBatchHandler({deps, stream, abort, commits, sleep: realSleep}),
	})
	stream.up.set({}, 1)
	log("info", `${stream.name} consumer running`, {topic, groupId})
	const poll = async () => {
		try {
			for (const p of await admin.fetchTopicOffsets(topic)) {
				const last = offsets.lastDone(p.partition)
				const next = last === null ? Number(p.low) : last + 1
				stream.lag.set({partition: String(p.partition)}, Math.max(0, Number(p.high) - next))
			}
		} catch (e) {
			log("warn", `${stream.name} lag poll failed`, errorFields(e))
		}
	}
	return {
		poll,
		stop: async () => {
			await consumer.disconnect().catch(() => {})
			await admin.disconnect().catch(() => {})
		},
	}
}
