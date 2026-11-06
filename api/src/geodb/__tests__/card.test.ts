import {readFileSync} from "node:fs"
import {describe, expect, it} from "vitest"
import {
	conformingId,
	DERIVED_PROPERTIES,
	diffCards,
	isEmptyDiff,
	MEMBERSHIP_PROPERTIES,
	pointNorm,
	rowsFromGeoDbOutput,
	statementsFromPostgres,
	type ValueRow,
} from "../card"
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
				value({}), // nothing set (a bytes row): unmappable
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

describe("rowsFromGeoDbOutput with names", () => {
	const OTHER = "ffffffffffffffffffffffffffffffff"
	// Columns: property, value, statement, entity; [claim property, claim value]; property name, target name; weight.
	const plain = `${PROP}\tv\ts1\t${ENTITY}\tProp\\tName\tTarget\\nName\t1\n`

	it("reads the two names cells before the weight, unescaped", () => {
		expect(rowsFromGeoDbOutput(plain, ENTITY, {names: true})).toEqual([
			{prop: PROP, raw: "v", propName: "Prop\tName", targetName: "Target\nName"},
		])
	})

	it("reads `-` as no name", () => {
		const out = `${PROP}\tv\ts1\t${ENTITY}\t-\t-\t1\n`
		expect(rowsFromGeoDbOutput(out, ENTITY, {names: true})).toEqual([
			{prop: PROP, raw: "v", propName: null, targetName: null},
		])
	})

	it("skips the two claim columns when claims were asked", () => {
		const out = `${PROP}\tv\ts1\t${ENTITY}\t${PROP}\tclaim\tP\tT\t1\n${PROP}\tw\ts2\t${ENTITY}\t-\t-\t-\tT2\t1\n`
		expect(rowsFromGeoDbOutput(out, ENTITY, {names: true, claims: true})).toEqual([
			{prop: PROP, raw: "v", propName: "P", targetName: "T"},
			{prop: PROP, raw: "w", propName: null, targetName: "T2"},
		])
	})

	it("keeps only the entity's rows, and leaves the names absent from a row too short to hold them", () => {
		const out = `${PROP}\tv\ts1\t${OTHER}\tP\tT\t1\n${PROP}\tshort\ts2\t${ENTITY}\t1\n`
		expect(rowsFromGeoDbOutput(out, ENTITY, {names: true})).toEqual([{prop: PROP, raw: "short"}])
	})

	it("without the option, ignores the cells after the entity exactly as before", () => {
		expect(rowsFromGeoDbOutput(plain, ENTITY)).toEqual([{prop: PROP, raw: "v"}])
		expect(rowsFromGeoDbOutput(plain, ENTITY, {})).toEqual([{prop: PROP, raw: "v"}])
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

	it("with skipEmptyText skips a Postgres empty text geo-db lacks, and still matches one it holds", () => {
		const empty = statementsFromPostgres([value({text: ""})], [])
		const lacking = diffCards(empty, [], {skipEmptyText: true})
		expect(isEmptyDiff(lacking)).toBe(true)
		expect(lacking.skipped).toBe(1)
		expect(isEmptyDiff(diffCards(empty, [{prop: PROP, raw: ""}], {skipEmptyText: true}))).toBe(true)
		// Strict (the default): the same missing "" is a gap, and a held one still matches.
		expect(diffCards(empty, []).onlyPostgres).toEqual([{prop: PROP, kind: "text", norm: ""}])
		expect(isEmptyDiff(diffCards(empty, [{prop: PROP, raw: ""}]))).toBe(true)
		// Only an empty text is forgiven: a non-empty one geo-db lacks is still a gap.
		expect(diffCards(statementsFromPostgres([value({text: "x"})], []), []).onlyPostgres).toHaveLength(1)
	})

	it("with skipEmptyText does not count a geo-db empty text Postgres lacks as extra; strict does", () => {
		expect(
			isEmptyDiff(diffCards({statements: [], skipped: 0}, [{prop: PROP, raw: ""}], {skipEmptyText: true})),
		).toBe(true)
		expect(diffCards({statements: [], skipped: 0}, [{prop: PROP, raw: ""}]).onlyGeoDb).toHaveLength(1)
	})
})

describe("diffCards derived properties", () => {
	const SCORE = "85a4668a42fa4f488969c0a9de0c294b"
	const RANK_RELATION = "890deffb384349fa826974000e3dcef6"

	it("counts a derived-only Postgres card against an empty geo-db as a match, all skipped", () => {
		const pg = statementsFromPostgres(
			[value({property_id: SCORE, integer: "83"}), value({property_id: SCORE, integer: "5"})],
			[{type_id: RANK_RELATION, to_entity_id: ENTITY, to_space_id: null}],
		)
		const diff = diffCards(pg, [])
		expect(isEmptyDiff(diff)).toBe(true)
		expect(diff.skipped).toBe(3)
	})

	it("skips ranking-indexer's Aggregated rankings relation, which no edit carries", () => {
		const AGGREGATED = "67651da611a942469ff4039aafbe9e43"
		const pg = statementsFromPostgres([], [{type_id: AGGREGATED, to_entity_id: ENTITY, to_space_id: null}])
		const diff = diffCards(pg, [])
		expect(isEmptyDiff(diff)).toBe(true)
		expect(diff.skipped).toBe(1)
	})

	it("still reports a real statement missing from geo-db, and never samples the derived one", () => {
		const pg = statementsFromPostgres([value({property_id: SCORE, integer: "83"}), value({text: "x"})], [])
		const diff = diffCards(pg, [])
		expect(diff.onlyPostgres).toEqual([{prop: PROP, kind: "text", norm: "x"}])
		expect(diff.onlyPostgres.some((s) => DERIVED_PROPERTIES.has(s.prop))).toBe(false)
		expect(diff.skipped).toBe(1)
	})

	it("does not report a derived row only geo-db holds", () => {
		const geo = [{prop: "103701ddcabe4a8e835b10345327b647", raw: "1.22"}]
		expect(isEmptyDiff(diffCards({statements: [], skipped: 0}, geo))).toBe(true)
	})

	it("does not report the membership rows only geo-db holds", () => {
		const geo = [...MEMBERSHIP_PROPERTIES].map((prop) => ({prop, raw: "2a1072ceefd74738a9bd6dac2adbf30d"}))
		expect(isEmptyDiff(diffCards({statements: [], skipped: 0}, geo))).toBe(true)
	})

	it("adds derived statements to the rows already skipped as unmappable", () => {
		const pg = statementsFromPostgres([value({}), value({property_id: SCORE, integer: "1"})], [])
		expect(diffCards(pg, []).skipped).toBe(2)
	})

	it("stays equal to the geodb-feeder DERIVED_PROPERTIES list", () => {
		const src = readFileSync(new URL("../../../../geodb-feeder/src/pgAdapter.ts", import.meta.url), "utf8")
		const block = src.match(/export const DERIVED_PROPERTIES = \[([\s\S]*?)\]/)?.[1] ?? ""
		const ids = [...block.matchAll(/"([0-9a-f-]{36})"/g)].map((m) => (m[1] as string).replaceAll("-", ""))
		expect(ids.length).toBeGreaterThan(0)
		expect(new Set(ids)).toEqual(new Set(DERIVED_PROPERTIES))
	})
})

describe("points", () => {
	it("maps the kg-indexer lat,lon form to geo-db's lon, lat form", () => {
		expect(pointNorm("52.402833333333,23.808333333333")).toBe("23.808333333, 52.402833333")
		expect(pointNorm("-33.5,151.25,12.5")).toBe("151.25, -33.5, 12.5")
		expect(pointNorm("1e-12,0")).toBe("0, 0")
		expect(pointNorm("nope")).toBeNull()
	})

	it("matches a Postgres point against the geo-db text row", () => {
		const {statements, skipped} = statementsFromPostgres([value({point: "52.402833333333,23.808333333333"})], [])
		const diff = diffCards({statements, skipped}, [{prop: PROP, raw: "23.808333333, 52.402833333"}])
		expect(diff.onlyPostgres).toEqual([])
		expect(diff.onlyGeoDb).toEqual([])
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

	it("reads the ordered-page shadow only when explicitly on", () => {
		expect(readGeoDbCardConfig({}).orderPick).toBe(false)
		expect(readGeoDbCardConfig({GEODB_ORDER_PICK_SHADOW: "true"}).orderPick).toBe(false)
		expect(readGeoDbCardConfig({GEODB_ORDER_PICK_SHADOW: " ON "}).orderPick).toBe(true)
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
