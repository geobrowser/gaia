import {describe, expect, it, vi} from "vitest"
import {__testExports, EntityNameFilterPlugin} from "../entityNameFilterPlugin"

const {NAME_PROPERTY_ID, DESCRIPTION_PROPERTY_ID, buildResolver, operatorFragment} = __testExports

// ---------------------------------------------------------------------------
// Mock graphile sql tag — captures fragment shape for assertion
// ---------------------------------------------------------------------------
//
// We don't need a real Postgres execution environment for these tests — just
// enough of the `pgSql` API surface that the resolver calls. The real graphile
// `sql.fragment` returns an opaque object that ultimately renders to SQL +
// bind parameters; we replicate that contract with a tagged-template that
// preserves the literal text plus interpolated placeholders, and a `value()`
// + `join()` helper. This lets the test assert "the fragment contains the
// EXISTS structure and references the right property UUID" without a DB.

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
	join(fragments: Mark[], separator: string): Mark {
		return {kind: "fragment", payload: {fragments, separator}}
	},
}

const sourceAlias = sql.identifier("e")

/**
 * Recursively flatten a fragment into a single string for substring-match
 * assertions. Values get serialized as `<<value>>` so `text = ${val}` becomes
 * `text = <<the-string>>` and is greppable.
 */
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
		const obj = p as {fragments: unknown[]; separator: string}
		return obj.fragments.map(flatten).join(obj.separator)
	}
	return ""
}

// ---------------------------------------------------------------------------
// operatorFragment — per-operator SQL shape
// ---------------------------------------------------------------------------

describe("operatorFragment", () => {
	const propertyId = NAME_PROPERTY_ID

	it("isNull: false → EXISTS with text IS NOT NULL", () => {
		const f = operatorFragment(sql, sourceAlias, propertyId, "isNull", false)
		expect(f).not.toBeNull()
		const s = flatten(f)
		expect(s).toContain("EXISTS")
		expect(s).toContain("v.entity_id = e")
		expect(s).toContain(`<<${JSON.stringify(propertyId)}>>`)
		expect(s).toContain("v.text IS NOT NULL")
		expect(s).not.toMatch(/^\s*NOT\b/)
	})

	it("isNull: true → NOT EXISTS with text IS NOT NULL (entity has no name)", () => {
		const f = operatorFragment(sql, sourceAlias, propertyId, "isNull", true)
		const s = flatten(f)
		expect(s).toContain("NOT")
		expect(s).toContain("EXISTS")
		expect(s).toContain("v.text IS NOT NULL")
	})

	it("isNot: '' → EXISTS with v.text <> ''", () => {
		const f = operatorFragment(sql, sourceAlias, propertyId, "isNot", "")
		const s = flatten(f)
		expect(s).toContain("EXISTS")
		expect(s).toContain("v.text <>")
		expect(s).toContain('<<"">>')
	})

	it("equalTo / is → EXISTS with v.text =", () => {
		const f1 = operatorFragment(sql, sourceAlias, propertyId, "equalTo", "Alice")
		const f2 = operatorFragment(sql, sourceAlias, propertyId, "is", "Alice")
		expect(flatten(f1)).toContain("v.text =")
		expect(flatten(f2)).toContain("v.text =")
		expect(flatten(f1)).toContain('<<"Alice">>')
	})

	it("in → EXISTS with v.text = ANY(...)", () => {
		const f = operatorFragment(sql, sourceAlias, propertyId, "in", ["Alice", "Bob"])
		expect(flatten(f)).toContain("v.text = ANY(")
	})

	it("notIn → NOT EXISTS with v.text = ANY(...)", () => {
		const f = operatorFragment(sql, sourceAlias, propertyId, "notIn", ["Alice", "Bob"])
		const s = flatten(f)
		expect(s).toContain("NOT")
		expect(s).toContain("v.text = ANY(")
	})

	it("includes → EXISTS with v.text LIKE %X%", () => {
		const f = operatorFragment(sql, sourceAlias, propertyId, "includes", "Ali")
		const s = flatten(f)
		expect(s).toContain("v.text LIKE")
		expect(s).toContain('<<"%Ali%">>')
	})

	it("includesInsensitive → EXISTS with v.text ILIKE %X%", () => {
		const f = operatorFragment(sql, sourceAlias, propertyId, "includesInsensitive", "Ali")
		const s = flatten(f)
		expect(s).toContain("v.text ILIKE")
		expect(s).toContain('<<"%Ali%">>')
	})

	it("startsWith → EXISTS with v.text LIKE X%", () => {
		const s = flatten(operatorFragment(sql, sourceAlias, propertyId, "startsWith", "Ali"))
		expect(s).toContain("v.text LIKE")
		expect(s).toContain('<<"Ali%">>')
	})

	it("greaterThan → EXISTS with v.text >", () => {
		const s = flatten(operatorFragment(sql, sourceAlias, propertyId, "greaterThan", "M"))
		expect(s).toContain("v.text >")
		expect(s).toContain('<<"M">>')
	})

	it("unknown operator → returns null (caller falls through to default)", () => {
		const f = operatorFragment(sql, sourceAlias, propertyId, "distinctFrom", "x")
		expect(f).toBeNull()
	})

	it("uses the per-field property ID (description vs name)", () => {
		const fName = operatorFragment(sql, sourceAlias, NAME_PROPERTY_ID, "isNull", false)
		const fDesc = operatorFragment(sql, sourceAlias, DESCRIPTION_PROPERTY_ID, "isNull", false)
		expect(flatten(fName)).toContain(`<<${JSON.stringify(NAME_PROPERTY_ID)}>>`)
		expect(flatten(fDesc)).toContain(`<<${JSON.stringify(DESCRIPTION_PROPERTY_ID)}>>`)
	})
})

