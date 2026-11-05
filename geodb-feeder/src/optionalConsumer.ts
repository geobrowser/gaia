/**
 * Start a consumer that must never take the edits consumer down with it. A failure to start (a missing Kafka ACL on
 * its topic or group is the expected one) is logged and sets `geodb_feeder_consumer_start_failed{consumer}` to 1, which
 * GeoDbFeederConsumerStartFailed alerts on: without it the failed consumer has no lag series, so its stall alert can
 * never fire and the only trace would be one log line. A successful start sets it to 0.
 */
import {errorFields, log} from "./log.js"
import type {Series} from "./metrics.js"

export async function startOptionalConsumer<T>(
	name: string,
	startFailed: Series,
	start: () => Promise<T>,
): Promise<T | null> {
	try {
		const c = await start()
		startFailed.set({consumer: name}, 0)
		return c
	} catch (e) {
		startFailed.set({consumer: name}, 1)
		log("error", `${name} consumer failed to start (edits consumer unaffected)`, {consumer: name, ...errorFields(e)})
		return null
	}
}
