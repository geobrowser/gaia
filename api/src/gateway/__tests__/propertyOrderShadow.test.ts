import {describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow} from "../../geodb/card"
import type {PropertyOrder} from "../lightShadow"
import {checkPropertyOrder, compareKeys, judgePage, judgePairs, keyOfRow} from "../propertyOrderShadow"

const PROP = "00000000-9962-4962-9962-0000000000bb"
const TYPES = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"
const SPACE = "c9f267dcb0d270718c2a3c45a64afd32"
const T1 = "6e3503fab974460ea3dbab8af9a41427"
const eid = (n: number) => `${n.toString(16).padStart(8, "0")}-0000-4000-8000-000000000000`

const order = (over: Partial<PropertyOrder> = {}): PropertyOrder => ({
	propertyId: PROP,
	dataType: "integer",
	direction: "desc",
	spaceIds: [SPACE],
	typeIds: null,
	...over,
})
const row = (n: number, raw: string, space?: string): GeoCardRow => ({
	prop: conformingId("entity", PROP),
	raw,
	entity: conformingId("entity", eid(n)),
	...(space ? {space} : {}),
})
const ids = (...ns: number[]) => ns.map(eid)
const verdict = (o: PropertyOrder, rows: GeoCardRow[], page: string[]) => judgePage(o, page, rows, "geo")

describe("keyOfRow and compareKeys", () => {
	it("orders numbers exactly, including negatives and decimals", () => {
		const k = (s: string) => keyOfRow("decimal", s)!
		expect(compareKeys(k("10"), k("9"))).toBe(1)
		expect(compareKeys(k("-1.5"), k("-1.25"))).toBe(-1)
		expect(compareKeys(k("2.50"), k("2.5"))).toBe(0)
		expect(compareKeys(k("-0"), k("0"))).toBe(0)
		expect(compareKeys(k("12345678901234567890"), k("12345678901234567891"))).toBe(-1)
	})

	it("orders instants, days and booleans; refuses text order but knows equal text", () => {
		const t = (s: string) => keyOfRow("datetime", s)!
		expect(compareKeys(t("instant:10.000000001"), t("instant:10.000000000"))).toBe(1)
		const d = (s: string) => keyOfRow("date", s)!
		expect(compareKeys(d("date:2024-02-01"), d("date:2024-10-01"))).toBe(-1)
		const b = (s: string) => keyOfRow("boolean", s)!
		expect(compareKeys(b("true"), b("false"))).toBe(1)
		const x = (s: string) => keyOfRow("text", s)!
		expect(compareKeys(x("a"), x("a"))).toBe(0)
		expect(compareKeys(x("a"), x("b"))).toBeNull()
	})

	it("drops rows that are no value of the type", () => {
		expect(keyOfRow("integer", "abc")).toBeNull()
		expect(keyOfRow("integer", "")).toBeNull()
		expect(keyOfRow("text", "   ")).toBeNull()
		expect(keyOfRow("datetime", "2024-01-01")).toBeNull()
		expect(keyOfRow("bogus", "1")).toBeNull()
	})
})

describe("judgePairs", () => {
	const n = (s: string) => keyOfRow("integer", s)
	it("accepts a descending page and breaks ties by ascending id", () => {
		const v = judgePairs(["a", "b", "c"], [n("5"), n("5"), n("1")], "desc")
		expect(v).toEqual({checked: 2, undecided: 0, firstBreak: null})
	})
	it("flags a tie broken the wrong way", () => {
		const v = judgePairs(["b", "a"], [n("5"), n("5")], "desc")
		expect(v.firstBreak).toEqual({index: 0, reason: "tie"})
	})
	it("flags a key out of order in either direction", () => {
		expect(judgePairs(["a", "b"], [n("1"), n("2")], "desc").firstBreak).toEqual({index: 0, reason: "key"})
		expect(judgePairs(["a", "b"], [n("2"), n("1")], "asc").firstBreak).toEqual({index: 0, reason: "key"})
	})
	it("counts a pair with an unknown key undecided", () => {
		expect(judgePairs(["a", "b", "c"], [n("3"), null, n("1")], "desc")).toEqual({
			checked: 0,
			undecided: 2,
			firstBreak: null,
		})
	})
})

