/**
 * The keyed user-votes consumer, started by main.ts when votesKeyedEnabled (GEODB_VOTES with GEODB_RECORD_KEYS): its
 * own group on `curation.votes` (vote-indexer's group is untouched). The loop is keyedConsumer.ts: a message that
 * fails is retried in place, never skipped. A failure to start is logged and counted (startOptionalConsumer,
 * GeoDbFeederConsumerStartFailed); it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"
import {handleVoteMessage, type KeyedVotesDeps} from "./votesKeyed.js"

export type VotesBatch = KeyedBatch

const votesStream = (metrics: Metrics, handle: KeyedStream<KeyedVotesDeps>["handle"] = handleVoteMessage): KeyedStream<KeyedVotesDeps> => ({
	name: "votes",
	handle,
	events: metrics.voteEvents,
	freshness: metrics.voteFreshness,
	lag: metrics.votesLag,
	up: metrics.votesUp,
})

/** The votes consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function votesBatchHandler(opts: {
	deps: KeyedVotesDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleVoteMessage
	retryMs?: number
}): (b: VotesBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: votesStream(opts.deps.metrics, opts.handle)})
}

export function startVotesConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedVotesDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.votes
	return startKeyedConsumer({...opts, topic, groupId, stream: votesStream(opts.deps.metrics)})
}
