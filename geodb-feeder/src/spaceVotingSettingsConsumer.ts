/**
 * The keyed space-voting-settings consumer, started by main.ts when spaceVotingSettingsKeyedEnabled
 * (GEODB_SPACE_VOTING_SETTINGS with GEODB_RECORD_KEYS): its own group on `space.governance` (kg-indexer's group and the
 * other governance consumers' are untouched). It reads every message on the topic and writes only
 * VOTING_SETTINGS_UPDATED (the rest count as ignored). The loop is keyedConsumer.ts: a message that fails is retried in
 * place, never skipped. A failure to start is logged and counted (startOptionalConsumer, GeoDbFeederConsumerStartFailed);
 * it never stops the edits consumer.
 */
import type kafkajs from "kafkajs"
import type {Config} from "./config.js"
import {type KeyedBatch, type KeyedStream, keyedBatchHandler, startKeyedConsumer} from "./keyedConsumer.js"
import type {Metrics} from "./metrics.js"
import {handleSpaceVotingSettingsMessage, type KeyedSpaceVotingSettingsDeps} from "./spaceVotingSettingsKeyed.js"

const settingsStream = (
	metrics: Metrics,
	handle: KeyedStream<KeyedSpaceVotingSettingsDeps>["handle"] = handleSpaceVotingSettingsMessage,
): KeyedStream<KeyedSpaceVotingSettingsDeps> => ({
	name: "space voting settings",
	handle,
	events: metrics.spaceVotingSettingsEvents,
	freshness: metrics.spaceVotingSettingsFreshness,
	lag: metrics.spaceVotingSettingsLag,
	up: metrics.spaceVotingSettingsUp,
})

/** The consumer's eachBatch body (keyedBatchHandler), exported for its test. */
export function spaceVotingSettingsBatchHandler(opts: {
	deps: KeyedSpaceVotingSettingsDeps
	abort: AbortController
	commits: {done: (partition: number, offset: number) => Promise<void>; flush: () => Promise<void>}
	sleep: (ms: number) => Promise<void>
	handle?: typeof handleSpaceVotingSettingsMessage
	retryMs?: number
}): (b: KeyedBatch) => Promise<void> {
	return keyedBatchHandler({...opts, stream: settingsStream(opts.deps.metrics, opts.handle)})
}

export function startSpaceVotingSettingsConsumer(opts: {
	kafka: InstanceType<typeof kafkajs.Kafka>
	config: Config
	deps: KeyedSpaceVotingSettingsDeps
	abort: AbortController
}): Promise<{stop: () => Promise<void>; poll: () => Promise<void>}> {
	const {topic, groupId} = opts.config.spaceVotingSettings
	return startKeyedConsumer({...opts, topic, groupId, stream: settingsStream(opts.deps.metrics)})
}
