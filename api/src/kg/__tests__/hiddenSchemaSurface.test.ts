import {describe, expect, it} from "vitest"
import {postgraphileSchema} from "../postgraphile"
import {HIDDEN_SURFACE} from "../securitySignals"

/**
 * Guards the service-internal tables and write-path functions that must never be public.
 *
 * PostGraphile publishes every table and volatile function it can see unless a smart tag
 * omits it, so a new backend table is public by default. If a migration renames one of these,
 * the smart tag silently stops matching; this test is what will tell you.
 *
 * The names come from HIDDEN_SURFACE in securitySignals.ts, which the probe tripwire also reads, so
 * anything hidden is both asserted absent here and alarmed on if someone asks for it.
 */
describe("hidden schema surface", () => {
	const typeNames = Object.keys(postgraphileSchema.getTypeMap())
	const queryFields = Object.keys(postgraphileSchema.getQueryType()?.getFields() ?? {})
	const mutationFields = Object.keys(postgraphileSchema.getMutationType()?.getFields() ?? {})

	it.each([...HIDDEN_SURFACE.types])("does not expose the %s type", (name) => {
		expect(typeNames).not.toContain(name)
	})

	it("does not expose any root field with a hidden prefix", () => {
		const exposed = queryFields.filter((f) => HIDDEN_SURFACE.queryFieldPrefixes.some((p) => f.startsWith(p)))
		expect(exposed).toEqual([])
		// And the broader net, in case a new notification table appears under another name.
		expect(queryFields.filter((f) => /webhook|notification/i.test(f))).toEqual([])
	})

	it.each([...HIDDEN_SURFACE.mutations])("does not expose the %s mutation", (name) => {
		expect(mutationFields).not.toContain(name)
	})
})
