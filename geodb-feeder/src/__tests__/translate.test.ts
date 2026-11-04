import {describe, expect, it} from "vitest"
import {conformingId, nonceOf} from "../ids.js"
import {genesisLanguageId as languageIdOf} from "../languages.js"
import {
	type GrcOp,
	type TranslateContext,
	floatToDecimal,
	translateOps,
	valueZon,
	zigString,
	zonDoc,
} from "../translate.js"

/** A 16-byte v4-shaped id whose first byte is `n`, as GRC-20 decodes ids. */
const id = (n: number) => new Uint8Array(Array.from({length: 16}, (_, i) => (i === 0 ? n : i === 6 ? 0x40 : i === 8 ? 0x80 : i)))
const hex = (n: number) => Buffer.from(id(n)).toString("hex")

function context() {
	const entities = new Set<string>()
	const statements = new Set<string>()
	const ctx: TranslateContext = {
		knownEntity: (e) => entities.has(e),
		markEntity: (e) => void entities.add(e),
		knownStmt: (s) => statements.has(s),
		markStmt: (s) => void statements.add(s),
	}
	return {ctx, entities, statements}
}

describe("valueZon", () => {
	it("keeps date precision and accepts the Z form", () => {
		expect(valueZon({type: "date", value: "2020-05-17"})).toEqual({zon: '.{ .date = "2,2020,5,17" }'})
		expect(valueZon({type: "date", value: "2020-05-17Z"})).toEqual({zon: '.{ .date = "2,2020,5,17" }'})
		expect(valueZon({type: "date", value: "2001-03Z"})).toEqual({zon: '.{ .date = "1,2001,3,0" }'})
		expect(valueZon({type: "date", value: "1999"})).toEqual({zon: '.{ .date = "0,1999,0,0" }'})
		expect(valueZon({type: "date", value: "-0044-03-15"})).toEqual({zon: '.{ .date = "2,-44,3,15" }'})
		expect(valueZon({type: "date", value: "last week"})).toEqual({skip: "date-unparsable"})
	})

	const english = "090adac0-fca4-822e-8e71-9263e67620ec"
	const french = languageIdOf("fr")

	it("passes a text's known language as its tag, and flags a language with no tag", () => {
		expect(valueZon({type: "text", value: "plain"})).toEqual({zon: '.{ .text = "plain" }'})
		expect(valueZon({type: "text", value: "hello", language: english})).toEqual({zon: '.{ .text = "hello" }', tag: "en"})
		expect(valueZon({type: "text", value: "bonjour", language: Uint8Array.from(Buffer.from(french, "hex"))})).toEqual({
			zon: '.{ .text = "bonjour" }',
			tag: "fr",
		})
		expect(valueZon({type: "text", value: "?", language: id(9)})).toEqual({zon: '.{ .text = "?" }', lang: true})
	})

	it("translates the scalar kinds", () => {
		expect(valueZon({type: "datetime", value: "2020-01-01T00:00:00Z"})).toEqual({zon: '.{ .instant = "1577836800000000" }'})
		expect(valueZon({type: "datetime", value: "nope"})).toEqual({skip: "datetime-unparsable"})
		expect(valueZon({type: "float", value: 1.5e-7})).toEqual({zon: '.{ .decimal = "15,-8" }'})
		expect(valueZon({type: "float", value: Number.NaN})).toEqual({skip: "float-nonfinite"})
		expect(valueZon({type: "decimal", mantissa: {type: "i64", value: 1200n}, exponent: -2})).toEqual({zon: '.{ .decimal = "12,0" }'})
		expect(valueZon({type: "decimal", mantissa: {type: "big", value: 1n}, exponent: 0})).toEqual({skip: "decimal-bigmantissa"})
		expect(valueZon({type: "point", lon: 1, lat: 2})).toEqual({zon: '.{ .point = "1, 2" }'})
		expect(valueZon({type: "point", lon: 1, lat: 2, alt: 3})).toEqual({zon: '.{ .point = "1, 2, 3" }'})
		// geo-db keeps 9 fractional digits of a degree and 6 of a metre; the kg-indexer text carries 12
		expect(valueZon({type: "point", lon: 23.808333333333, lat: 52.402833333333})).toEqual({zon: '.{ .point = "23.808333333, 52.402833333" }'})
		expect(valueZon({type: "point", lon: -75.525277777778, lat: 10.423611111111})).toEqual({zon: '.{ .point = "-75.525277778, 10.423611111" }'})
		expect(valueZon({type: "point", lon: 30.316666666667, lat: 59.95})).toEqual({zon: '.{ .point = "30.316666667, 59.95" }'})
		expect(valueZon({type: "point", lon: 1e-12, lat: -1e-12, alt: 1.23456789})).toEqual({zon: '.{ .point = "0, 0, 1.234568" }'})
		expect(valueZon({type: "boolean", value: false})).toEqual({zon: ".{ .boolean = false }"})
		expect(valueZon({type: "integer", value: 42})).toEqual({zon: ".{ .number = 42 }"})
		expect(valueZon({type: "bytes", value: ""})).toEqual({skip: "value-bytes"})
	})
})