describe("judgePage", () => {
	it("matches a page in order and uses the greatest value of an entity for desc", () => {
		const rows = [row(1, "3"), row(1, "9"), row(2, "5"), row(3, "1")]
		const v = verdict(order(), rows, ids(1, 2, 3))
		expect(v).toMatchObject({result: "match", complete: true, uncompared: []})
	})

	it("uses the least value for asc", () => {
		const rows = [row(1, "3"), row(1, "9"), row(2, "5")]
		expect(verdict(order({direction: "asc"}), rows, ids(1, 2))).toMatchObject({result: "match"})
		expect(verdict(order({direction: "asc"}), rows, ids(2, 1))).toMatchObject({result: "mismatch"})
	})

	it("flags a wrong order", () => {
		const rows = [row(1, "1"), row(2, "5")]
		const v = verdict(order(), rows, ids(1, 2))
		expect(v).toMatchObject({result: "mismatch", detail: {firstBreak: {index: 0, reason: "key"}}})
	})

	it("breaks a tie by entity id", () => {
		const rows = [row(1, "4"), row(2, "4")]
		expect(verdict(order(), rows, ids(1, 2))).toMatchObject({result: "match"})
		expect(verdict(order(), rows, ids(2, 1))).toMatchObject({
			result: "mismatch",
			detail: {firstBreak: {reason: "tie"}},
		})
	})

	it("is partial when an entity has no value, and says nothing when no pair can be placed", () => {
		const rows = [row(1, "5"), row(3, "1")]
		expect(verdict(order(), rows, ids(1, 2, 3))).toMatchObject({result: "skip"})
		const more = [row(1, "5"), row(2, "4"), row(4, "1")]
		expect(verdict(order(), more, ids(1, 2, 3, 4))).toMatchObject({
			result: "match",
			complete: false,
			uncompared: ["order_partial"],
		})
	})

	it("judges text only by ties", () => {
		const o = order({dataType: "text"})
		expect(verdict(o, [row(1, "x"), row(2, "x")], ids(1, 2))).toMatchObject({result: "match", complete: true})
		expect(verdict(o, [row(2, "x"), row(1, "x")], ids(2, 1))).toMatchObject({result: "mismatch"})
		expect(verdict(o, [row(1, "x"), row(2, "y")], ids(2, 1))).toMatchObject({result: "skip"})
	})

	it("counts a value only in a Space where the entity has one of the Types", () => {
		const typed = (n: number, space: string): GeoCardRow => ({
			prop: conformingId("entity", TYPES),
			raw: conformingId("entity", T1),
			entity: conformingId("entity", eid(n)),
			space,
		})
		const o = order({typeIds: [T1], spaceIds: [SPACE, "d9f267dcb0d270718c2a3c45a64afd32"]})
		// Entity 1's big value sits in a Space where it has no Type, so its key is 2; entity 2's is 5.
		const rows = [typed(1, "s1"), typed(2, "s1"), row(1, "2", "s1"), row(1, "99", "s2"), row(2, "5", "s1")]
		expect(judgePage(o, ids(2, 1), rows, null)).toMatchObject({result: "match"})
		expect(judgePage(o, ids(1, 2), rows, null)).toMatchObject({result: "mismatch"})
	})
})

describe("checkPropertyOrder", () => {
	const deps = (rows: GeoCardRow[] | Error) => {
		const readCards = vi.fn(async () => {
			if (rows instanceof Error) throw rows
			return rows
		})
		const warn = vi.fn()
		return {
			readCards,
			warn,
			deps: {
				geoDb: {readCards},
				geoSpaceOf: async (s: string) => (s === SPACE ? "g".repeat(0) + "a".repeat(32) : null),
				log: {info: vi.fn(), warn},
			} as never,
		}
	}

	it("reads the page's entities once and judges them", async () => {
		const d = deps([row(1, "5"), row(2, "1")])
		const r = await checkPropertyOrder(d.deps, order(), ids(1, 2), "ordered")
		expect(r).toMatchObject({result: "match", complete: true})
		expect(d.readCards).toHaveBeenCalledTimes(1)
	})

	it("skips a short page, an unknown Space and an unknown data type without reading", async () => {
		const d = deps([])
		expect(await checkPropertyOrder(d.deps, order(), ids(1), "o")).toEqual({result: "skip"})
		expect(await checkPropertyOrder(d.deps, order({spaceIds: ["e".repeat(32)]}), ids(1, 2), "o")).toEqual({
			result: "skip",
		})
		expect(await checkPropertyOrder(d.deps, order({dataType: "point"}), ids(1, 2), "o")).toEqual({result: "skip"})
		expect(d.readCards).not.toHaveBeenCalled()
	})

	it("respects the read budget", async () => {
		const d = deps([row(1, "5"), row(2, "1")])
		const budgeted = {...(d.deps as object), budget: {tryAcquire: () => false}} as never
		expect(await checkPropertyOrder(budgeted, order(), ids(1, 2), "o")).toEqual({result: "skip"})
		expect(d.readCards).not.toHaveBeenCalled()
	})

	it("reports a failed read without throwing", async () => {
		const d = deps(new Error("boom"))
		expect(await checkPropertyOrder(d.deps, order(), ids(1, 2), "o")).toEqual({result: "failed"})
		expect(d.warn).toHaveBeenCalled()
	})
})
