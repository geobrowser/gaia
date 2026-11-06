/**
 * The keyed space-topics consumer, started by main.ts when spaceTopicsKeyedEnabled (GEODB_SPACE_TOPICS with
 * GEODB_RECORD_KEYS): its own group on `space.topics` (kg-indexer's group is untouched). It writes TOPIC_DECLARED and
 * TOPIC_REMOVED events (the rest count as ignored). The loop is keyedConsumer.ts: a message that fails is retried in
 * place, never skipped. A failure to start is logged and counted (startOptionalConsumer, GeoDbFeederConsumerStartFailed);
 * it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"
import {handleSpaceTopicMessage, type KeyedSpaceTopicsDeps} from "./spaceTopicsKeyed.js"

const topicsStream = (metrics: Metrics, handle: KeyedStream<KeyedSpaceTopicsDeps>["handle"] = handleSpaceTopicMessage): KeyedStream<KeyedSpaceTopicsDeps> => ({
	name: "space topics",
	handle,
	events: metrics.spaceTopicsEvents,
	freshness: metrics.spaceTopicsFreshness,
	lag: metrics.spaceTopicsLag,
	up: metrics.spaceTopicsUp,
})

/** The consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function spaceTopicsBatchHandler(opts: {
	deps: KeyedSpaceTopicsDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleSpaceTopicMessage
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: topicsStream(opts.deps.metrics, opts.handle)})
}

export function startSpaceTopicsConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedSpaceTopicsDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.spaceTopics
	return startKeyedConsumer({...opts, topic, groupId, stream: topicsStream(opts.deps.metrics)})
}
