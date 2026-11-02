import {describe, expect, it} from "vitest"
import {conformingId, diffCards, isEmptyDiff, rowsFromGeoDbOutput, statementsFromPostgres, type ValueRow} from "../card"
import {parseMode, readGeoDbCardConfig} from "../config"

const PROP = "98012398829d48b083738f2589675894"
const ENTITY = "073ff16be7fe4591a984c325d629d92d"

const value = (over: Partial<ValueRow>): ValueRow => ({
	property_id: PROP,
	text: null,
	integer: null,
	float: null,
	decimal: null,
	boolean: null,
	datetime: null,
	date: null,
	...over,
})

describe("conformingId", () => {
	it("passes a conforming UUIDv4 through, undashed and lowercase", () => {
		expect(conformingId("entity", "073FF16B-E7FE-4591-A984-C325D629D92D")).toBe(ENTITY)
	})

	it("remaps other ids exactly as the shadow loader does", () => {
		expect(conformingId("entity", "0123456789abcdef0123456789abcdef")).toBe("a7f79f68ecd64763867f42bfc4387c95")
		expect(conformingId("space", "00000000-0000-0000-0000-000000000001")).toBe("e2c3812c809a4b56bd7bf07b987af9b2")
	})
})

describe("statementsFromPostgres", () => {
	it("maps values and relations and counts the rows geo-db cannot carry", () => {
		const {statements, skipped} = statementsFromPostgres(
			[
				value({text: "Noam Shazeer"}),
				value({integer: "366741"}),
				value({boolean: true}),
				value({datetime: "2024-05-01T10:00:00.5Z"}),
				value({date: "1996-02-14Z"}),
				value({}), // nothing set (a point or bytes row): unmappable
				value({text: "a", integer: "1"}), // ambiguous: unmappable
			],
			[{type_id: PROP, to_entity_id: ENTITY, to_space_id: null}],
		)
		expect(skipped).toBe(2)
		expect(statements.map((s) => [s.kind, s.norm])).toEqual([
			["text", "Noam Shazeer"],
			["num", "366741"],
			["text", "true"],
			["text", "instant:1714557600.500000000"],
			["text", "date:1996-02-14"],
			["text", ENTITY],
		])
	})
})

describe("rowsFromGeoDbOutput", () => {
	it("keeps the entity's own rows and unescapes text cells", () => {
		const out = `${PROP}\ta\\tb\\nc\\\\d\tstmt1\t${ENTITY}\t1\n${PROP}\tother\tstmt2\tffffffffffffffffffffffffffffffff\t1\n\n`
		expect(rowsFromGeoDbOutput(out, ENTITY)).toEqual([{prop: PROP, raw: "a\tb\nc\\d"}])
	})
})

describe("diffCards", () => {
	const pg = statementsFromPostgres([value({text: "x"}), value({integer: "7"})], [])

	it("matches as multisets, numbers by value", () => {
		const diff = diffCards(pg, [
			{prop: PROP, raw: "7.0"},
			{prop: PROP, raw: "x"},
		])
		expect(isEmptyDiff(diff)).toBe(true)
	})

	it("reports what is on one side only", () => {
		const diff = diffCards(pg, [
			{prop: PROP, raw: "x"},
			{prop: PROP, raw: "extra"},
		])
		expect(diff.onlyPostgres).toEqual([{prop: PROP, kind: "num", norm: "7"}])
		expect(diff.onlyGeoDb).toEqual([{prop: PROP, raw: "extra"}])
	})

	it("does not let one geo-db row satisfy two equal statements", () => {
		const twice = statementsFromPostgres([value({text: "x"}), value({text: "x"})], [])
		expect(diffCards(twice, [{prop: PROP, raw: "x"}]).onlyPostgres).toHaveLength(1)
	})
})

describe("config", () => {
	it("is off unless explicitly on or shadow", () => {
		expect(parseMode(undefined)).toBe("off")
		expect(parseMode("true")).toBe("off")
		expect(parseMode("ON")).toBe("on")
		expect(parseMode(" shadow ")).toBe("shadow")
		expect(readGeoDbCardConfig({}).mode).toBe("off")
	})

	it("defaults the timeout and rejects a bad one", () => {
		expect(readGeoDbCardConfig({}).timeoutMs).toBe(500)
		expect(readGeoDbCardConfig({GEODB_TIMEOUT_MS: "-5"}).timeoutMs).toBe(500)
		expect(readGeoDbCardConfig({GEODB_TIMEOUT_MS: "250"}).timeoutMs).toBe(250)
		expect(readGeoDbCardConfig({}).poolSize).toBe(4)
		expect(readGeoDbCardConfig({GEODB_POOL_SIZE: "0"}).poolSize).toBe(4)
		expect(readGeoDbCardConfig({GEODB_POOL_SIZE: "8"}).poolSize).toBe(8)
		expect(readGeoDbCardConfig({GEODB_POOL_SIZE: "9999"}).poolSize).toBe(64)
	})
})
