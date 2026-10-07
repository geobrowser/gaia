import {describe, expect, it} from "vitest"
import {
	AGREEMENT_PENALTY,
	cosine,
	type OwnTopicWeight,
	type PairFitCandidate,
	type PairFitItem,
	type PairFitResult,
	relevance,
	type SharedClaim,
	scorePairs,
} from "./score"

const USER = "u".padEnd(32, "0")
const topic = (n: number) => `${"t".repeat(28)}${n.toString().padStart(4, "0")}`
const claim = (n: number) => `${"c".repeat(28)}${n.toString().padStart(4, "0")}`
const person = (n: number) => `${"p".repeat(28)}${n.toString().padStart(4, "0")}`

const w = (t: number, weight: number): OwnTopicWeight => ({topicId: topic(t), weight})
const shared = (n: number, opposite: boolean, topics: number[] = []): SharedClaim => ({
	claimId: claim(n),
	opposite,
	topicIds: topics.map(topic),
})

function candidate(n: number, spec: Partial<PairFitCandidate> = {}): PairFitCandidate {
	return {userId: person(n), weights: [], sharedClaims: [], accountWeight: 1, excluded: false, ...spec}
}

const names = new Map([
	[topic(1), "AI safety"],
	[topic(2), "Energy"],
	[claim(1), "Nuclear power is safe"],
	[claim(2), "AGI by 2030"],
])

function at(r: PairFitResult, index: number): PairFitItem {
	const item = r.items[index]
	if (!item) throw new Error(`no item ${index}`)
	return item
}

const score = (user: OwnTopicWeight[], candidates: PairFitCandidate[], n = names) =>
	scorePairs({user: {userId: USER, weights: user}, candidates, names: n})

describe("cosine", () => {
	it("is 1 for the same tastes at any scale, 0 for disjoint ones", () => {
		const a = new Map([
			["x", 1],
			["y", 2],
		])
		const b = new Map([
			["x", 10],
			["y", 20],
		])
		expect(cosine(a, b)).toBeCloseTo(1, 12)
		expect(cosine(a, new Map([["z", 3]]))).toBe(0)
		expect(cosine(a, new Map())).toBe(0)
	})
})

describe("relevance", () => {
	it("counts a claim neither cares about half, one both care about most fully", () => {
		expect(relevance(0, 0)).toBe(0.5)
		expect(relevance(1, 0)).toBe(0.5)
		expect(relevance(1, 1)).toBe(1)
		expect(relevance(0.25, 1)).toBeCloseTo(0.75, 12)
	})
})

