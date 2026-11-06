/**
 * The keyed space-addresses consumer, started by main.ts when spaceAddressesKeyedEnabled (GEODB_SPACE_ADDRESSES with
 * GEODB_RECORD_KEYS): its own group on `space.creations` (kg-indexer's group is untouched). It writes SPACE_ADDRESS_UPDATED and
 * HermesCreateSpace messages (the rest count as ignored). The loop is keyedConsumer.ts: a message that fails is retried in
 * place, never skipped. A failure to start is logged and counted (startOptionalConsumer, GeoDbFeederConsumerStartFailed);
 * it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"
import {handleSpaceAddressMessage, type KeyedSpaceAddressesDeps} from "./spaceAddressesKeyed.js"

const addressesStream = (metrics: Metrics, handle: KeyedStream<KeyedSpaceAddressesDeps>["handle"] = handleSpaceAddressMessage): KeyedStream<KeyedSpaceAddressesDeps> => ({
	name: "space addresses",
	handle,
	events: metrics.spaceAddressesEvents,
	freshness: metrics.spaceAddressesFreshness,
	lag: metrics.spaceAddressesLag,
	up: metrics.spaceAddressesUp,
})

/** The consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function spaceAddressesBatchHandler(opts: {
	deps: KeyedSpaceAddressesDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleSpaceAddressMessage
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: addressesStream(opts.deps.metrics, opts.handle)})
}

export function startSpaceAddressesConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedSpaceAddressesDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.spaceAddresses
	return startKeyedConsumer({...opts, topic, groupId, stream: addressesStream(opts.deps.metrics)})
}
