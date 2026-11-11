import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow, type GeoVersionRow} from "../card"
import {type GeoDbCardClient, GeoDbError} from "../client"
import {
	__resetEntityAtVersionShadowMetricsForTests,
	compareNames,
	compareSnapshot,
	createEntityAtVersionShadow,
	type EntityAtVersionShadowConfig,
	PROBE_COOLDOWN_MS,
	readEntityAtVersionShadowConfig,
	renderEntityAtVersionShadowMetrics,
	type ServedSnapshot,
} from "../entityAtVersionShadow"

const ENTITY = "11111111111141118111111111111111"
const SPACE = "22222222222242228222222222222222"
const GEO_SPACE = "55555555555545558555555555555555"
const PROP = "33333333333343338333333333333333"
const TYPE = "44444444444444448444444444444444"
const TARGET = "66666666666646668666666666666666"
const OTHER = "77777777777747778777777777777777"
const NAME_PROP = "a126ca53-0c8e-48d5-b888-82c734c38935"
const EDIT = "00000000000040008000000000000001"
const OLD_EDIT = "00000000000040008000000000000002"

const config: EntityAtVersionShadowConfig = {enabled: true, sampleEvery: 1, maxPerMinute: 100, maxInFlight: 4}

const versionRow = (editId: string, over: Partial<GeoVersionRow> = {}): GeoVersionRow => ({
	space: GEO_SPACE,
	seq: 3,
	timestampNs: "1700000000000000000",
	editId,
	authors: [],
	total: 1,
	...over,
})

const text = (value: string): GeoCardRow => ({prop: conformingId("entity", PROP), raw: value})
const rel = (to: string): GeoCardRow => ({prop: conformingId("entity", TYPE), raw: conformingId("entity", to)})

const snapshot = (over: Partial<ServedSnapshot> = {}): ServedSnapshot => ({
	entityId: ENTITY,
	editId: EDIT,
	spaceId: SPACE,
	values: [{propertyId: PROP, text: "hello"}],
	relations: [{typeId: TYPE, toEntityId: TARGET, toSpaceId: SPACE}],
	...over,
})

const count = (name: string, labels: string) => {
	const m = renderEntityAtVersionShadowMetrics().match(new RegExp(`${name}\\{${labels}\\} (\\d+)`))
	return Number(m?.[1])
}
const result = (route: string, r: string) =>
	count("gaia_api_entity_at_version_shadow_total", `route="${route}",result="${r}"`)
const mismatch = (route: string, f: string) =>
	count("gaia_api_entity_at_version_shadow_mismatch_total", `route="${route}",field="${f}"`)
const unsupported = (route: string, r: string) =>
	count("gaia_api_entity_at_version_shadow_unsupported_total", `route="${route}",reason="${r}"`)
const skipped = (r: string) => count("gaia_api_entity_at_version_shadow_skipped_total", `reason="${r}"`)

const settle = () => new Promise((r) => setTimeout(r, 0))

type Reads = {
	versions?: GeoVersionRow[] | Error
	card?: GeoCardRow[] | Error
	cardAt?: GeoCardRow[] | Error
	cards?: GeoCardRow[] | Error
}
const client = (reads: Reads) => {
	const pick = <T>(v: T | Error | undefined, fallback: T): Promise<T> =>
		v instanceof Error ? Promise.reject(v) : Promise.resolve(v ?? fallback)
	return {
		readCard: vi.fn(() => pick(reads.card, [])),
		readCardSpaces: vi.fn(() => pick(reads.card, [])),
		readCardAt: vi.fn(() => pick(reads.cardAt, [])),
		readCards: vi.fn(() => pick(reads.cards, [])),
		readEntityVersions: vi.fn(() => pick(reads.versions, [])),
	} as unknown as GeoDbCardClient & {
		readCard: ReturnType<typeof vi.fn>
		readCardSpaces: ReturnType<typeof vi.fn>
		readCardAt: ReturnType<typeof vi.fn>
		readCards: ReturnType<typeof vi.fn>
		readEntityVersions: ReturnType<typeof vi.fn>
	}
}

const make = (reads: Reads, over: Partial<EntityAtVersionShadowConfig> = {}, now?: () => number) => {
	const geoDb = client(reads)
	const log = {warn: vi.fn()}
	const shadow = createEntityAtVersionShadow({
		config: {...config, ...over},
		geoDb,
		geoSpaceOf: async (id) => (id === SPACE ? GEO_SPACE : null),
		log,
		...(now ? {now} : {}),
	})
	return {geoDb, log, shadow}
}

