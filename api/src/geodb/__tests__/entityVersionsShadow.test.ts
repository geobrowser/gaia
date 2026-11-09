import {beforeEach, describe, expect, it, vi} from "vitest"
import {conformingId, entityVersionRowsFromGeoDbOutput, type GeoVersionRow} from "../card"
import {type GeoDbCardClient, GeoDbError} from "../client"
import {
	__resetEntityVersionsShadowMetricsForTests,
	compareVersions,
	createEntityVersionsShadow,
	PROBE_COOLDOWN_MS,
	readEntityVersionsShadowConfig,
	renderEntityVersionsShadowMetrics,
	type ServedVersions,
} from "../entityVersionsShadow"

const ENTITY = "11111111111141118111111111111111"
const SPACE = "22222222222242228222222222222222"
const EDIT = (n: number) => `0000000000004000800000000000000${n}`
const AUTHOR = "33333333333343338333333333333333"

const geoRow = (n: number, over: Partial<GeoVersionRow> = {}, total = 3): GeoVersionRow => ({
	space: SPACE,
	seq: 10 - n,
	timestampNs: String(1_700_000_000_000_000_000 - n),
	editId: EDIT(n),
	authors: [AUTHOR],
	total,
	...over,
})

const served = (over: Partial<ServedVersions> = {}, ids = [1, 2, 3]): ServedVersions => ({
	entityId: ENTITY,
	limit: 50,
	offset: 0,
	versions: ids.map((n) => ({editId: EDIT(n), createdById: AUTHOR})),
	...over,
})

const count = (name: string, label: string, value: string) => {
	const m = renderEntityVersionsShadowMetrics().match(new RegExp(`${name}\\{${label}="${value}"\\} (\\d+)`))
	return Number(m?.[1])
}
const result = (r: string) => count("gaia_api_versions_shadow_total", "result", r)
const mismatch = (f: string) => count("gaia_api_versions_shadow_mismatch_total", "field", f)
const unsupported = (r: string) => count("gaia_api_versions_shadow_unsupported_total", "reason", r)
const skipped = (r: string) => count("gaia_api_versions_shadow_skipped_total", "reason", r)

const settle = () => new Promise((r) => setTimeout(r, 0))

describe("entityVersionRowsFromGeoDbOutput", () => {
	const row = (n: number, authors: string) =>
		[SPACE, 9, "ab".repeat(16), `1700000000000000${n}`, EDIT(n), "-", authors, "-", "cd".repeat(8)].join("\t")

	it("reads rows, their authors and the total", () => {
		const out = `${row(1, `[${AUTHOR},${ENTITY}]`)}\n${row(2, "[]")}\ntotal: 7\n`
		const rows = entityVersionRowsFromGeoDbOutput(out)
		expect(rows).toHaveLength(2)
		expect(rows[0]).toMatchObject({space: SPACE, seq: 9, editId: EDIT(1), authors: [AUTHOR, ENTITY], total: 7})
		expect(rows[1]?.authors).toEqual([])
	})

	it("reads an empty log", () => {
		expect(entityVersionRowsFromGeoDbOutput("total: 0\n")).toEqual([])
	})

	it("rejects a missing total and a malformed row", () => {
		expect(() => entityVersionRowsFromGeoDbOutput(`${row(1, "[]")}\n`)).toThrow(/no total/)
		expect(() => entityVersionRowsFromGeoDbOutput("nope\ttoo short\ntotal: 1\n")).toThrow(/malformed/)
	})
})

