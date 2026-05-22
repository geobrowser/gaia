import {beforeEach, describe, expect, it, vi} from "vitest"

// Mock the structured logger so we can assert on the search-like-op warn
// without hitting Sentry / stdout. Must precede the import of the plugin.
vi.mock("../../services/telemetry", () => ({
	log: {
		debug: vi.fn(),
		info: vi.fn(),
		warn: vi.fn(),
		error: vi.fn(),
	},
}))

import {log} from "../../services/telemetry"
import {__testExports, EntityComputedTextFilterPlugin} from "../entityComputedTextFilterPlugin"

const {NAME_PROPERTY_ID, DESCRIPTION_PROPERTY_ID, buildResolver, operatorPredicate, buildMergedExists} = __testExports

// ---------------------------------------------------------------------------
// Mock graphile sql tag — captures fragment shape for assertion
// ---------------------------------------------------------------------------
//
// We don't need a real Postgres execution environment for these tests — just
// enough of the `pgSql` API surface that the resolver calls. The real graphile
// `sql.fragment` returns an opaque object that ultimately renders to SQL +
// bind parameters; we replicate that contract with a tagged-template that
// preserves the literal text plus interpolated placeholders, plus `value()`,
// `identifier()`, and `join()` helpers. This lets us assert "the fragment
// contains the right EXISTS structure / property UUID / predicates" without
// running anything against a DB.

type Mark = {kind: "value" | "literal" | "fragment"; payload: unknown}

const sql = {
	fragment(strings: TemplateStringsArray, ...values: unknown[]): Mark {
		const parts: unknown[] = []
		for (let i = 0; i < strings.length; i++) {
			parts.push({kind: "literal", payload: strings[i]})
			if (i < values.length) parts.push(values[i])
		}
		return {kind: "fragment", payload: parts}
	},
	value(v: unknown): Mark {
		return {kind: "value", payload: v}
	},
	identifier(...parts: string[]): Mark {
		return {kind: "literal", payload: parts.join(".")}
	},
	// Mirror pg-sql2's real contract: the separator MUST be a string. The
	// real `sql.join` throws "Invalid separator - must be a string" on
	// anything else — including a `sql.fragment`. A previous version of
	// this plugin passed a fragment and only learned that at runtime in
	// staging. Throwing here keeps unit tests honest.
	join(fragments: Mark[], separator: string): Mark {
		if (typeof separator !== "string") {
			throw new Error("Invalid separator - must be a string")
		}
		return {kind: "fragment", payload: {fragments, separator}}
	},
}

const sourceAlias = sql.identifier("e")

function flatten(node: unknown): string {
	if (node === null || node === undefined) return ""
	if (typeof node === "string") return node
	if (typeof node !== "object") return String(node)
	const m = node as Mark
	if (m.kind === "literal") return String(m.payload)
	if (m.kind === "value") return `<<${JSON.stringify(m.payload)}>>`
	if (m.kind === "fragment") {
		const p = m.payload as unknown
		if (Array.isArray(p)) return p.map(flatten).join("")
		const obj = p as {fragments: unknown[]; separator: unknown}
		return obj.fragments.map(flatten).join(obj.separator as string)
	}
	return ""
}

// Convenience accessors for the tagged OpResult shape.
function asMerge(result: unknown): {kind: "merge"; innerPred: Mark} {
	if (!result || typeof result !== "object" || (result as {kind?: string}).kind !== "merge") {
		throw new Error(`expected merge result, got ${JSON.stringify(result)}`)
	}
	return result as {kind: "merge"; innerPred: Mark}
}

function asStandalone(result: unknown): {kind: "standalone"; fragment: Mark} {
	if (!result || typeof result !== "object" || (result as {kind?: string}).kind !== "standalone") {
		throw new Error(`expected standalone result, got ${JSON.stringify(result)}`)
	}
	return result as {kind: "standalone"; fragment: Mark}
}

// ---------------------------------------------------------------------------
// operatorPredicate — per-operator classification + inner predicate shape
// ---------------------------------------------------------------------------

