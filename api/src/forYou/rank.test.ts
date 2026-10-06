import {describe, expect, it} from "vitest"
import {type CandidateRow, type ForYouConfig, rankForYou, reasonText, type TopicWeight} from "./rank"

// The migration's defaults (0102_for_you_serving.sql) and the live tau.
const CONFIG: ForYouConfig = {maxBoostDays: 6, interestHalfSaturation: 2, explorationShare: 0.1, maxTopics: 50}
const TAU = 100000
const DAY = 86400 / TAU

const id = (n: number) => n.toString(16).padStart(32, "0")
const topic = (n: number) => `${"f".repeat(28)}${n.toString(16).padStart(4, "0")}`

function candidates(
	specs: {score: number | null; topics?: number[]; excluded?: CandidateRow["excluded"]}[],
): CandidateRow[] {
	return specs.map((s, i) => ({
		entityId: id(i + 1),
		rankingScore: s.score,
		topicIds: (s.topics ?? []).map(topic),
		excluded: s.excluded ?? null,
	}))
}

function weight(t: number, w: number, kind = "vote", count: number | null = Math.round(w)): TopicWeight {
	return {
		topicId: topic(t),
		weight: w,
		ownWeight: w,
		relatedWeight: 0,
		topKind: kind,
		topKindCount: count,
		topObjectId: null,
		relatedViaTopicId: null,
	}
}

const rank = (c: CandidateRow[], w: TopicWeight[], config: Partial<ForYouConfig> = {}, seed = "s") =>
	rankForYou({
		candidates: c,
		weights: w,
		topicNames: new Map([[topic(1), "AI safety"]]),
		config: {...CONFIG, explorationShare: 0, ...config},
		tauSeconds: TAU,
		seed,
	})

const order = (r: ReturnType<typeof rank>) => r.items.map((i) => i.entityId)

describe("rankForYou", () => {
	it("keeps Best's order exactly when nothing matches the user", () => {
		const c = candidates([{score: 10}, {score: 9, topics: [2]}, {score: 8}])
		const r = rank(c, [weight(1, 5)])
		expect(order(r)).toEqual([id(1), id(2), id(3)])
		expect(r.items.every((i) => i.score.boost === 0 && i.reason === null)).toBe(true)
	})

	it("lifts an interest item over a Best item less than its boost ahead, and no further", () => {
		// weight 4 at half-saturation 2 is two thirds of 6 days: 4 days.
		const boost = 4 * DAY
		const c = candidates([{score: 10 + boost + 0.01}, {score: 10 + boost - 0.01}, {score: 10, topics: [1]}])
		const r = rank(c, [weight(1, 4)])
		expect(order(r)).toEqual([id(1), id(3), id(2)])
		expect(r.items[1]!.score.boost).toBeCloseTo(boost, 10)
		expect(r.items[1]!.score.total).toBeCloseTo(10 + boost, 10)
	})

	it("never boosts past max_boost_days, however strong the interest", () => {
		const c = candidates([{score: 10 + 6 * DAY + 0.001}, {score: 10, topics: [1]}])
		const r = rank(c, [weight(1, 1e9)])
		expect(order(r)).toEqual([id(1), id(2)])
		expect(r.items[1]!.score.boost).toBeLessThan(6 * DAY)
	})

	it("a single fresh vote is worth two days of recency at the defaults", () => {
		const r = rank(candidates([{score: 10, topics: [1]}]), [weight(1, 1)])
		expect(r.items[0]!.score.boost / DAY).toBeCloseTo(2, 10)
	})

	it("scores an item by its most-weighted topic, not the sum of its topics", () => {
		const c = candidates([
			{score: 10, topics: [1, 2, 3]},
			{score: 10, topics: [4]},
		])
		const r = rank(c, [weight(1, 1), weight(2, 1), weight(3, 1), weight(4, 2)])
		expect(order(r)).toEqual([id(2), id(1)])
		expect(r.items[1]!.score.interest).toBe(1)
	})

	it("drops already-answered candidates and says why", () => {
		const c = candidates([
			{score: 10, excluded: "voted"},
			{score: 9},
			{score: 8, excluded: "interested"},
			{score: 7, excluded: "not_interested"},
		])
		const r = rank(c, [weight(1, 1)])
		expect(order(r)).toEqual([id(2)])
		expect(r.excluded).toEqual([
			{entityId: id(1), reason: "voted"},
			{entityId: id(3), reason: "interested"},
			{entityId: id(4), reason: "not_interested"},
		])
	})

	it("ranks an unscored candidate as Best's lowest, not as zero", () => {
		const c = candidates([{score: 17900}, {score: 17899}, {score: null, topics: [1]}])
		const r = rank(c, [weight(1, 100)])
		const unscored = r.items.find((i) => i.entityId === id(3))!
		expect(unscored.score.best).toBeNull()
		expect(unscored.score.total).toBeGreaterThan(17899)
	})

	it("gives a reason a card can show", () => {
		const r = rank(candidates([{score: 10, topics: [1]}]), [weight(1, 4, "vote", 4)])
		expect(r.items[0]!.reason).toMatchObject({topicId: topic(1), topicName: "AI safety", kind: "vote", count: 4})
		expect(r.items[0]!.reason?.text).toBe("4 votes on AI safety")
	})

	it("is deterministic", () => {
		const c = candidates(Array.from({length: 40}, (_, i) => ({score: 100 - i, topics: i % 3 === 0 ? [1] : []})))
		const a = rank(c, [weight(1, 3)], {explorationShare: 0.1}, "user:window")
		const b = rank(c, [weight(1, 3)], {explorationShare: 0.1}, "user:window")
		expect(a).toEqual(b)
	})
})

