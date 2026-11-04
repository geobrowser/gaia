/**
 * The Space system-entity consumer, started by main.ts behind GEODB_SPACE_ENTITIES: its own group on
 * `space.creations` (the kg-indexer's group is untouched). It reuses ProposalOffsets for its offsets file. A failure here is logged and counted; it never
 * stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {errorFields, log} from "./log.js"
import {observeFreshness} from "./metrics.js"
import {createLock} from "./apply.js"
import type {ProposalOffsets} from "./proposals.js"
import {handleSpaceCreationMessage, sweepPending, type SpaceEntityDeps} from "./spaceEntities.js"
import {createGroupCommitter} from "./groupCommit.js"

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

export async function startSpaceEntityConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: SpaceEntityDeps
	offsets: ProposalOffsets
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {kafka, config, deps, offsets, abort} = opts
	const {topic, groupId} = config.spaceEntities
	// The batch handler and the pending sweep both write; one at a time keeps the journals consistent.
	const serial = createLock()
	const admin = kafka.admin()
	await admin.connect()
	if (!offsets.hadOffsets) {
		try {
			await admin.resetOffsets({groupId, topic, earliest: true})
		} catch (e) {
			log("warn", "space entities: could not reset group offsets", errorFields(e))
		}
	}
	const consumer = kafka.consumer({groupId, sessionTimeout: 120_000, heartbeatInterval: 5000})
	await consumer.connect()
	await consumer.subscribe({topic, fromBeginning: true})
	consumer.on(consumer.events.CRASH, (e) => {
		deps.metrics.spaceEntitiesUp.set({}, 0)
		log("error", "space entities consumer crashed (edits consumer unaffected)", {error: String(e.payload?.error).slice(0, 300)})
	})
	const commits = createGroupCommitter((partition, offset) =>
		consumer.commitOffsets([{topic, partition, offset}]).catch((e) => log("warn", "space entities group commit failed", errorFields(e))),
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
						outcome = await serial(() => handleSpaceCreationMessage({...deps, beat: heartbeat, signal: abort.signal}, batch.partition, m))
						break
					} catch (e) {
						if (abort.signal.aborted) return
						log("error", "space creation message failed, retrying (never skipped)", {partition: batch.partition, offset, ...errorFields(e)})
						await sleep(5000)
						await heartbeat()
					}
				}
				offsets.markDone(batch.partition, offset)
				deps.metrics.spaceEntityEvents.inc({outcome})
				observeFreshness(deps.metrics.spaceEntityFreshness, m.timestamp)
				resolveOffset(m.offset)
				await heartbeat()
				await commits.done(batch.partition, offset)
			}
			await commits.flush()
		},
	})
	deps.metrics.spaceEntitiesUp.set({}, 1)
	log("info", "space entities consumer running", {topic, groupId})
	const poll = async () => {
		try {
			// Entities parked because their Space was not founded yet: apply those founded since.
			await serial(() => sweepPending({...deps, signal: abort.signal}))
			deps.metrics.spaceEntitiesPending.set({}, deps.pending.size)
		} catch (e) {
			log("warn", "space entities pending sweep failed", errorFields(e))
		}
		try {
			for (const p of await admin.fetchTopicOffsets(topic)) {
				const last = offsets.lastDone(p.partition)
				const next = last === null ? Number(p.low) : last + 1
				deps.metrics.spaceEntitiesLag.set({partition: String(p.partition)}, Math.max(0, Number(p.high) - next))
			}
		} catch (e) {
			log("warn", "space entities lag poll failed", errorFields(e))
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
