/**
 * The keyed proposal-actions consumer, started by main.ts when proposalActionsKeyedEnabled (GEODB_PROPOSAL_ACTIONS with
 * GEODB_RECORD_KEYS): its own group on `space.governance` (kg-indexer's group and the proposals consumer's are
 * untouched). It reads every message on the topic and writes only PROPOSAL_CREATED or PROPOSAL_UPDATED (the rest count as ignored). The
 * loop is keyedConsumer.ts: a message that fails is retried in place, never skipped. A failure to start is logged and
 * counted (startOptionalConsumer, GeoDbFeederConsumerStartFailed); it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"
import {handleProposalActionMessage, type KeyedProposalActionsDeps} from "./proposalActionsKeyed.js"

const proposalActionsStream = (
	metrics: Metrics,
	handle: KeyedStream<KeyedProposalActionsDeps>["handle"] = handleProposalActionMessage,
): KeyedStream<KeyedProposalActionsDeps> => ({
	name: "proposal actions",
	handle,
	events: metrics.proposalActionEvents,
	freshness: metrics.proposalActionFreshness,
	lag: metrics.proposalActionsLag,
	up: metrics.proposalActionsUp,
})

/** The proposal-actions consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function proposalActionsBatchHandler(opts: {
	deps: KeyedProposalActionsDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleProposalActionMessage
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: proposalActionsStream(opts.deps.metrics, opts.handle)})
}

export function startProposalActionsConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedProposalActionsDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.proposalActions
	return startKeyedConsumer({...opts, topic, groupId, stream: proposalActionsStream(opts.deps.metrics)})
}