describe("scorePairs", () => {
	it("scores a pair with no shared votes on interest alone, and never as disagreeing", () => {
		const r = score([w(1, 4), w(2, 1)], [candidate(1, {weights: [w(1, 2), w(2, 0.5)]})])
		const item = at(r, 0)
		expect(item.parts.interest).toBeCloseTo(1, 12)
		expect(item.parts.disagreement).toBe(0)
		expect(item.disagreeing).toBe(false)
		expect(item.score).toBeCloseTo(1 / 3, 12)
		expect(item.reason).toEqual({
			kind: "shared_topic",
			topicId: topic(1),
			name: "AI safety",
			text: "You both care about AI safety",
		})
	})

	it("gives strangers with nothing in common zero and no reason", () => {
		const r = score([w(1, 4)], [candidate(1, {weights: [w(2, 3)]}), candidate(2)])
		expect(r.items.map((i) => [i.score, i.reason])).toEqual([
			[0, null],
			[0, null],
		])
	})

	it("scores a disagreement by both people's interest in the claim's topics", () => {
		// Both care most about topic 1, so a claim on it is fully relevant; topic 2 interests
		// neither, so a claim on it counts half.
		const user = [w(1, 4)]
		const onTopic = candidate(1, {weights: [w(1, 2)], sharedClaims: [shared(1, true, [1])]})
		const offTopic = candidate(2, {weights: [w(1, 2)], sharedClaims: [shared(2, true, [2])]})
		const r = score(user, [offTopic, onTopic])
		expect(r.items.map((i) => i.userId)).toEqual([person(1), person(2)])
		const on = at(r, 0)
		const off = at(r, 1)
		expect(on.parts.disagreement).toBeCloseTo(1 / (1 + 1), 12)
		expect(off.parts.disagreement).toBeCloseTo(0.5 / (0.5 + 1), 12)
		expect(on.score).toBeCloseTo((1 + 2 * 0.5) / 3, 12)
		expect(on.disagreeing).toBe(true)
		expect(on.parts).toMatchObject({sharedClaims: 1, opposed: 1, agreed: 0})
		expect(on.reason).toEqual({
			kind: "disagree",
			claimId: claim(1),
			name: "Nuclear power is safe",
			text: "You two disagree on Nuclear power is safe",
		})
	})

	it("ranks a disagreeing pair above an interest-only twin", () => {
		const weights = [w(1, 1)]
		const r = score(weights, [
			candidate(1, {weights}),
			candidate(2, {weights, sharedClaims: [shared(1, true, [1])]}),
		])
		expect(r.items.map((i) => i.userId)).toEqual([person(2), person(1)])
	})

	it("lets agreement dampen a disagreement but never go below interest alone", () => {
		const weights = [w(1, 1)]
		const mixed = candidate(1, {weights, sharedClaims: [shared(1, true, [1]), shared(2, false, [1])]})
		const agreeOnly = candidate(2, {weights, sharedClaims: [shared(3, false, [1]), shared(4, false, [1])]})
		const r = score(weights, [mixed, agreeOnly])
		const byId = new Map(r.items.map((i) => [i.userId, i]))
		const raw = 1 - AGREEMENT_PENALTY * 1
		expect(byId.get(person(1))?.parts.disagreement).toBeCloseTo(raw / (raw + 1), 12)
		expect(byId.get(person(1))?.parts).toMatchObject({sharedClaims: 2, opposed: 1, agreed: 1})
		const agree = byId.get(person(2))
		expect(agree?.parts.disagreement).toBe(0)
		expect(agree?.disagreeing).toBe(false)
		expect(agree?.score).toBeCloseTo(1 / 3, 12)
		expect(agree?.reason?.kind).toBe("shared_topic")
	})

	it("is labelled disagreeing only when it counts: a weight-0 account is not", () => {
		const r = score([w(1, 1)], [candidate(1, {accountWeight: 0, sharedClaims: [shared(1, true, [1])]})])
		expect(at(r, 0).disagreeing).toBe(false)
		expect(at(r, 0).parts.disagreement).toBe(0)
	})

	it("scales the candidate's disagreement by their account weight", () => {
		const one = at(score([], [candidate(1, {accountWeight: 1, sharedClaims: [shared(1, true)]})]), 0)
		const ramp = at(score([], [candidate(1, {accountWeight: 0.25, sharedClaims: [shared(1, true)]})]), 0)
		expect(ramp.parts.disagreement).toBeCloseTo(one.parts.disagreement * 0.25, 12)
		expect(ramp.parts.accountWeight).toBe(0.25)
	})

	it("leaves out excluded accounts and the user themselves", () => {
		const r = score(
			[w(1, 1)],
			[
				candidate(1, {excluded: true, weights: [w(1, 1)], sharedClaims: [shared(1, true)]}),
				{...candidate(2), userId: USER},
				candidate(3),
			],
		)
		expect(r.items.map((i) => i.userId)).toEqual([person(3)])
		expect(r.excluded).toEqual([{userId: person(1), reason: "excluded_account"}])
	})

	it("names the most relevant disagreement, and says how many when it has no name", () => {
		const weights = [w(1, 4), w(2, 1)]
		const r = score(weights, [
			candidate(1, {weights, sharedClaims: [shared(1, true, [2]), shared(2, true, [1]), shared(9, true)]}),
		])
		expect(at(r, 0).reason).toMatchObject({claimId: claim(2), text: "You two disagree on AGI by 2030"})
		const unnamed = score([], [candidate(1, {sharedClaims: [shared(7, true), shared(8, true)]})], new Map())
		expect(at(unnamed, 0).reason).toEqual({
			kind: "disagree",
			claimId: claim(7),
			name: null,
			text: "You two disagree on 2 claims",
		})
	})

	it("keeps the caller's order among equal scores", () => {
		const r = score([], [candidate(3), candidate(1), candidate(2)])
		expect(r.items.map((i) => i.userId)).toEqual([person(3), person(1), person(2)])
	})

	it("never returns either person's side", () => {
		const r = score([w(1, 1)], [candidate(1, {weights: [w(1, 1)], sharedClaims: [shared(1, true, [1])]})])
		expect(JSON.stringify(r)).not.toMatch(/agree"?:\s*(true|false)|vote_type|voteType|position|stance/i)
	})
})
