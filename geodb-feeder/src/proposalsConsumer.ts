/**
 * The proposal system-entity consumer, started by main.ts behind GEODB_PROPOSAL_ENTITIES: its own group on
 * `space.governance` (the kg-indexer's group is untouched). A failure here is logged and counted; it never
 * stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {errorFields, log} from "./log.js"
import {observeFreshness} from "./metrics.js"
import {handleProposalMessage, ProposalOffsets, type ProposalDeps} from "./proposals.js"

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

export async function startProposalConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: ProposalDeps
	offsets: ProposalOffsets
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {kafka, config, deps, offsets, abort} = opts
	const {topic, groupId} = config.proposals
	const admin = kafka.admin()
	await admin.connect()
	if (!offsets.hadOffsets) {
		try {
			await admin.resetOffsets({groupId, topic, earliest: true})
		} catch (e) {
			log("warn", "proposals: could not reset group offsets", errorFields(e))
		}
	}
	const consumer = kafka.consumer({groupId, sessionTimeout: 120_000, heartbeatInterval: 5000})
	await consumer.connect()
	await consumer.subscribe({topic, fromBeginning: true})
	consumer.on(consumer.events.CRASH, (e) => {
		deps.metrics.proposalsUp.set({}, 0)
		log("error", "proposals consumer crashed (edits consumer unaffected)", {error: String(e.payload?.error).slice(0, 300)})
	})
	let lastCommit = 0
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
					continue
				}
				let outcome: string
				for (;;) {
					try {
						outcome = await handleProposalMessage({...deps, beat: heartbeat, signal: abort.signal}, batch.partition, m)
						break
					} catch (e) {
						if (abort.signal.aborted) return
						log("error", "proposal message failed, retrying (never skipped)", {partition: batch.partition, offset, ...errorFields(e)})
						await sleep(5000)
						await heartbeat()
					}
				}
				offsets.markDone(batch.partition, offset)
				deps.metrics.proposalEvents.inc({outcome})
				observeFreshness(deps.metrics.proposalFreshness, m.timestamp)
				resolveOffset(m.offset)
				await heartbeat()
				if (Date.now() - lastCommit > 5000) {
					lastCommit = Date.now()
					await consumer
						.commitOffsets([{topic, partition: batch.partition, offset: String(offset + 1)}])
						.catch((e) => log("warn", "proposals group commit failed", errorFields(e)))
				}
			}
		},
	})
	deps.metrics.proposalsUp.set({}, 1)
	log("info", "proposals consumer running", {topic, groupId})
	const poll = async () => {
		try {
			for (const p of await admin.fetchTopicOffsets(topic)) {
				const last = offsets.lastDone(p.partition)
				const next = last === null ? Number(p.low) : last + 1
				deps.metrics.proposalsLag.set({partition: String(p.partition)}, Math.max(0, Number(p.high) - next))
			}
		} catch (e) {
			log("warn", "proposals lag poll failed", errorFields(e))
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
