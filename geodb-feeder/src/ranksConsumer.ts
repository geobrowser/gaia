/**
 * The keyed ranks consumer, started by main.ts when ranksKeyedEnabled (GEODB_RANKS with
 * GEODB_RECORD_KEYS): its own group on `knowledge.edits` (the edits consumer's group and ranking-indexer's are
 * untouched). It decodes every edit and writes only what ranking-indexer folds into the ranks tables (ranksKeyed.ts; an
 * edit with nothing rank-relevant counts as ignored). The
 * loop is keyedConsumer.ts: a message that fails is retried in place, never skipped. A failure to start is logged and
 * counted (startOptionalConsumer, GeoDbFeederConsumerStartFailed); it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"
import {handleRanksMessage, type KeyedRanksDeps} from "./ranksKeyed.js"

const ranksStream = (
	metrics: Metrics,
	handle: KeyedStream<KeyedRanksDeps>["handle"] = handleRanksMessage,
): KeyedStream<KeyedRanksDeps> => ({
	name: "ranks",
	handle,
	events: metrics.ranksEvents,
	freshness: metrics.ranksFreshness,
	lag: metrics.ranksLag,
	up: metrics.ranksUp,
})

/** The consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function ranksBatchHandler(opts: {
	deps: KeyedRanksDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleRanksMessage
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: ranksStream(opts.deps.metrics, opts.handle)})
}

export function startRanksConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedRanksDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.ranks
	return startKeyedConsumer({...opts, topic, groupId, stream: ranksStream(opts.deps.metrics)})
}
