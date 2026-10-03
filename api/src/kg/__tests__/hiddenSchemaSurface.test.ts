import {describe, expect, it} from "vitest"
import {postgraphileSchema} from "../postgraphile"

/**
 * Guards the service-internal tables and write-path functions that must never be public.
 *
 * PostGraphile publishes every table and volatile function it can see unless a smart tag
 * omits it, so a new backend table is public by default. If a migration renames one of these,
 * the smart tag silently stops matching; this test is what will tell you.
 */
describe("hidden schema surface", () => {
	const typeNames = Object.keys(postgraphileSchema.getTypeMap())
	const queryFields = Object.keys(postgraphileSchema.getQueryType()?.getFields() ?? {})
	const mutationFields = Object.keys(postgraphileSchema.getMutationType()?.getFields() ?? {})

	it.each(["AppWebhook", "NotificationOutbox", "NotificationDelivery", "NotificationPollCursor"])(
		"does not expose the %s type",
		(name) => {
			expect(typeNames).not.toContain(name)
		},
	)

	it("does not expose notification or webhook root fields", () => {
		expect(queryFields.filter((f) => /webhook|notification/i.test(f))).toEqual([])
	})

	it.each(["refreshSpaceTopicSuggestions", "sampleFeedComposition", "recordFeedCompositionSample"])(
		"does not expose the %s mutation",
		(name) => {
			expect(mutationFields).not.toContain(name)
		},
	)
})