beforeEach(() => __resetEntityAtVersionShadowMetricsForTests())

describe("readEntityAtVersionShadowConfig", () => {
	it("is off by default and clamps the knobs", () => {
		expect(readEntityAtVersionShadowConfig({})).toEqual({
			enabled: false,
			sampleEvery: 10,
			maxPerMinute: 30,
			maxInFlight: 2,
		})
		expect(
			readEntityAtVersionShadowConfig({
				GEODB_ENTITY_AT_VERSION_SHADOW: "on",
				GEODB_ENTITY_AT_VERSION_SHADOW_SAMPLE_EVERY: "0",
				GEODB_ENTITY_AT_VERSION_SHADOW_MAX_PER_MINUTE: "999999",
				GEODB_ENTITY_AT_VERSION_SHADOW_MAX_IN_FLIGHT: "3",
			}),
		).toEqual({enabled: true, sampleEvery: 10, maxPerMinute: 6000, maxInFlight: 3})
	})
})

describe("metrics", () => {
	it("export nothing until mounted, then every series at zero", () => {
		expect(renderEntityAtVersionShadowMetrics()).toBe("")
		make({})
		const out = renderEntityAtVersionShadowMetrics()
		expect(out).toContain('gaia_api_entity_at_version_shadow_total{route="snapshot",result="match"} 0')
		expect(out).toContain(
			'gaia_api_entity_at_version_shadow_unsupported_total{route="enrich_names",reason="not_head"} 0',
		)
		expect(out).toContain('gaia_api_entity_at_version_shadow_skipped_total{reason="rate"} 0')
	})
})

describe("compareSnapshot", () => {
	it("agrees on equal values and relations, ignoring block relations and empty text", () => {
		const card = [text("hello"), rel(TARGET)]
		expect(compareSnapshot(snapshot(), card)).toEqual([])
		expect(
			compareSnapshot(
				snapshot({
					values: [
						{propertyId: PROP, text: "hello"},
						{propertyId: OTHER, text: ""},
					],
				}),
				card,
			),
		).toEqual([])
	})

	it("names what differs on each side", () => {
		expect(compareSnapshot(snapshot(), [rel(TARGET)])).toEqual(["only_postgres"])
		expect(compareSnapshot(snapshot(), [text("hello"), rel(TARGET), text("extra")])).toEqual(["only_geo_db"])
		expect(compareSnapshot(snapshot(), [text("other"), rel(TARGET)])).toEqual(["only_postgres", "only_geo_db"])
	})
})

describe("compareNames", () => {
	const nameRow = (id: string, raw: string): GeoCardRow => ({
		prop: conformingId("entity", NAME_PROP),
		raw,
		entity: conformingId("entity", id),
	})
	it("sorts ids into ok, wrong and elsewhere", () => {
		const names = new Map([
			[PROP, "a"],
			[TYPE, "b"],
			[TARGET, "c"],
		])
		expect(compareNames(names, [nameRow(PROP, "a"), nameRow(TYPE, "zzz")])).toEqual({ok: 1, wrong: 1, elsewhere: 1})
	})
})

