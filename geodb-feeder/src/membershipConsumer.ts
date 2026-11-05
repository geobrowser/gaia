/**
 * The membership consumer and the shadow comparison loop, started by main.ts behind their flags. Neither
 * touches Postgres writes: the consumer is a second group on `space.membership` (the kg-indexer's group is
 * untouched), the comparison only reads. A failure here is logged and counted; it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {errorFields, log} from "./log.js"
import {observeFreshness} from "./metrics.js"
import {handleMembershipMessage, type MembershipDeps} from "./membership.js"
import {handleKeyedMembershipMessage, type KeyedMembershipDeps} from "./membershipKeyed.js"
import {createGroupCommitter} from "./groupCommit.js"

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

/**
 * `deps`: the legacy journal-planned dual write (membership.ts), group KAFKA_MEMBERSHIP_GROUP_ID. `keyed`: the
 * natural-key records (membershipKeyed.ts, GEODB_RECORD_KEYS), on a group of its own so it replays the topic from
 * the start into the governance system Space without moving the legacy group's offsets. main.ts starts one or the
 * other, never both.
 */
export async function startMembershipConsumer(
	opts: {
		kafka: InstanceType<typeof kafkajs.Kafka>
		config: Config
		abort: AbortController
	} & ({deps: MembershipDeps; keyed?: undefined} | {keyed: KeyedMembershipDeps; deps?: undefined}),
): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {kafka, config, abort} = opts
	const deps = opts.keyed ?? opts.deps
	const offsets = opts.keyed ? opts.keyed.offsets : opts.deps.state
	const handle = (partition: number, m: kafkajs.KafkaMessage, beat: () => Promise<void>) =>
		opts.keyed
			? handleKeyedMembershipMessage({...opts.keyed, beat, signal: abort.signal}, partition, m)
			: handleMembershipMessage({...opts.deps, beat, signal: abort.signal}, partition, m)
	const {topic} = config.membership
	const groupId = opts.keyed ? config.membership.keyedGroupId : config.membership.groupId
	const admin = kafka.admin()
	await admin.connect()
	if (!offsets.hadOffsets) {
		try {
			await admin.resetOffsets({groupId, topic, earliest: true})
		} catch (e) {
			log("warn", "membership: could not reset group offsets", errorFields(e))
		}
	}
	const consumer = kafka.consumer({groupId, sessionTimeout: 120_000, heartbeatInterval: 5000})
	await consumer.connect()
	await consumer.subscribe({topic, fromBeginning: true})
	consumer.on(consumer.events.CRASH, (e) => {
		deps.metrics.membershipUp.set({}, 0)
		log("error", "membership consumer crashed (edits consumer unaffected)", {error: String(e.payload?.error).slice(0, 300)})
	})
	const commits = createGroupCommitter((partition, offset) =>
		consumer.commitOffsets([{topic, partition, offset}]).catch((e) => log("warn", "membership group commit failed", errorFields(e))),
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
						outcome = await handle(batch.partition, m, heartbeat)
						break
					} catch (e) {
						if (abort.signal.aborted) return
						log("error", "membership message failed, retrying (never skipped)", {partition: batch.partition, offset, ...errorFields(e)})
						await sleep(5000)
						await heartbeat()
					}
				}
				offsets.markDone(batch.partition, offset)
				deps.metrics.membershipEvents.inc({outcome})
				observeFreshness(deps.metrics.membershipFreshness, m.timestamp)
				resolveOffset(m.offset)
				await heartbeat()
				await commits.done(batch.partition, offset)
			}
			await commits.flush()
		},
	})
	deps.metrics.membershipUp.set({}, 1)
	log("info", "membership consumer running", {
		topic,
		groupId,
		...(opts.keyed ? {keyed: true} : {statements: opts.deps.state.statementCount}),
	})
	const poll = async () => {
		try {
			for (const p of await admin.fetchTopicOffsets(topic)) {
				const last = offsets.lastDone(p.partition)
				const next = last === null ? Number(p.low) : last + 1
				deps.metrics.membershipLag.set({partition: String(p.partition)}, Math.max(0, Number(p.high) - next))
			}
		} catch (e) {
			log("warn", "membership lag poll failed", errorFields(e))
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