describe("compareVersions", () => {
	it("agrees on the same count, order and author", () => {
		expect(compareVersions(served(), [geoRow(1), geoRow(2), geoRow(3)])).toEqual([])
	})

	it("flags a different count", () => {
		expect(compareVersions(served(), [geoRow(1, {}, 2), geoRow(2, {}, 2)])).toContain("count")
	})

	it("does not flag a count a full served page cannot show", () => {
		// 3 served of limit 3, geo holds 9: the page is full, so the totals may differ.
		expect(compareVersions(served({limit: 3}), [geoRow(1, {}, 9), geoRow(2, {}, 9), geoRow(3, {}, 9)])).toEqual([])
	})

	it("flags the same edits in another order", () => {
		expect(compareVersions(served(), [geoRow(2), geoRow(1), geoRow(3)])).toEqual(["order"])
	})

	it("flags different edits as edit_ids, not order", () => {
		expect(compareVersions(served(), [geoRow(1), geoRow(2), geoRow(4)])).toEqual(["edit_ids"])
	})

	it("flags a different author, and an author geo-db does not record", () => {
		expect(compareVersions(served(), [geoRow(1, {authors: [ENTITY]}), geoRow(2), geoRow(3)])).toEqual(["author"])
		expect(compareVersions(served(), [geoRow(1, {authors: []}), geoRow(2), geoRow(3)])).toEqual([
			"author_unrecorded",
		])
	})

	it("treats no author on both sides as agreement and geo-db's authors against none as a mismatch", () => {
		const none = served({}, [1]).versions.map((v) => ({...v, createdById: null}))
		expect(compareVersions(served({versions: none}), [geoRow(1, {authors: []}, 1)])).toEqual([])
		expect(compareVersions(served({versions: none}), [geoRow(1, {}, 1)])).toEqual(["author"])
	})

	it("accepts an author geo-db holds under its conformed id", () => {
		const legacy = "44444444444444444444444444444444"
		const page = served({versions: [{editId: EDIT(1), createdById: legacy}]})
		expect(compareVersions(page, [geoRow(1, {authors: [conformingId("entity", legacy)]}, 1)])).toEqual([])
	})

	it("matches two empty logs", () => {
		expect(compareVersions(served({}, []), [])).toEqual([])
	})
})

