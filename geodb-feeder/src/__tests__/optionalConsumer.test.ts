import {describe, expect, it} from "vitest"
import {createMetrics} from "../metrics.js"
import {startOptionalConsumer} from "../optionalConsumer.js"

describe("startOptionalConsumer", () => {
	it("returns null and sets consumer_start_failed{consumer} to 1 when the start throws", async () => {
		const m = createMetrics()
		const c = await startOptionalConsumer("votes", m.consumerStartFailed, async () => {
			throw new Error("TOPIC_AUTHORIZATION_FAILED")
		})
		expect(c).toBeNull()
		expect(m.render()).toContain('geodb_feeder_consumer_start_failed{consumer="votes"} 1')
	})

	it("returns the consumer and sets the gauge to 0 when it starts", async () => {
		const m = createMetrics()
		const c = await startOptionalConsumer("membership_keyed", m.consumerStartFailed, async () => ({ok: true}))
		expect(c).toEqual({ok: true})
		expect(m.render()).toContain('geodb_feeder_consumer_start_failed{consumer="membership_keyed"} 0')
	})

	it("has no labelled series while no optional consumer is enabled (the alert cannot fire flag-off)", () => {
		const out = createMetrics().render()
		expect(out).toContain("\ngeodb_feeder_consumer_start_failed 0")
		expect(out).not.toContain("geodb_feeder_consumer_start_failed{")
	})
})
