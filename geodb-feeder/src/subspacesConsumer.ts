/**
 * The keyed subspaces consumer, started by main.ts when subspacesKeyedEnabled (GEODB_SUBSPACES with GEODB_RECORD_KEYS):
 * its own group on `space.trust.extensions` (kg-indexer's group is untouched). It writes only TRUST_EXTENSION events (the
 * rest count as ignored). The loop is keyedConsumer.ts: a message that fails is retried in place, never skipped. A
 * failure to start is logged and counted (startOptionalConsumer, GeoDbFeederConsumerStartFailed); it never stops the
 * edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"
import {handleSubspaceMessage, type KeyedSubspacesDeps} from "./subspacesKeyed.js"

const subspacesStream = (
	metrics: Metrics,
	handle: KeyedStream<KeyedSubspacesDeps>["handle"] = handleSubspaceMessage,
): KeyedStream<KeyedSubspacesDeps> => ({
	name: "subspaces",
	handle,
	events: metrics.subspaceEvents,
	freshness: metrics.subspaceFreshness,
	lag: metrics.subspacesLag,
	up: metrics.subspacesUp,
})

/** The subspaces consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function subspacesBatchHandler(opts: {
	deps: KeyedSubspacesDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleSubspaceMessage
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: subspacesStream(opts.deps.metrics, opts.handle)})
}

export function startSubspacesConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedSubspacesDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.subspaces
	return startKeyedConsumer({...opts, topic, groupId, stream: subspacesStream(opts.deps.metrics)})
}
