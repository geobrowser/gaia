import {describe, expect, it} from "vitest"
import {createMetrics, observeFreshness} from "../metrics.js"

const line = (text: string, series: string) =>
	text
		.split("\n")
		.find((l) => l.startsWith(`${series} `))
		?.split(" ")
		.pop()

describe("freshness", () => {
	it("observes now minus the Kafka timestamp into buckets that reach a minute", () => {
		const m = createMetrics()
		observeFreshness(m.freshness, "1000000", 1_000_300) // 0.3 s
		observeFreshness(m.freshness, "1000000", 1_045_000) // 45 s
		const text = m.render()
		expect(line(text, 'geodb_feeder_edit_freshness_seconds_bucket{le="0.25"}')).toBe("0")
		expect(line(text, 'geodb_feeder_edit_freshness_seconds_bucket{le="0.5"}')).toBe("1")
		expect(line(text, 'geodb_feeder_edit_freshness_seconds_bucket{le="30"}')).toBe("1")
		expect(line(text, 'geodb_feeder_edit_freshness_seconds_bucket{le="60"}')).toBe("2")
		expect(line(text, "geodb_feeder_edit_freshness_seconds_count")).toBe("2")
	})

	it("skips a missing timestamp and clamps a future one to zero", () => {
		const m = createMetrics()
		observeFreshness(m.proposalFreshness, undefined)
		observeFreshness(m.proposalFreshness, "0")
		observeFreshness(m.proposalFreshness, "2000000", 1_000_000)
		const text = m.render()
		expect(line(text, "geodb_feeder_proposal_freshness_seconds_count")).toBe("1")
		expect(line(text, "geodb_feeder_proposal_freshness_seconds_sum")).toBe("0")
	})

	it("leaves the apply histogram on its own buckets", () => {
		expect(createMetrics().render()).not.toContain('geodb_feeder_apply_seconds_bucket{le="60"}')
	})
})

describe("applied message timestamp", () => {
	it("is a separate gauge family keyed by partition, and leaves the last-applied wall clock alone", () => {
		const m = createMetrics()
		m.appliedMessageTimestamp.set({partition: "0"}, 1_700_000_000)
		m.appliedMessageTimestamp.set({partition: "1"}, 1_700_000_050.5)
		const text = m.render()
		expect(text).toContain("# TYPE geodb_feeder_applied_message_timestamp_seconds gauge")
		expect(line(text, 'geodb_feeder_applied_message_timestamp_seconds{partition="0"}')).toBe("1700000000")
		expect(line(text, 'geodb_feeder_applied_message_timestamp_seconds{partition="1"}')).toBe("1700000050.5")
		expect(text).toContain("# TYPE geodb_feeder_last_applied_timestamp_seconds gauge")
	})
})
