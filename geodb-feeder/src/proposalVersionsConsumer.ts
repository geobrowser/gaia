/**
 * The keyed proposal-versions consumer, started by main.ts when proposalVersionsKeyedEnabled (GEODB_PROPOSAL_VERSIONS with
 * GEODB_RECORD_KEYS): its own group on `space.governance` (kg-indexer's group and the proposals consumer's are
 * untouched). It reads every message on the topic and writes only PROPOSAL_CREATED, PROPOSAL_UPDATED or PROPOSAL_SETTINGS_UPDATED (the rest count as ignored). The
 * loop is keyedConsumer.ts: a message that fails is retried in place, never skipped. A failure to start is logged and
 * counted (startOptionalConsumer, GeoDbFeederConsumerStartFailed); it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"
import {handleProposalVersionMessage, type KeyedProposalVersionsDeps} from "./proposalVersionsKeyed.js"

const proposalVersionsStream = (
	metrics: Metrics,
	handle: KeyedStream<KeyedProposalVersionsDeps>["handle"] = handleProposalVersionMessage,
): KeyedStream<KeyedProposalVersionsDeps> => ({
	name: "proposal versions",
	handle,
	events: metrics.proposalVersionEvents,
	freshness: metrics.proposalVersionFreshness,
	lag: metrics.proposalVersionsLag,
	up: metrics.proposalVersionsUp,
})

/** The proposal-versions consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function proposalVersionsBatchHandler(opts: {
	deps: KeyedProposalVersionsDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleProposalVersionMessage
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: proposalVersionsStream(opts.deps.metrics, opts.handle)})
}

export function startProposalVersionsConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedProposalVersionsDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.proposalVersions
	return startKeyedConsumer({...opts, topic, groupId, stream: proposalVersionsStream(opts.deps.metrics)})
}
