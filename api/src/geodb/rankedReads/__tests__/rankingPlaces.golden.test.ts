import {readFileSync} from "node:fs"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {
	currentRankingPlaces,
	currentRankingPushdowns,
	type RankingInputs,
	type RankingPlace,
	type RankingSource,
	rankingPlaceValues,
} from "../trending"

/**
 * The TypeScript twin of the Best debates ranking reads (migration 0123, GEO-3681), held to the SQL's own output over a
 * seeded graph and ten scenarios (api/scripts/rankingplaces/gen_golden.py; the Rust twin reads the same run's .txt).
 */
interface Scenario {
	name: string
	since: number
	base: number
	step: number
	sources: RankingSource[]
	places_after: RankingPlace[]
	pushdowns: {entity_id: string; downvoters: number; rankers: number}[]
	places: RankingPlace[]
	values: {entity_id: string; place_value: string}[]
}
interface Golden {
	today: string
	now: number
	entities: {id: string; created_at: string; kind: string}[]
	blocked: string[]
	members: RankingInputs["members"]
	scores: RankingInputs["scores"]
	ballots: RankingInputs["ballots"]
	items: RankingInputs["items"]
	votes: (Omit<RankingInputs["votes"][number], "voted_at_us"> & {voted_at_offset: number})[]
	cases: Record<string, string>
	scenarios: Scenario[]
}

const g = JSON.parse(readFileSync(join(__dirname, "fixtures", "ranking-places-golden.json"), "utf8")) as Golden
const nowUs = Math.round(g.now * 1_000_000)
const created = new Map(g.entities.map((e) => [e.id, /^[0-9]+$/.test(e.created_at) ? Number(e.created_at) : null]))
const blocked = new Set(g.blocked)
const inputs: RankingInputs = {
	scores: g.scores,
	ballots: g.ballots,
	items: g.items,
	members: g.members,
	votes: g.votes.map(({voted_at_offset, ...v}) => ({...v, voted_at_us: nowUs - voted_at_offset * 1_000_000})),
	rankable: new Map(
		g.entities
			.filter((e) => ["debate", "both"].includes(e.kind) && !blocked.has(e.id))
			.map((e) => [e.id, created.get(e.id)!]),
	),
}
const key = (p: RankingPlace) => `${p.source}|${p.entity_id}|${p.place}`

describe("Best debates ranking twins (0123)", () => {
	it("has the scenarios and the named cases", () => {
		expect(g.scenarios).toHaveLength(10)
		const onFull = g.scenarios.find((s) => s.name === "on_full")!
		expect(onFull.pushdowns.map((p) => p.entity_id)).toContain(g.cases.pushed)
		expect(onFull.places.filter((p) => p.entity_id === g.cases.in_both)).toHaveLength(2)
		expect(onFull.places.some((p) => p.entity_id === g.cases.ranked_not_listed)).toBe(true)
		expect(onFull.places.some((p) => p.entity_id === g.cases.pushed)).toBe(false)
	})

	for (const s of g.scenarios) {
		it(`${s.name}: push-downs, places and place values match the SQL`, () => {
			const pushed = currentRankingPushdowns(s.sources, inputs, s.since)
			expect(pushed).toEqual(s.pushdowns)
			const places = currentRankingPlaces(s.sources, inputs, new Set(pushed.map((p) => p.entity_id)), g.today)
			expect(places.map(key).sort()).toEqual(s.places.map(key).sort())
			const ids = new Set([...created.keys(), ...s.places_after.map((p) => p.entity_id)])
			const values = rankingPlaceValues(s.base, s.step, s.sources, s.places_after, created, ids, g.today)
			expect(values.size).toBe(s.values.length)
			for (const v of s.values) {
				const got = values.get(v.entity_id)!
				const want = Number(v.place_value)
				expect(Math.abs(got - want)).toBeLessThanOrEqual(1e-9 * Math.max(1, Math.abs(want)))
			}
		})
	}
})
