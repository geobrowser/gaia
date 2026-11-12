/**
 * The keyed edit-meta consumer, started by main.ts when editMetaKeyedEnabled (GEODB_EDIT_META with GEODB_RECORD_KEYS):
 * its own group on `knowledge.edits` (the edits consumer's group is untouched). It reads each edit's envelope only
 * (editMetaKeyed.ts). The loop is keyedConsumer.ts: a message that fails is retried in place, never skipped. A failure
 * to start is logged and counted (startOptionalConsumer, GeoDbFeederConsumerStartFailed); it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {handleEditMetaMessage, type KeyedEditMetaDeps} from "./editMetaKeyed.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"

const editMetaStream = (
	metrics: Metrics,
	handle: KeyedStream<KeyedEditMetaDeps>["handle"] = handleEditMetaMessage,
): KeyedStream<KeyedEditMetaDeps> => ({
	name: "edit_meta",
	handle,
	events: metrics.editMetaEvents,
	freshness: metrics.editMetaFreshness,
	lag: metrics.editMetaLag,
	up: metrics.editMetaUp,
})

/** The consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function editMetaBatchHandler(opts: {
	deps: KeyedEditMetaDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleEditMetaMessage
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: editMetaStream(opts.deps.metrics, opts.handle)})
}

export function startEditMetaConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedEditMetaDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.editMeta
	return startKeyedConsumer({...opts, topic, groupId, stream: editMetaStream(opts.deps.metrics)})
}