// ---------------------------------------------------------------------------
// buildResolver — composing multiple operators into one fragment
// ---------------------------------------------------------------------------

describe("buildResolver", () => {
	const resolver = buildResolver(NAME_PROPERTY_ID, sql)

	it("single operator returns a single fragment", () => {
		const out = resolver({sourceAlias, fieldValue: {isNull: false}})
		expect(out).not.toBeNull()
		const s = flatten(out)
		expect(s).toContain("EXISTS")
		expect(s).toContain("v.text IS NOT NULL")
	})

	it("multiple operators are AND-joined (matches `name: {isNull: false, isNot: ''}` shape)", () => {
		const out = resolver({sourceAlias, fieldValue: {isNull: false, isNot: ""}})
		const s = flatten(out)
		expect(s).toContain("v.text IS NOT NULL")
		expect(s).toContain("v.text <>")
		expect(s).toContain(" AND ")
	})

	it("returns null for null / non-object / empty input", () => {
		expect(resolver({sourceAlias, fieldValue: null})).toBeNull()
		expect(resolver({sourceAlias, fieldValue: undefined})).toBeNull()
		expect(resolver({sourceAlias, fieldValue: "string"})).toBeNull()
		expect(resolver({sourceAlias, fieldValue: {}})).toBeNull()
	})

	it("ignores unknown operators but still emits known ones", () => {
		const out = resolver({sourceAlias, fieldValue: {isNull: false, distinctFrom: "x"}})
		expect(out).not.toBeNull()
		const s = flatten(out)
		expect(s).toContain("v.text IS NOT NULL")
		expect(s).not.toContain("distinctFrom")
	})
})

// ---------------------------------------------------------------------------
// Plugin wiring — verifies build-hook wrap behavior
// ---------------------------------------------------------------------------

describe("EntityNameFilterPlugin (wiring)", () => {
	it("wraps connectionFilterRegisterResolver and substitutes for EntityFilter.name / .description", () => {
		const original = vi.fn()
		const build = {
			pgSql: sql,
			connectionFilterRegisterResolver: original,
		}

		// Hook system stub: capture the build-hook handler and invoke it.
		// biome-ignore lint/suspicious/noExplicitAny: simplified stub
		let buildHook: any
		const builder = {
			// biome-ignore lint/suspicious/noExplicitAny: simplified stub
			hook: (name: string, fn: any) => {
				if (name === "build") buildHook = fn
			},
		}

		EntityNameFilterPlugin(builder)
		const result = buildHook(build)
		expect(result).toBe(build)

		// Now simulate computed-columns plugin registering the default resolver.
		const defaultNameResolver = vi.fn()
		const defaultDescResolver = vi.fn()
		const defaultOtherResolver = vi.fn()

		result.connectionFilterRegisterResolver("EntityFilter", "name", defaultNameResolver)
		result.connectionFilterRegisterResolver("EntityFilter", "description", defaultDescResolver)
		result.connectionFilterRegisterResolver("EntityFilter", "createdAt", defaultOtherResolver)

		expect(original).toHaveBeenCalledTimes(3)
		const [nameCall, descCall, otherCall] = original.mock.calls as [unknown[], unknown[], unknown[]]

		// First call: name field — original called with our overrideResolver, NOT the default.
		expect(nameCall[0]).toBe("EntityFilter")
		expect(nameCall[1]).toBe("name")
		expect(nameCall[2]).not.toBe(defaultNameResolver) // ← substituted
		expect(typeof nameCall[2]).toBe("function")

		// Second call: description field — overridden too.
		expect(descCall[1]).toBe("description")
		expect(descCall[2]).not.toBe(defaultDescResolver)

		// Third call: createdAt — passes through unchanged.
		expect(otherCall[1]).toBe("createdAt")
		expect(otherCall[2]).toBe(defaultOtherResolver)
	})

	it("no-ops when connection-filter plugin not loaded", () => {
		const build = {pgSql: sql} // no connectionFilterRegisterResolver
		// biome-ignore lint/suspicious/noExplicitAny: simplified stub
		let buildHook: any
		const builder = {
			// biome-ignore lint/suspicious/noExplicitAny: simplified stub
			hook: (_: string, fn: any) => {
				buildHook = fn
			},
		}
		EntityNameFilterPlugin(builder)
		const result = buildHook(build)
		expect(result).toBe(build)
		expect((result as Record<string, unknown>).connectionFilterRegisterResolver).toBeUndefined()
	})

	it("no-ops when pgSql missing", () => {
		const build = {connectionFilterRegisterResolver: () => undefined} // no pgSql
		// biome-ignore lint/suspicious/noExplicitAny: simplified stub
		let buildHook: any
		const builder = {
			// biome-ignore lint/suspicious/noExplicitAny: simplified stub
			hook: (_: string, fn: any) => {
				buildHook = fn
			},
		}
		EntityNameFilterPlugin(builder)
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
		EntityNameFilterPlugin(builder)
		const result = buildHook(build)

		const someOtherDefault = vi.fn()
		result.connectionFilterRegisterResolver("PropertyFilter", "name", someOtherDefault)
		expect(original.mock.calls[0]?.[2]).toBe(someOtherDefault) // pass-through
	})
})
