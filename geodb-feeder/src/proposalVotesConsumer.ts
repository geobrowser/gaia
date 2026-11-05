/**
 * The keyed proposal-votes consumer, started by main.ts when proposalVotesKeyedEnabled (GEODB_PROPOSAL_VOTES with
 * GEODB_RECORD_KEYS): its own group on `space.governance` (kg-indexer's group and the proposals consumer's are
 * untouched). It reads every message on the topic and writes only PROPOSAL_VOTED (the rest count as ignored). The
 * loop is keyedConsumer.ts: a message that fails is retried in place, never skipped. A failure to start is logged and
 * counted (startOptionalConsumer, GeoDbFeederConsumerStartFailed); it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"
import {handleProposalVoteMessage, type KeyedProposalVotesDeps} from "./proposalVotesKeyed.js"

const proposalVotesStream = (
	metrics: Metrics,
	handle: KeyedStream<KeyedProposalVotesDeps>["handle"] = handleProposalVoteMessage,
): KeyedStream<KeyedProposalVotesDeps> => ({
	name: "proposal votes",
	handle,
	events: metrics.proposalVoteEvents,
	freshness: metrics.proposalVoteFreshness,
	lag: metrics.proposalVotesLag,
	up: metrics.proposalVotesUp,
})

/** The proposal-votes consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function proposalVotesBatchHandler(opts: {
	deps: KeyedProposalVotesDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleProposalVoteMessage
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: proposalVotesStream(opts.deps.metrics, opts.handle)})
}

export function startProposalVotesConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedProposalVotesDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.proposalVotes
	return startKeyedConsumer({...opts, topic, groupId, stream: proposalVotesStream(opts.deps.metrics)})
}