describe("operatorPredicate", () => {
	const propertyId = NAME_PROPERTY_ID

	// --- Existence ---
	it("isNull: false → merge with v.text IS NOT NULL", () => {
		const r = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "isNull", false))
		expect(flatten(r.innerPred)).toContain("v.text IS NOT NULL")
	})

	it("isNull: true → standalone NOT EXISTS (… v.text IS NOT NULL)", () => {
		const r = asStandalone(operatorPredicate(sql, sourceAlias, propertyId, "isNull", true))
		const s = flatten(r.fragment)
		expect(s).toContain("NOT EXISTS")
		expect(s).toContain("v.text IS NOT NULL")
	})

	// --- Equality ---
	it("equalTo / is → merge with v.text = $val", () => {
		const r1 = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "equalTo", "Alice"))
		const r2 = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "is", "Alice"))
		expect(flatten(r1.innerPred)).toMatch(/v\.text = <<"Alice">>/)
		expect(flatten(r2.innerPred)).toMatch(/v\.text = <<"Alice">>/)
	})

	it("isNot / notEqualTo → merge with v.text <> $val (NULL implicitly excluded)", () => {
		const r1 = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "isNot", ""))
		const r2 = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "notEqualTo", "Bob"))
		expect(flatten(r1.innerPred)).toMatch(/v\.text <> <<"">>/)
		expect(flatten(r2.innerPred)).toMatch(/v\.text <> <<"Bob">>/)
	})

	it("isInsensitive / equalToInsensitive → merge with lower(v.text) = lower($val)", () => {
		const r1 = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "isInsensitive", "Alice"))
		const r2 = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "equalToInsensitive", "Alice"))
		expect(flatten(r1.innerPred)).toContain("lower(v.text) = lower(")
		expect(flatten(r2.innerPred)).toContain("lower(v.text) = lower(")
	})

	it("in → merge with v.text = ANY(...)", () => {
		const r = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "in", ["Alice", "Bob"]))
		expect(flatten(r.innerPred)).toContain("v.text = ANY(")
	})

	it("comparison ops → merge with v.text <op> $val", () => {
		expect(flatten(asMerge(operatorPredicate(sql, sourceAlias, propertyId, "lessThan", "M")).innerPred)).toMatch(
			/v\.text < <<"M">>/,
		)
		expect(
			flatten(asMerge(operatorPredicate(sql, sourceAlias, propertyId, "greaterThanOrEqualTo", "M")).innerPred),
		).toMatch(/v\.text >= <<"M">>/)
	})

	// --- Pattern matching: positive ---
	it("includes / startsWith / endsWith → merge with LIKE/ILIKE", () => {
		const inc = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "includes", "foo"))
		expect(flatten(inc.innerPred)).toContain('v.text LIKE <<"%foo%">>')

		const sw = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "startsWithInsensitive", "foo"))
		expect(flatten(sw.innerPred)).toContain('v.text ILIKE <<"foo%">>')

		const ew = asMerge(operatorPredicate(sql, sourceAlias, propertyId, "endsWith", "foo"))
		expect(flatten(ew.innerPred)).toContain('v.text LIKE <<"%foo">>')
	})

	// --- Negative pattern matching: NULL-safe inner predicates ---
	const NEGATIVE_OP_FIXTURES: ReadonlyArray<readonly [string, unknown]> = [
		["notIn", ["a", "b"]],
		["notIncludes", "foo"],
		["notIncludesInsensitive", "foo"],
		["notStartsWith", "foo"],
		["notStartsWithInsensitive", "foo"],
		["notEndsWith", "foo"],
		["notEndsWithInsensitive", "foo"],
		["notLike", "%foo%"],
		["notLikeInsensitive", "%foo%"],
		["isNotInsensitive", "foo"],
		["notEqualToInsensitive", "foo"],
	]

	for (const [op, val] of NEGATIVE_OP_FIXTURES) {
		it(`${op} → merge with "v.text IS NOT NULL AND NOT (...)" so name-less entities are excluded`, () => {
			const r = asMerge(operatorPredicate(sql, sourceAlias, propertyId, op, val))
			const s = flatten(r.innerPred)
			expect(s).toContain("v.text IS NOT NULL")
			expect(s).toContain("AND NOT (")
		})
	}

	// --- Unknown ops fall back ---
	it("unknown operator → null (caller delegates to default resolver)", () => {
		expect(operatorPredicate(sql, sourceAlias, propertyId, "distinctFrom", "x")).toBeNull()
		expect(operatorPredicate(sql, sourceAlias, propertyId, "notDistinctFrom", "x")).toBeNull()
		expect(operatorPredicate(sql, sourceAlias, propertyId, "inInsensitive", ["a"])).toBeNull()
	})

	// --- Per-field property ID dispatch ---
	it("uses the per-field property ID (description vs name) — not relevant for inner preds, but property hits buildMergedExists", () => {
		const nameExists = buildMergedExists(sql, sourceAlias, NAME_PROPERTY_ID, [
			sql.fragment`v.text = ${sql.value("X")}`,
		])
		const descExists = buildMergedExists(sql, sourceAlias, DESCRIPTION_PROPERTY_ID, [
			sql.fragment`v.text = ${sql.value("X")}`,
		])
		expect(flatten(nameExists)).toContain(`<<${JSON.stringify(NAME_PROPERTY_ID)}>>`)
		expect(flatten(descExists)).toContain(`<<${JSON.stringify(DESCRIPTION_PROPERTY_ID)}>>`)
		// Sanity: SDK constants resolve to the canonical UUIDs (any format).
		expect(NAME_PROPERTY_ID).toMatch(/^a126ca53-?0c8e-?48d5-?b888-?82c734c38935$/)
		expect(DESCRIPTION_PROPERTY_ID).toMatch(/^9b1f76ff-?9711-?404c-?861e-?59dc3fa7d037$/)
	})
})