describe("zigString", () => {
	it("escapes quotes, backslashes and control characters", () => {
		expect(zigString('a"b\\c\nd\te\u0001')).toBe('"a\\"b\\\\c\\nd\\te\\x01"')
	})
	it("replaces lone surrogates", () => {
		expect(zigString("x\ud800y")).toBe('"x�y"')
	})
})

describe("floatToDecimal", () => {
	it("normalizes trailing zeros", () => {
		expect(floatToDecimal(1200)).toEqual([12n, 2])
		expect(floatToDecimal(0)).toEqual([0n, 0])
		expect(floatToDecimal(Number.POSITIVE_INFINITY)).toBeNull()
	})
})

describe("ids", () => {
	it("keeps v4 ids and remaps the rest deterministically", () => {
		expect(conformingId("entity", hex(1))).toBe(hex(1))
		const remapped = conformingId("entity", "00000000000000000000000000000001")
		expect(remapped).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
		expect(conformingId("entity", "00000000000000000000000000000001")).toBe(remapped)
		expect(conformingId("statement", "00000000000000000000000000000001")).not.toBe(remapped)
	})
	it("derives a v4-shaped, stable nonce", () => {
		expect(nonceOf("a19c345ab9866679b001d7d2138d88a1")).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
		expect(nonceOf("a19c345ab9866679b001d7d2138d88a1")).toBe(nonceOf("a19c345ab9866679b001d7d2138d88a1"))
	})
})