describe("exploration", () => {
	// 60 candidates: every third on the user's topic, the rest outside their interests.
	const c = candidates(Array.from({length: 60}, (_, i) => ({score: 1000 - i * 0.1, topics: i % 3 === 0 ? [1] : [2]})))
	const w = [weight(1, 3)]

	it("takes the stated share of slots, only from outside the user's interests", () => {
		const r = rank(c, w, {explorationShare: 0.1})
		const explored = r.items.filter((i) => i.exploration)
		expect(explored).toHaveLength(6)
		expect(r.exploration).toEqual({share: 0.1, slots: 6, poolSize: 40, picked: 6})
		expect(explored.every((i) => i.score.interest === 0)).toBe(true)
		expect(explored.every((i) => i.explorationProbability === 6 / 40)).toBe(true)
		expect(r.items.filter((i) => !i.exploration).every((i) => i.explorationProbability === null)).toBe(true)
		expect(r.items).toHaveLength(60)
		expect(new Set(order(r)).size).toBe(60)
	})

	it("places the k-th pick no lower than position 10k - 1", () => {
		for (const seed of ["a", "b", "c", "d", "e"]) {
			const r = rank(c, w, {explorationShare: 0.1}, seed)
			const positions = r.items.flatMap((item, index) => (item.exploration ? [index] : []))
			positions.forEach((p, k) => expect(p).toBeLessThanOrEqual(10 * (k + 1) - 1))
		}
	})

	it("varies the picks with the seed", () => {
		const picks = new Set(
			["a", "b", "c", "d", "e", "f"].map((seed) =>
				rank(c, w, {explorationShare: 0.1}, seed)
					.items.filter((i) => i.exploration)
					.map((i) => i.entityId)
					.join(),
			),
		)
		expect(picks.size).toBeGreaterThan(1)
	})

	it("picks every outside item about equally often", () => {
		const counts = new Map<string, number>()
		const runs = 2000
		for (let s = 0; s < runs; s += 1) {
			for (const item of rank(c, w, {explorationShare: 0.1}, `seed-${s}`).items) {
				if (item.exploration) counts.set(item.entityId, (counts.get(item.entityId) ?? 0) + 1)
			}
		}
		// Each of the 40 is picked with probability 6/40 = 0.15, so ~300 times in 2000 runs.
		expect(counts.size).toBe(40)
		for (const n of counts.values()) expect(Math.abs(n / runs - 0.15)).toBeLessThan(0.05)
	})

	it("does nothing when everything matches the user", () => {
		const all = candidates(Array.from({length: 30}, (_, i) => ({score: 100 - i, topics: [1]})))
		const r = rank(all, w, {explorationShare: 0.1})
		expect(r.items.some((i) => i.exploration)).toBe(false)
		expect(r.exploration.picked).toBe(0)
	})
})

describe("reasonText", () => {
	it("reads naturally for every kind", () => {
		expect(reasonText("vote", 1, "AI safety", null)).toBe("1 vote on AI safety")
		expect(reasonText("comment", 3, "Energy", null)).toBe("3 comments on Energy")
		expect(reasonText("debate", 2, "Energy", null)).toBe("2 debates on Energy")
		expect(reasonText("follow", 1, "Energy", null)).toBe("You follow Energy")
		expect(reasonText("interested", 1, "Energy", null)).toBe("Interested in 1 question on Energy")
		expect(reasonText("related", null, "Grid storage", "Energy")).toBe("Grid storage, related to Energy")
		expect(reasonText("vote", 2, null, null)).toBe("2 votes on a topic you engage with")
	})
})