// ---------------------------------------------------------------------------
// buildResolver — merging predicates into a single EXISTS
// ---------------------------------------------------------------------------

describe("buildResolver — multi-operator merging (P2 fix)", () => {
	beforeEach(() => {
		vi.clearAllMocks()
	})

	const noopDefault = vi.fn(() => null)
	const resolver = buildResolver("name", NAME_PROPERTY_ID, sql, noopDefault)

	/**
	 * Count how many `EXISTS (` substrings appear at the top level of the
	 * fragment, excluding the standalone `NOT EXISTS` form. Used to verify
	 * that multi-operator filters collapse to ONE EXISTS, not N.
	 */
	function countExistsClauses(s: string): number {
		// Split on "NOT EXISTS" first so we don't double-count: each NOT
		// EXISTS contains an EXISTS substring.
		const withoutNotExists = s.replaceAll("NOT EXISTS", "")
		return (withoutNotExists.match(/\bEXISTS\s*\(/g) ?? []).length
	}

	it("single positive op → exactly one EXISTS containing the predicate", () => {
		const out = resolver({sourceAlias, fieldValue: {is: "Alice"}})
		const s = flatten(out)
		expect(countExistsClauses(s)).toBe(1)
		expect(s).toContain('v.text = <<"Alice">>')
	})

	it("two positive ops merge into ONE EXISTS so the same row must satisfy both (P2 fix)", () => {
		// This is the reviewer's example: {startsWith: "A", endsWith: "Z"}.
		// Before the merge fix, we emitted two separate EXISTS clauses, and
		// could match an entity whose space-A name = "Alice" + space-B name
		// = "Beta-Z" — neither row alone satisfies both predicates.
		// After the fix, both predicates are AND-ed inside one EXISTS, so
		// at least one row must match both.
		const out = resolver({sourceAlias, fieldValue: {startsWith: "A", endsWith: "Z"}})
		const s = flatten(out)
		expect(countExistsClauses(s)).toBe(1)
		expect(s).toContain('v.text LIKE <<"A%">>')
		expect(s).toContain('v.text LIKE <<"%Z">>')
		// Both predicates AND-ed inside the EXISTS.
		expect(s).toMatch(/EXISTS[\s\S]*v\.text LIKE <<"A%">>[\s\S]*AND[\s\S]*v\.text LIKE <<"%Z">>/)
	})

	it("classic isNull:false + isNot:'' → ONE EXISTS with both predicates", () => {
		const out = resolver({sourceAlias, fieldValue: {isNull: false, isNot: ""}})
		const s = flatten(out)
		expect(countExistsClauses(s)).toBe(1)
		expect(s).toContain("v.text IS NOT NULL")
		expect(s).toContain('v.text <> <<"">>')
	})

	it("positive + negative ops merge into ONE EXISTS (same row must satisfy both)", () => {
		const out = resolver({sourceAlias, fieldValue: {is: "Alice", notLike: "Bob%"}})
		const s = flatten(out)
		expect(countExistsClauses(s)).toBe(1)
		expect(s).toContain('v.text = <<"Alice">>')
		// notLike contributes its IS NOT NULL AND NOT (LIKE …) inner pred.
		expect(s).toContain("v.text IS NOT NULL")
		expect(s).toContain("AND NOT (")
	})

	it("isNull:true alone → standalone NOT EXISTS, no merged EXISTS", () => {
		const out = resolver({sourceAlias, fieldValue: {isNull: true}})
		const s = flatten(out)
		expect(s).toContain("NOT EXISTS")
		// Only the NOT EXISTS, no merged-positive EXISTS.
		expect(countExistsClauses(s)).toBe(0)
	})

	it("isNull:true + positive op → AND of standalone NOT EXISTS + merged EXISTS", () => {
		// User-pathological combo (asks for "no name" AND "name = X" — never
		// matches anything). But the SQL shape should still be well-formed.
		const out = resolver({sourceAlias, fieldValue: {isNull: true, is: "Alice"}})
		const s = flatten(out)
		expect(s).toContain("NOT EXISTS")
		expect(countExistsClauses(s)).toBe(1) // one merged EXISTS for the positive op
		expect(s).toContain(" AND ")
	})

	it("returns null for null / non-object / empty input", () => {
		expect(resolver({sourceAlias, fieldValue: null})).toBeNull()
		expect(resolver({sourceAlias, fieldValue: undefined})).toBeNull()
		expect(resolver({sourceAlias, fieldValue: "string"})).toBeNull()
		expect(resolver({sourceAlias, fieldValue: {}})).toBeNull()
	})
})

// ---------------------------------------------------------------------------
// buildResolver — fallback semantics for unknown operators
// ---------------------------------------------------------------------------

describe("buildResolver — fallback to default resolver", () => {
	it("delegates unknown operators to the default resolver, AND-joins with fast-path ops", () => {
		const defaultResolve = vi.fn((args) => sql.fragment`<<DEFAULT(${sql.value(JSON.stringify(args.fieldValue))})>>`)
		const r = buildResolver("name", NAME_PROPERTY_ID, sql, defaultResolve)
		const out = r({sourceAlias, fieldValue: {is: "X", distinctFrom: "Y"}, extra: "ctx"})

		expect(out).not.toBeNull()
		const s = flatten(out)

		// Fast-path part (the merged EXISTS).
		expect(s).toContain('v.text = <<"X">>')

		// Fallback part (default got called for the single unknown op only).
		expect(defaultResolve).toHaveBeenCalledTimes(1)
		expect(defaultResolve.mock.calls[0]?.[0]).toMatchObject({
			sourceAlias,
			fieldValue: {distinctFrom: "Y"},
			extra: "ctx", // other input fields preserved
		})
		expect(s).toContain("DEFAULT")
		expect(s).toContain(" AND ")
	})

	it("falls through entirely to default when every op is unknown", () => {
		const defaultResolve = vi.fn(() => sql.fragment`<<DEFAULT_ONLY>>`)
		const r = buildResolver("name", NAME_PROPERTY_ID, sql, defaultResolve)
		const out = r({sourceAlias, fieldValue: {distinctFrom: "x", notDistinctFrom: "y"}})

		expect(out).not.toBeNull()
		expect(defaultResolve).toHaveBeenCalledTimes(2) // one per unknown op
		expect(flatten(out)).toContain("DEFAULT_ONLY")
		expect(flatten(out)).toContain(" AND ")
	})
})

// ---------------------------------------------------------------------------
// Plugin wiring — verifies build-hook wrap behavior
// ---------------------------------------------------------------------------

describe("EntityComputedTextFilterPlugin (wiring)", () => {
	it("wraps connectionFilterRegisterResolver and substitutes for EntityFilter.name / .description", () => {
		const original = vi.fn()
		const build = {pgSql: sql, connectionFilterRegisterResolver: original}

		// biome-ignore lint/suspicious/noExplicitAny: simplified stub
		let buildHook: any
		const builder = {
			// biome-ignore lint/suspicious/noExplicitAny: simplified stub
			hook: (name: string, fn: any) => {
				if (name === "build") buildHook = fn
			},
		}

		EntityComputedTextFilterPlugin(builder)
		const result = buildHook(build)
		expect(result).toBe(build)

		const defaultNameResolver = vi.fn()
		const defaultDescResolver = vi.fn()
		const defaultOtherResolver = vi.fn()

		result.connectionFilterRegisterResolver("EntityFilter", "name", defaultNameResolver)
		result.connectionFilterRegisterResolver("EntityFilter", "description", defaultDescResolver)
		result.connectionFilterRegisterResolver("EntityFilter", "createdAt", defaultOtherResolver)

		expect(original).toHaveBeenCalledTimes(3)
		const [nameCall, descCall, otherCall] = original.mock.calls as [unknown[], unknown[], unknown[]]

		expect(nameCall[0]).toBe("EntityFilter")
		expect(nameCall[1]).toBe("name")
		expect(nameCall[2]).not.toBe(defaultNameResolver) // ← substituted
		expect(typeof nameCall[2]).toBe("function")

		expect(descCall[1]).toBe("description")
		expect(descCall[2]).not.toBe(defaultDescResolver)

		expect(otherCall[1]).toBe("createdAt")
		expect(otherCall[2]).toBe(defaultOtherResolver) // pass-through
	})

	it("no-ops when connection-filter plugin not loaded", () => {
		const build = {pgSql: sql}
		// biome-ignore lint/suspicious/noExplicitAny: simplified stub
		let buildHook: any
		const builder = {
			// biome-ignore lint/suspicious/noExplicitAny: simplified stub
			hook: (_: string, fn: any) => {
				buildHook = fn
			},
		}
		EntityComputedTextFilterPlugin(builder)
		const result = buildHook(build)
		expect(result).toBe(build)
		expect((result as Record<string, unknown>).connectionFilterRegisterResolver).toBeUndefined()
	})

	it("no-ops when pgSql missing", () => {
		const build = {connectionFilterRegisterResolver: () => undefined}
		// biome-ignore lint/suspicious/noExplicitAny: simplified stub
		let buildHook: any
		const builder = {
			// biome-ignore lint/suspicious/noExplicitAny: simplified stub
			hook: (_: string, fn: any) => {
				buildHook = fn
			},
		}
		EntityComputedTextFilterPlugin(builder)
		const result = buildHook(build)
		expect(result).toBe(build)
	})

	it("does not substitute on filter types other than EntityFilter", () => {
		const original = vi.fn()
		const build = {pgSql: sql, connectionFilterRegisterResolver: original}
		// biome-ignore lint/suspicious/noExplicitAny: simplified stub
		let buildHook: any
		const builder = {
			// biome-ignore lint/suspicious/noExplicitAny: simplified stub
			hook: (_: string, fn: any) => {
				buildHook = fn
			},
		}
		EntityComputedTextFilterPlugin(builder)
		const result = buildHook(build)

		const someOtherDefault = vi.fn()
		result.connectionFilterRegisterResolver("PropertyFilter", "name", someOtherDefault)
		expect(original.mock.calls[0]?.[2]).toBe(someOtherDefault) // pass-through
	})
})

// ---------------------------------------------------------------------------
// Search-like operator detection — warn-level log when callers use the
// GraphQL filter for what looks like text search instead of /search
// ---------------------------------------------------------------------------

describe("buildResolver — search-like operator warn", () => {
	beforeEach(() => {
		vi.clearAllMocks()
	})

	const resolver = buildResolver(
		"name",
		NAME_PROPERTY_ID,
		sql,
		vi.fn(() => null),
	)

	const SEARCH_LIKE_FIXTURES: ReadonlyArray<readonly [string, unknown]> = [
		["includes", "alice"],
		["includesInsensitive", "alice"],
		["like", "%alice%"],
		["likeInsensitive", "%alice%"],
		["startsWith", "Ali"],
		["startsWithInsensitive", "Ali"],
		["endsWith", "ce"],
		["endsWithInsensitive", "ce"],
		["notIncludes", "bob"],
		["notLike", "%bob%"],
		["notStartsWith", "Bob"],
		["notEndsWith", "X"],
	]

	for (const [op, val] of SEARCH_LIKE_FIXTURES) {
		it(`logs warn for ${op} (signaling caller should use /search)`, () => {
			resolver({sourceAlias, fieldValue: {[op]: val}})

			expect(log.warn).toHaveBeenCalledWith(
				"GraphQL pattern-match filter on entity name/description — prefer /search",
				expect.objectContaining({field: "name", operator: op, value: val}),
			)
		})
	}

	it("does NOT warn for equality / existence ops (typeahead-style is fine)", () => {
		resolver({sourceAlias, fieldValue: {is: "Alice"}})
		resolver({sourceAlias, fieldValue: {isNot: ""}})
		resolver({sourceAlias, fieldValue: {isNull: false}})
		resolver({sourceAlias, fieldValue: {isNull: true}})
		resolver({sourceAlias, fieldValue: {in: ["a", "b"]}})
		resolver({sourceAlias, fieldValue: {isInsensitive: "alice"}})

		expect(log.warn).not.toHaveBeenCalled()
	})

	it("warns once per search-like op in a mixed bag, not for the equality op", () => {
		resolver({sourceAlias, fieldValue: {is: "Alice", includes: "li", startsWith: "A"}})

		expect(log.warn).toHaveBeenCalledTimes(2)
		expect(log.warn).toHaveBeenCalledWith(
			expect.stringContaining("prefer /search"),
			expect.objectContaining({operator: "includes"}),
		)
		expect(log.warn).toHaveBeenCalledWith(
			expect.stringContaining("prefer /search"),
			expect.objectContaining({operator: "startsWith"}),
		)
	})

	it("uses the GraphQL field name (description vs name) in the log", () => {
		const descResolver = buildResolver(
			"description",
			DESCRIPTION_PROPERTY_ID,
			sql,
			vi.fn(() => null),
		)
		descResolver({sourceAlias, fieldValue: {includes: "engineer"}})

		expect(log.warn).toHaveBeenCalledWith(expect.any(String), expect.objectContaining({field: "description"}))
	})

	it("truncates very long values in the log to keep payload bounded", () => {
		const huge = "x".repeat(500)
		resolver({sourceAlias, fieldValue: {includes: huge}})

		const call = (log.warn as ReturnType<typeof vi.fn>).mock.calls[0]
		expect(call?.[1]?.value).toHaveLength(200) // truncated from 500 → 200
	})
})

// ---------------------------------------------------------------------------
// Real pg-sql2 contract — catches API drift between our mock and the lib
// the plugin actually runs against in production.
// ---------------------------------------------------------------------------
//
// The `sql.join must take a string separator` bug that broke every
// name/description filter on staging slipped past the existing unit tests
// because the mock's `join` happily accepted a fragment. This block exercises
// the same code paths against REAL pg-sql2 so any contract divergence —
// signature, return shape, throw behavior of `value`/`fragment`/`join` /
// `identifier` — is caught at unit-test time in CI rather than production.
//
// No DB needed: we just call `realSql.compile(node)` to verify the produced
// fragment compiles to a `{text, values}` pair without throwing.

import * as realSql from "pg-sql2"

describe("entityComputedTextFilterPlugin — real pg-sql2 contract (CI integration)", () => {
	const realSourceAlias = realSql.identifier("e")

	// Every operator the plugin claims to fast-path. If any of these throws
	// on real pg-sql2, this test catches it before we ever ship.
	const ALL_FAST_PATH_OPS: ReadonlyArray<readonly [string, unknown]> = [
		// Existence
		["isNull", false],
		["isNull", true],
		// Equality
		["is", "Alice"],
		["equalTo", "Alice"],
		["isNot", ""],
		["notEqualTo", "Bob"],
		["isInsensitive", "Alice"],
		["equalToInsensitive", "Alice"],
		["isNotInsensitive", "Bob"],
		["notEqualToInsensitive", "Bob"],
		// Set
		["in", ["A", "B", "C"]],
		["notIn", ["X", "Y"]],
		// Comparisons
		["lessThan", "M"],
		["lessThanOrEqualTo", "M"],
		["greaterThan", "M"],
		["greaterThanOrEqualTo", "M"],
		// LIKE / ILIKE positives
		["includes", "ali"],
		["includesInsensitive", "ali"],
		["startsWith", "Bob"],
		["startsWithInsensitive", "Bob"],
		["endsWith", "Z"],
		["endsWithInsensitive", "Z"],
		["like", "%a%"],
		["likeInsensitive", "%a%"],
		// LIKE / ILIKE negatives (NULL-safe shape)
		["notIncludes", "ali"],
		["notIncludesInsensitive", "ali"],
		["notStartsWith", "Bob"],
		["notStartsWithInsensitive", "Bob"],
		["notEndsWith", "Z"],
		["notEndsWithInsensitive", "Z"],
		["notLike", "%a%"],
		["notLikeInsensitive", "%a%"],
	]

	it.each(ALL_FAST_PATH_OPS.map(([op, val]) => [op, val] as const))(
		"operatorPredicate('%s') compiles via real pg-sql2 without throwing",
		(op, val) => {
			const result = operatorPredicate(realSql, realSourceAlias, NAME_PROPERTY_ID, op, val)
			expect(result).not.toBeNull()

			// Whichever shape it returns, the SQL fragment inside must be
			// compilable by real pg-sql2.
			if (!result) return
			const node = result.kind === "merge" ? result.innerPred : result.fragment
			expect(() => realSql.compile(node)).not.toThrow()
		},
	)

	it("buildResolver — single op compiles", () => {
		const resolver = buildResolver(
			"name",
			NAME_PROPERTY_ID,
			realSql,
			vi.fn(() => null),
		)
		const out = resolver({sourceAlias: realSourceAlias, fieldValue: {is: "Alice"}})
		expect(out).not.toBeNull()
		expect(() => realSql.compile(out)).not.toThrow()
	})

	it("buildResolver — multi-op merge compiles (regression: sql.join string-separator)", () => {
		// The exact case that broke staging: two ops triggers buildMergedExists's
		// `sql.join(innerPreds, " AND ")`. If that ever drifts back to passing
		// a fragment, real pg-sql2 throws and this test fails.
		const resolver = buildResolver(
			"name",
			NAME_PROPERTY_ID,
			realSql,
			vi.fn(() => null),
		)
		const out = resolver({sourceAlias: realSourceAlias, fieldValue: {isNull: false, isNot: ""}})
		expect(out).not.toBeNull()

		const compiled = realSql.compile(out)
		expect(compiled.text).toContain("EXISTS")
		expect(compiled.text).toContain("v.text IS NOT NULL")
		expect(compiled.text).toContain("v.text <>")
		// Bind values flow through correctly.
		expect(compiled.values).toContain("")
	})

	it("buildResolver — standalone + merged compiles (top-level join also uses string separator)", () => {
		// {isNull: true} → standalone NOT EXISTS; {is: "X"} → merged EXISTS.
		// Two top-level fragments AND-ed via sql.join — the second sql.join
		// site that was previously passing a fragment. Real pg-sql2 must
		// accept the result.
		const resolver = buildResolver(
			"name",
			NAME_PROPERTY_ID,
			realSql,
			vi.fn(() => null),
		)
		const out = resolver({sourceAlias: realSourceAlias, fieldValue: {isNull: true, is: "Alice"}})
		expect(out).not.toBeNull()

		const compiled = realSql.compile(out)
		expect(compiled.text).toContain("NOT EXISTS")
		expect(compiled.text).toContain("EXISTS")
		expect(compiled.text).toContain("AND")
	})

	it("buildResolver — many-op merge (5 predicates) compiles", () => {
		const resolver = buildResolver(
			"name",
			NAME_PROPERTY_ID,
			realSql,
			vi.fn(() => null),
		)
		const out = resolver({
			sourceAlias: realSourceAlias,
			fieldValue: {
				isNull: false,
				isNot: "",
				startsWith: "A",
				endsWith: "Z",
				includes: "li",
			},
		})
		expect(out).not.toBeNull()
		expect(() => realSql.compile(out)).not.toThrow()
	})

	it("buildResolver — fallback fragment (from default resolver) composes with merged EXISTS", () => {
		// Simulates a mixed-bag where the default resolver returns a real SQL
		// fragment for an unknown op. Verifies the AND-join works against
		// realSql when one of the fragments comes from outside the plugin.
		const defaultResolve = vi.fn(({fieldValue}) => {
			// Mimic what connection-filter would emit for distinctFrom.
			const op = Object.keys(fieldValue)[0]
			const val = Object.values(fieldValue)[0]
			return realSql.fragment`entities_name(${realSourceAlias}) IS DISTINCT FROM ${realSql.value(val)} /* op=${realSql.value(op)} */`
		})
		const resolver = buildResolver("name", NAME_PROPERTY_ID, realSql, defaultResolve)
		const out = resolver({sourceAlias: realSourceAlias, fieldValue: {is: "Alice", distinctFrom: "Bob"}})
		expect(out).not.toBeNull()
		expect(defaultResolve).toHaveBeenCalledTimes(1)
		expect(() => realSql.compile(out)).not.toThrow()
	})
})