describe("translateOps", () => {
	const ops: GrcOp[] = [
		{
			type: "createEntity",
			id: id(1),
			values: [
				{property: id(2), value: {type: "text", value: 'Hi "x"\n', language: id(9)}},
				{property: id(2), value: {type: "text", value: "later", language: id(9)}},
				{property: id(3), value: {type: "date", value: "2020-05-17Z"}},
				{property: id(4), value: {type: "date", value: "1999"}},
			],
		},
		{type: "createRelation", id: id(6), from: id(1), to: id(7), relationType: id(8)},
		{type: "updateEntity", id: id(10), set: [{property: id(2), value: {type: "integer", value: 42}}], unset: [{property: id(3)}]},
		{type: "deleteRelation", id: id(6)},
		{type: "deleteEntity", id: id(11)},
		{type: "somethingNew"},
	]

	it("writes a relation's position only when asked, and drops one geo-db would refuse", () => {
		const rel = (n: number, position?: unknown): GrcOp => ({type: "createRelation", id: id(n), from: id(1), to: id(7), relationType: id(8), position})
		const add = (n: number, position = "") =>
			`.{ .add = .{ .id = "${conformingId("statement", hex(n))}", .entity = "${hex(1)}", .property = "${hex(8)}", .value = .{ .reference = "${hex(7)}" }${position} } }`
		const ops = [rel(20, "a0V"), rel(21), rel(22, ""), rel(23, "a".repeat(65)), rel(24, "a\u00e9"), rel(25, 'q"\\')]

		const on = translateOps(ops, context().ctx, {positions: true})
		expect(on.intents.slice(1)).toEqual([
			add(20, ', .position = "a0V"'),
			add(21),
			add(22),
			add(23),
			add(24),
			add(25, ', .position = "q\\"\\\\"'),
		])
		expect(on.stats["relation-position-kept"]).toBe(2)
		expect(on.stats["relation-position-dropped"]).toBe(3)

		const off = translateOps(ops, context().ctx)
		expect(off.intents.slice(1)).toEqual([add(20), add(21), add(22), add(23), add(24), add(25)])
		expect(off.stats["relation-position-kept"]).toBeUndefined()
		expect(off.stats["relation-position-dropped"]).toBe(5)
	})

	it("sets a text with a known language with its tag, and one with an unknown language without", () => {
		const {ctx} = context()
		const {intents, stats} = translateOps(
			[
				{
					type: "createEntity",
					id: id(1),
					values: [
						{property: id(2), value: {type: "text", value: "hello", language: "090adac0fca4822e8e719263e67620ec"}},
						{property: id(3), value: {type: "text", value: "huh", language: id(9)}},
						{property: id(4), value: {type: "text", value: "plain"}},
					],
				},
			],
			ctx,
		)
		expect(intents).toEqual([
			`.{ .create_entity = "${hex(1)}" }`,
			`.{ .set = .{ .entity = "${hex(1)}", .property = "${hex(2)}", .value = .{ .text = "hello" }, .language = "en" } }`,
			`.{ .set = .{ .entity = "${hex(1)}", .property = "${hex(3)}", .value = .{ .text = "huh" } } }`,
			`.{ .set = .{ .entity = "${hex(1)}", .property = "${hex(4)}", .value = .{ .text = "plain" } } }`,
		])
		expect(stats).toMatchObject({"text-language-kept": 1, "text-language-dropped": 1, "values-set": 3})
	})

	it("emits hex op ids, last-value-wins per language cell, and date precision", () => {
		const {ctx} = context()
		const {intents, touched, stats} = translateOps(ops, ctx)
		expect(intents).toEqual([
			`.{ .create_entity = "${hex(1)}" }`,
			`.{ .set = .{ .entity = "${hex(1)}", .property = "${hex(2)}", .value = .{ .text = "later" } } }`,
			`.{ .set = .{ .entity = "${hex(1)}", .property = "${hex(3)}", .value = .{ .date = "2,2020,5,17" } } }`,
			`.{ .set = .{ .entity = "${hex(1)}", .property = "${hex(4)}", .value = .{ .date = "0,1999,0,0" } } }`,
			`.{ .add = .{ .id = "${hex(6)}", .entity = "${hex(1)}", .property = "${hex(8)}", .value = .{ .reference = "${hex(7)}" } } }`,
			`.{ .create_entity = "${hex(10)}" }`,
			`.{ .set = .{ .entity = "${hex(10)}", .property = "${hex(2)}", .value = .{ .number = 42 } } }`,
			`.{ .unset = .{ .entity = "${hex(10)}", .property = "${hex(3)}" } }`,
			`.{ .set_active = .{ .statement = "${hex(6)}", .active = false } }`,
		])
		expect(touched).toEqual([hex(1), hex(1), hex(10)])
		expect(stats).toMatchObject({
			"duplicate-value-superseded": 1,
			"text-language-dropped": 1,
			"values-set": 4,
			"entity-predates-window": 1,
			unset: 1,
			deleteRelation: 1,
			"deleteEntity-predates-window": 1,
			"skip:op-somethingNew": 1,
		})
	})

	it("is replay-safe against what is already known", () => {
		const {ctx} = context()
		translateOps(ops, ctx)
		const second = translateOps(ops, ctx)
		expect(second.intents.some((i) => i.includes("create_entity"))).toBe(false)
		expect(second.stats["create-of-known-entity"]).toBe(1)
		expect(second.stats["create-of-known-relation"]).toBe(1)
	})
})

describe("zonDoc", () => {
	it("wraps intents, carrying the founding key only when given", () => {
		expect(zonDoc("s", "k", ['.{ .create_entity = "e" }'])).toBe('.{ .space = "s", .key = "k", .intents = .{\n .{ .create_entity = "e" },\n} }\n')
		expect(zonDoc("s", null, ["x"])).toBe('.{ .space = "s", .intents = .{\n x,\n} }\n')
	})
})