describe("createEntityVersionsShadow", () => {
	beforeEach(() => __resetEntityVersionsShadowMetricsForTests())

	const setup = (
		reader: GeoDbCardClient["readEntityVersions"],
		env: Record<string, string> = {},
		clock = {t: 1_000_000},
	) => {
		const warn = vi.fn()
		const geoDb = {readEntityVersions: reader} as unknown as GeoDbCardClient
		const shadow = createEntityVersionsShadow({
			config: readEntityVersionsShadowConfig({
				GEODB_VERSIONS_SHADOW: "on",
				GEODB_VERSIONS_SHADOW_SAMPLE_EVERY: "1",
				...env,
			}),
			geoDb,
			log: {warn},
			now: () => clock.t,
		})
		return {shadow, warn, clock}
	}

	it("exports nothing until mounted, then every series at zero", () => {
		expect(renderEntityVersionsShadowMetrics()).toBe("")
		setup(async () => [])
		expect(result("match")).toBe(0)
		expect(unsupported("server")).toBe(0)
	})

	it("does nothing when the flag is off", async () => {
		const read = vi.fn(async () => [geoRow(1)])
		const {shadow} = setup(read, {GEODB_VERSIONS_SHADOW: "0"})
		shadow.versions(served())
		await settle()
		expect(read).not.toHaveBeenCalled()
	})

	it("counts a match and a mismatch, logging counts only", async () => {
		const read = vi.fn(async () => [geoRow(1), geoRow(2), geoRow(3)])
		const {shadow, warn} = setup(read)
		shadow.versions(served())
		await settle()
		expect(result("match")).toBe(1)
		expect(read).toHaveBeenCalledWith({entityId: ENTITY, first: 50})
		shadow.versions(served({}, [1, 2]))
		await settle()
		expect(result("mismatch")).toBe(1)
		expect(mismatch("edit_ids")).toBe(0)
		expect(mismatch("count")).toBe(1)
		expect(JSON.stringify(warn.mock.calls)).not.toContain(ENTITY)
	})

	it("does not read a scoped or offset request", async () => {
		const read = vi.fn(async () => [])
		const {shadow} = setup(read)
		shadow.versions(served({spaceId: SPACE}))
		shadow.versions(served({offset: 50}))
		await settle()
		expect(read).not.toHaveBeenCalled()
		expect(unsupported("scoped")).toBe(1)
		expect(unsupported("paged")).toBe(1)
	})

	it("compares one request in N", async () => {
		const read = vi.fn(async () => [geoRow(1), geoRow(2), geoRow(3)])
		const {shadow} = setup(read, {GEODB_VERSIONS_SHADOW_SAMPLE_EVERY: "4"})
		for (let i = 0; i < 8; i++) shadow.versions(served())
		await settle()
		expect(read).toHaveBeenCalledTimes(2)
		expect(skipped("sample")).toBe(6)
	})

	it("holds to the per-minute cap, then resumes in the next minute", async () => {
		const read = vi.fn(async () => [geoRow(1), geoRow(2), geoRow(3)])
		const {shadow, clock} = setup(read, {GEODB_VERSIONS_SHADOW_MAX_PER_MINUTE: "2"})
		for (let i = 0; i < 5; i++) {
			shadow.versions(served())
			await settle()
		}
		expect(read).toHaveBeenCalledTimes(2)
		expect(skipped("rate")).toBe(3)
		clock.t += 60_000
		shadow.versions(served())
		await settle()
		expect(read).toHaveBeenCalledTimes(3)
	})

	it("drops a comparison past the in-flight bound", async () => {
		let release: (rows: GeoVersionRow[]) => void = () => {}
		const read = vi.fn(
			() =>
				new Promise<GeoVersionRow[]>((r) => {
					release = r
				}),
		)
		const {shadow} = setup(read, {GEODB_VERSIONS_SHADOW_MAX_IN_FLIGHT: "1"})
		shadow.versions(served())
		shadow.versions(served())
		expect(unsupported("busy")).toBe(1)
		release([geoRow(1), geoRow(2), geoRow(3)])
		await settle()
		expect(result("match")).toBe(1)
	})

	it("returns before the read finishes: it never waits on geo-db", () => {
		const read = vi.fn(() => new Promise<GeoVersionRow[]>(() => {}))
		const {shadow} = setup(read)
		expect(shadow.versions(served())).toBeUndefined()
		expect(read).toHaveBeenCalledTimes(1)
	})

	it("counts a failed read and swallows a thrown reader", async () => {
		const {shadow} = setup(async () => {
			throw new GeoDbError("timeout", "slow")
		})
		shadow.versions(served())
		await settle()
		expect(result("failed")).toBe(1)
		const {shadow: odd, warn} = setup(async () => {
			throw new Error("boom")
		})
		odd.versions(served())
		await settle()
		expect(result("failed")).toBe(2)
		expect(warn).toHaveBeenCalledWith("versions shadow failed", {error: "Error: boom"})
	})

	it("reports a client that predates the line, then leaves it alone until the cooldown ends", async () => {
		const read = vi.fn(async () => {
			throw new GeoDbError("client_failed", "want <space uuid>[,<space uuid>]...|*\\t<geoql>")
		})
		const {shadow, clock} = setup(read)
		shadow.versions(served())
		await settle()
		shadow.versions(served())
		shadow.versions(served())
		await settle()
		expect(read).toHaveBeenCalledTimes(1)
		expect(unsupported("client")).toBe(3)
		expect(result("unsupported")).toBe(3)
		clock.t += PROBE_COOLDOWN_MS
		shadow.versions(served())
		await settle()
		expect(read).toHaveBeenCalledTimes(2)
	})

	it("reports a node before wire revision 12 and a client with no such method", async () => {
		const {shadow} = setup(async () => {
			throw new GeoDbError("client_failed", "refused: DefiniteRefusal UnknownOpcode")
		})
		shadow.versions(served())
		await settle()
		expect(unsupported("server")).toBe(1)
		const {shadow: bare} = setup(undefined, {})
		bare.versions(served())
		await settle()
		expect(unsupported("client")).toBe(1)
	})
})