describe("snapshot twin", () => {
	it("matches when the edit is the newest version and the card agrees", async () => {
		const {shadow, geoDb} = make({versions: [versionRow(EDIT)], card: [text("hello"), rel(TARGET)]})
		shadow.snapshot(snapshot())
		await settle()
		expect(result("snapshot", "match")).toBe(1)
		expect(geoDb.readCard).toHaveBeenCalledTimes(1)
	})

	it("matches an unscoped read across the log's Spaces", async () => {
		const {shadow, geoDb} = make({versions: [versionRow(EDIT)], card: [text("hello"), rel(TARGET)]})
		shadow.snapshot(snapshot({spaceId: undefined}))
		await settle()
		expect(result("snapshot", "match")).toBe(1)
		expect(geoDb.readCardSpaces).toHaveBeenCalledTimes(1)
	})

	it("counts a mismatch by field and logs the request identity once per mismatch", async () => {
		const {shadow, log} = make({versions: [versionRow(EDIT)], card: [text("other"), rel(TARGET)]})
		shadow.snapshot(snapshot())
		await settle()
		expect(result("snapshot", "mismatch")).toBe(1)
		expect(mismatch("snapshot", "only_postgres")).toBe(1)
		expect(mismatch("snapshot", "only_geo_db")).toBe(1)
		expect(log.warn).toHaveBeenCalledTimes(1)
	})

	it("counts an older edit as unsupported not_head and reads no card", async () => {
		const {shadow, geoDb} = make({versions: [versionRow(EDIT)]})
		shadow.snapshot(snapshot({editId: OLD_EDIT}))
		await settle()
		expect(unsupported("snapshot", "not_head")).toBe(1)
		expect(result("snapshot", "unsupported")).toBe(1)
		expect(geoDb.readCard).not.toHaveBeenCalled()
	})

	it("reads an older edit of a scoped request at its version and compares it", async () => {
		const {shadow, geoDb} = make({
			versions: [versionRow(EDIT, {seq: 5}), versionRow(OLD_EDIT, {seq: 2})],
			cardAt: [text("hello"), rel(TARGET)],
		})
		shadow.snapshot(snapshot({editId: OLD_EDIT}))
		await settle()
		expect(geoDb.readCardAt).toHaveBeenCalledWith({
			geoSpaceId: GEO_SPACE,
			entityId: conformingId("entity", ENTITY),
			seq: 2,
		})
		expect(geoDb.readCard).not.toHaveBeenCalled()
		expect(result("snapshot", "match")).toBe(1)
	})

	it("counts a mismatch at an older version, with its seq in the log line", async () => {
		const {shadow, log} = make({
			versions: [versionRow(EDIT, {seq: 5}), versionRow(OLD_EDIT, {seq: 2})],
			cardAt: [text("other"), rel(TARGET)],
		})
		shadow.snapshot(snapshot({editId: OLD_EDIT}))
		await settle()
		expect(result("snapshot", "mismatch")).toBe(1)
		expect(log.warn).toHaveBeenCalledWith("entity-at-version shadow mismatch", expect.objectContaining({seq: 2}))
	})

	it("keeps an unscoped older edit, and a client without readCardAt, as not_head", async () => {
		const versions = [versionRow(EDIT, {seq: 5}), versionRow(OLD_EDIT, {seq: 2})]
		const a = make({versions})
		a.shadow.snapshot(snapshot({editId: OLD_EDIT, spaceId: undefined}))
		await settle()
		expect(unsupported("snapshot", "not_head")).toBe(1)
		expect(a.geoDb.readCardAt).not.toHaveBeenCalled()
		const b = make({versions})
		delete (b.geoDb as {readCardAt?: unknown}).readCardAt
		b.shadow.snapshot(snapshot({editId: OLD_EDIT}))
		await settle()
		expect(unsupported("snapshot", "not_head")).toBe(2)
	})

	it("compares dashed edit ids", async () => {
		const dashed = "00000000-0000-4000-8000-000000000001"
		const {shadow} = make({versions: [versionRow(dashed)], card: [text("hello"), rel(TARGET)]})
		shadow.snapshot(snapshot())
		await settle()
		expect(result("snapshot", "match")).toBe(1)
	})

	it("is absent_ok when neither side holds anything, else not_head", async () => {
		const a = make({versions: []})
		a.shadow.snapshot(snapshot({values: [], relations: []}))
		await settle()
		expect(result("snapshot", "absent_ok")).toBe(1)
		a.shadow.snapshot(snapshot())
		await settle()
		expect(unsupported("snapshot", "not_head")).toBe(1)
	})

	it("counts a Space geo-db does not hold as unmapped_space", async () => {
		const {shadow} = make({})
		shadow.snapshot(snapshot({spaceId: OTHER}))
		await settle()
		expect(unsupported("snapshot", "unmapped_space")).toBe(1)
	})

	it("counts a failed read, an overloaded store as busy, and an old client as unsupported with a cooldown", async () => {
		const failed = make({versions: new GeoDbError("timeout", "slow")})
		failed.shadow.snapshot(snapshot())
		await settle()
		expect(result("snapshot", "failed")).toBe(1)

		__resetEntityAtVersionShadowMetricsForTests()
		const busy = make({versions: new GeoDbError("overloaded", "full")})
		busy.shadow.snapshot(snapshot())
		await settle()
		expect(unsupported("snapshot", "busy")).toBe(1)

		__resetEntityAtVersionShadowMetricsForTests()
		let t = 1_000
		const old = make(
			{versions: new GeoDbError("client_failed", "refused: DefiniteRefusal UnknownOpcode")},
			{},
			() => t,
		)
		old.shadow.snapshot(snapshot())
		await settle()
		expect(unsupported("snapshot", "server")).toBe(1)
		old.shadow.snapshot(snapshot())
		await settle()
		expect(old.geoDb.readEntityVersions).toHaveBeenCalledTimes(1)
		expect(unsupported("snapshot", "server")).toBe(2)
		t += PROBE_COOLDOWN_MS + 1
		old.shadow.snapshot(snapshot())
		await settle()
		expect(old.geoDb.readEntityVersions).toHaveBeenCalledTimes(2)
	})
})

