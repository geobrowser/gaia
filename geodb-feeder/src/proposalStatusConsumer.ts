/**
 * The keyed proposal-status consumer, started by main.ts when proposalStatusKeyedEnabled (GEODB_PROPOSAL_STATUS with
 * GEODB_RECORD_KEYS): its own group on `space.governance` (kg-indexer's group and the other governance consumers' are
 * untouched). It reads every message on the topic and writes only PROPOSAL_EXECUTED (the rest count as ignored). The
 * loop is keyedConsumer.ts: a message that fails is retried in place, never skipped. A failure to start is logged and
 * counted (startOptionalConsumer, GeoDbFeederConsumerStartFailed); it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"
import {handleProposalStatusMessage, type KeyedProposalStatusDeps} from "./proposalStatusKeyed.js"

const statusStream = (
	metrics: Metrics,
	handle: KeyedStream<KeyedProposalStatusDeps>["handle"] = handleProposalStatusMessage,
): KeyedStream<KeyedProposalStatusDeps> => ({
	name: "proposal status",
	handle,
	events: metrics.proposalStatusEvents,
	freshness: metrics.proposalStatusFreshness,
	lag: metrics.proposalStatusLag,
	up: metrics.proposalStatusUp,
})

/** The consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function proposalStatusBatchHandler(opts: {
	deps: KeyedProposalStatusDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleProposalStatusMessage
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: statusStream(opts.deps.metrics, opts.handle)})
}

export function startProposalStatusConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedProposalStatusDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.proposalStatus
	return startKeyedConsumer({...opts, topic, groupId, stream: statusStream(opts.deps.metrics)})
}
