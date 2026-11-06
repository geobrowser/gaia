import {readFileSync} from "node:fs"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {addDays, type Graph, listedAt, nameKey, positionOrder, Trending, valueDate} from "../trending"

interface Case {
	fn: string
	args: Record<string, unknown>
	rows: Record<string, unknown>[]
}
interface Golden {
	today: string
	seeds: {seed: number; tables: Record<string, unknown[]>; cases: Case[]}[]
}

const golden = JSON.parse(readFileSync(join(__dirname, "fixtures", "golden.json"), "utf8")) as Golden

const key = (r: object) => JSON.stringify(Object.entries(r).sort(([a], [b]) => (a < b ? -1 : 1)))
const asSet = (rows: object[]) => rows.map(key).sort()

function run(t: Trending, c: Case): object[] {
	const a = c.args as Record<string, any>
	switch (c.fn) {
		case "claims_and_debates":
			return t.claimsAndDebates(a.ids)
		case "debate_links":
			return t.debateLinks(a.claims, a.debates)
		case "current_agendas":
			return t.currentAgendas()
		case "current_agenda_listings":
			return t.currentAgendaListings()
		case "agenda_contents":
			return t.agendaContents(a.spaces, a.since)
		case "newest_listings":
			return t.newestListings(a.ids, a.sources)
		case "claim_listings":
			return t.claimListings(a.ids)
		case "entity_listings":
			return t.entityListings(a.ids)
		case "entity_topic_listings":
			return t.entityTopicListings(a.ids)
		case "current_debate_agendas":
			return t.currentDebateAgendas(a.spaces, a.earlier) as unknown as Record<string, unknown>[]
		case "topic_suggestions_for_spaces":
			return t.topicSuggestionsForSpaces(a.spaces) as unknown as Record<string, unknown>[]
		default:
			throw new Error(`unknown function ${c.fn}`)
	}
}

const ORDERED = new Set(["current_debate_agendas", "topic_suggestions_for_spaces"])

describe("group B trending twins against Postgres golden output", () => {
	it("covers every function with non-empty output", () => {
		const nonEmpty = new Map<string, number>()
		for (const s of golden.seeds)
			for (const c of s.cases) if (c.rows.length > 0) nonEmpty.set(c.fn, (nonEmpty.get(c.fn) ?? 0) + 1)
		for (const fn of [
			"claims_and_debates",
			"debate_links",
			"current_agendas",
			"current_agenda_listings",
			"agenda_contents",
			"newest_listings",
			"claim_listings",
			"entity_listings",
			"entity_topic_listings",
			"current_debate_agendas",
			"topic_suggestions_for_spaces",
		]) {
			expect(nonEmpty.get(fn) ?? 0, fn).toBeGreaterThan(0)
		}
	})

	for (const s of golden.seeds) {
		it(`seed ${s.seed}: every case matches`, () => {
			const t = new Trending(s.tables as unknown as Graph, golden.today)
			let compared = 0
			for (const c of s.cases) {
				const got = run(t, c)
				const want = c.rows.map((r) =>
					r.listed_at ? {...r, listed_at: new Date(r.listed_at as string).toISOString()} : r,
				)
				expect(got.length, `${c.fn} ${JSON.stringify(c.args).slice(0, 80)} length`).toBe(want.length)
				if (ORDERED.has(c.fn)) expect(got.map(key), c.fn).toEqual(want.map(key))
				else expect(asSet(got), c.fn).toEqual(asSet(want))
				compared += want.length
			}
			expect(compared).toBeGreaterThan(0)
		})
	}
})

describe("helpers", () => {
	it("valueDate", () => {
		expect(valueDate({datetime: "2026-10-30T10:00", date: null, datetime_utc: null})).toBe("2026-10-30")
		expect(valueDate({datetime: null, date: "2026-02-30", datetime_utc: null})).toBeNull()
		expect(valueDate({datetime: "2026-13-01", date: "2026-01-01", datetime_utc: null})).toBeNull()
		expect(valueDate({datetime: null, date: null, datetime_utc: "2026-11-02T02:00:00+00:00"})).toBe("2026-11-01")
		expect(valueDate({datetime: null, date: null, datetime_utc: null})).toBeNull()
	})
	it("listedAt follows daylight saving", () => {
		expect(listedAt("2026-11-01")).toBe("2026-11-01T04:00:00.000Z")
		expect(listedAt("2026-11-02")).toBe("2026-11-02T05:00:00.000Z")
	})
	it("positionOrder swaps case and sorts nulls last", () => {
		expect(positionOrder("aB")?.toString()).toBe("ab Ab")
		expect(positionOrder(null)).toBeNull()
	})
	it("addDays and nameKey", () => {
		expect(addDays("2026-03-01", -1)).toBe("2026-02-28")
		expect(nameKey("  Foo \t Bar ")).toBe("foo bar")
	})
})
