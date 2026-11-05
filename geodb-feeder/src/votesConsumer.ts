/**
 * The keyed user-votes consumer, started by main.ts when votesKeyedEnabled (GEODB_VOTES with GEODB_RECORD_KEYS): its
 * own group on `curation.votes` (vote-indexer's group is untouched). A message that fails is retried in place, never
 * skipped (a skipped vote would leave its key at an older value until the key's next event). A failure to start is
 * logged; it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {errorFields, log} from "./log.js"
import {observeFreshness} from "./metrics.js"
import {createGroupCommitter} from "./groupCommit.js"
import {handleVoteMessage, type KeyedVotesDeps} from "./votesKeyed.js"

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

export async function startVotesConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedVotesDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {kafka, config, deps, abort} = opts
	const {offsets} = deps
	const {topic, groupId} = config.votes
	const admin = kafka.admin()
	await admin.connect()
	if (!offsets.hadOffsets) {
		try {
			await admin.resetOffsets({groupId, topic, earliest: true})
		} catch (e) {
			log("warn", "votes: could not reset group offsets", errorFields(e))
		}
	}
	const consumer = kafka.consumer({groupId, sessionTimeout: 120_000, heartbeatInterval: 5000})
	await consumer.connect()
	await consumer.subscribe({topic, fromBeginning: true})
	consumer.on(consumer.events.CRASH, (e) => {
		deps.metrics.votesUp.set({}, 0)
		log("error", "votes consumer crashed (edits consumer unaffected)", {error: String(e.payload?.error).slice(0, 300)})
	})
	const commits = createGroupCommitter((partition, offset) =>
		consumer.commitOffsets([{topic, partition, offset}]).catch((e) => log("warn", "votes group commit failed", errorFields(e))),
	)
	await consumer.run({
		autoCommit: false,
		partitionsConsumedConcurrently: 1,
		eachBatch: async ({batch, resolveOffset, heartbeat}) => {
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
						outcome = await handleVoteMessage({...deps, beat: heartbeat, signal: abort.signal}, batch.partition, m)
						break
					} catch (e) {
						if (abort.signal.aborted) return
						log("error", "vote message failed, retrying (never skipped)", {partition: batch.partition, offset, ...errorFields(e)})
						await sleep(5000)
						await heartbeat()
					}
				}
				offsets.markDone(batch.partition, offset)
				deps.metrics.voteEvents.inc({outcome})
				observeFreshness(deps.metrics.voteFreshness, m.timestamp)
				resolveOffset(m.offset)
				await heartbeat()
				await commits.done(batch.partition, offset)
			}
			await commits.flush()
		},
	})
	deps.metrics.votesUp.set({}, 1)
	log("info", "votes consumer running", {topic, groupId})
	const poll = async () => {
		try {
			for (const p of await admin.fetchTopicOffsets(topic)) {
				const last = offsets.lastDone(p.partition)
				const next = last === null ? Number(p.low) : last + 1
				deps.metrics.votesLag.set({partition: String(p.partition)}, Math.max(0, Number(p.high) - next))
			}
		} catch (e) {
			log("warn", "votes lag poll failed", errorFields(e))
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