describe("enrich_names twin", () => {
	const nameRow = (id: string, raw: string): GeoCardRow => ({
		prop: conformingId("entity", NAME_PROP),
		raw,
		entity: conformingId("entity", id),
	})
	const served = (names: [string, string][]) => ({spaceId: SPACE, names: new Map(names)})

	it("matches when geo-db's names equal the served ones", async () => {
		const {shadow, geoDb} = make({cards: [nameRow(PROP, "a"), nameRow(TYPE, "b")]})
		shadow.names(
			served([
				[PROP, "a"],
				[TYPE, "b"],
			]),
		)
		await settle()
		expect(result("enrich_names", "match")).toBe(1)
		expect(geoDb.readCards).toHaveBeenCalledTimes(1)
	})

	it("counts a different name as a name mismatch", async () => {
		const {shadow} = make({cards: [nameRow(PROP, "zzz")]})
		shadow.names(served([[PROP, "a"]]))
		await settle()
		expect(result("enrich_names", "mismatch")).toBe(1)
		expect(mismatch("enrich_names", "name")).toBe(1)
	})

	it("counts names held elsewhere as unsupported name_elsewhere, but still matches when another id compared", async () => {
		const a = make({cards: []})
		a.shadow.names(served([[PROP, "a"]]))
		await settle()
		expect(unsupported("enrich_names", "name_elsewhere")).toBe(1)

		__resetEntityAtVersionShadowMetricsForTests()
		const b = make({cards: [nameRow(PROP, "a")]})
		b.shadow.names(
			served([
				[PROP, "a"],
				[TYPE, "b"],
			]),
		)
		await settle()
		expect(result("enrich_names", "match")).toBe(1)
	})

	it("is absent_ok when nothing was named and unmapped_space for an unknown Space", async () => {
		const {shadow} = make({})
		shadow.names(served([]))
		shadow.names({spaceId: OTHER, names: new Map([[PROP, "a"]])})
		await settle()
		expect(result("enrich_names", "absent_ok")).toBe(1)
		expect(unsupported("enrich_names", "unmapped_space")).toBe(1)
	})
})

describe("sampling and limits", () => {
	it("does nothing when the flag is off", async () => {
		const {shadow, geoDb} = make({}, {enabled: false})
		shadow.snapshot(snapshot())
		await settle()
		expect(geoDb.readEntityVersions).not.toHaveBeenCalled()
	})

	it("compares one in sampleEvery and counts the rest as sample skips", async () => {
		const {shadow, geoDb} = make(
			{versions: [versionRow(EDIT)], card: [text("hello"), rel(TARGET)]},
			{sampleEvery: 3},
		)
		for (let i = 0; i < 6; i++) shadow.snapshot(snapshot())
		await settle()
		expect(geoDb.readEntityVersions).toHaveBeenCalledTimes(2)
		expect(skipped("sample")).toBe(4)
	})

	it("stops at the per-minute cap and resumes in the next minute", async () => {
		let t = 0
		const {shadow, geoDb} = make(
			{versions: [versionRow(EDIT)], card: [text("hello"), rel(TARGET)]},
			{maxPerMinute: 2},
			() => t,
		)
		for (let i = 0; i < 4; i++) shadow.snapshot(snapshot())
		await settle()
		expect(geoDb.readEntityVersions).toHaveBeenCalledTimes(2)
		expect(skipped("rate")).toBe(2)
		t += 60_001
		shadow.snapshot(snapshot())
		await settle()
		expect(geoDb.readEntityVersions).toHaveBeenCalledTimes(3)
	})

	it("drops a comparison past the in-flight bound as busy", () => {
		const {shadow} = make({}, {maxInFlight: 1})
		shadow.snapshot(snapshot())
		shadow.snapshot(snapshot())
		expect(unsupported("snapshot", "busy")).toBe(1)
	})
})
