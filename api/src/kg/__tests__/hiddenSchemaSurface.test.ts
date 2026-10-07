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

	// GEO-3088: what a user engages with, and the topic interests learned from it, are private.
	// They live in the `personalization` schema, which is safe only while PostGraphile is pointed at
	// `public` alone; this fails if the schema list grows or a table or function moves into `public`.
	it("does not expose per-user interest data", () => {
		const net = /interest|cooccurrence|personali[sz]ation/i
		expect(typeNames.filter((t) => net.test(t))).toEqual([])
		expect(queryFields.filter((f) => net.test(f))).toEqual([])
		expect(mutationFields.filter((f) => net.test(f))).toEqual([])
	})

	// GEO-3224: pair fit compares two people's positions, so it is served only by the private
	// /internal/pair-fit route; nothing about pair fit or matchmaking may be a GraphQL field.
	it("does not expose debate pair fit", () => {
		const net = /pairFit|matchmak/i
		expect(typeNames.filter((t) => net.test(t))).toEqual([])
		expect(queryFields.filter((f) => net.test(f))).toEqual([])
	})

	// GEO-3146: the stance map's inputs are per-user stances, and a position on it is an inferred
	// political opinion. Nothing about it may be public, under any inflection.
	it("does not expose the stance map", () => {
		const net = /stance_?map/i
		expect(typeNames.filter((t) => net.test(t))).toEqual([])
		expect(queryFields.filter((f) => net.test(f))).toEqual([])
		expect(mutationFields.filter((f) => net.test(f))).toEqual([])
	})

	// GEO-3235: what each signed-in user was shown, and per-item behaviour from analytics, are
	// private. Same reason and same net as above, for the feed-signal names.
	it("does not expose feed signals or seen sets", () => {
		const net = /feedItemSignal|feedUserSeen|feedSignal|walletAddressHash/i
		expect(typeNames.filter((t) => net.test(t))).toEqual([])
		expect(queryFields.filter((f) => net.test(f))).toEqual([])
		expect(mutationFields.filter((f) => net.test(f))).toEqual([])
	})

	// GEO-3143: the primer and anchor reads are public, and return entities, never the scores or
	// account weights they were chosen from.
	it("exposes the primer and anchor reads as entity lists", () => {
		const fields = postgraphileSchema.getQueryType()?.getFields() ?? {}
		for (const name of ["nextPrimerClaim", "anchorClaims"]) {
			expect(queryFields).toContain(name)
			expect(String(fields[name]?.type)).toMatch(/Entit/)
		}
		expect(fields.nextPrimerClaim?.args.map((a) => a.name)).toEqual(
			expect.arrayContaining(["userId", "spaceIds", "answeredClaimIds", "skippedClaimIds"]),
		)
	})
})
