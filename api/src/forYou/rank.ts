/**
 * For you (GEO-3140): Best's candidate window re-ordered by one user's topic interests.
 *
 * Pure: everything it reads is passed in, so the ordering, the exploration picks and the reasons
 * are unit-tested without a database. The route (`router.ts`) does the reads.
 *
 * The caller passes Best's candidates in Best's order. Each candidate's score is
 *
 *     total = best + boost,   boost = maxBoostDays * (86400 / tau) * w / (w + halfSaturation)
 *
 * where `best` is its stored ranking_score and `w` the user's weight on its most-weighted topic.
 * Why that shape, and how its two constants were chosen, is in migration 0102's header.
 *
 * Exploration: `explorationShare` of the slots go to candidates with no weight on any of their
 * topics, picked uniformly at random from all such candidates and placed at every 1/share-th
 * position (9, 19, 29, ... at 10%), or kept where they were if that is higher. Each is marked with
 * the probability it had of being picked. The random source is seeded from the user and the
 * candidate list, so the same window re-ranks identically on every request, which the web app's
 * offset pagination into a window depends on.
 */

/** Bump on any change to this file's logic. The served version is `for-you-<this>.<config revision>`. */
export const FOR_YOU_CODE_VERSION = 1

export type ForYouConfig = {
	maxBoostDays: number
	interestHalfSaturation: number
	explorationShare: number
	maxTopics: number
}

/** One row of `personalization.user_topic_weights`. */
export type TopicWeight = {
	topicId: string
	weight: number
	ownWeight: number
	relatedWeight: number
	/** vote | comment | debate | follow | interested | not_interested | related */
	topKind: string
	topKindCount: number | null
	topObjectId: string | null
	relatedViaTopicId: string | null
}

/**
 * Why a candidate is left out of a user's For you: a position they still hold on it, Interested on
 * a question, or Not interested (GEO-2862, synced from geo-chat; migration 0105).
 */
export const EXCLUSION_REASONS = ["voted", "interested", "not_interested"] as const
export type ExclusionReason = (typeof EXCLUSION_REASONS)[number]

export function isExclusionReason(value: unknown): value is ExclusionReason {
	return typeof value === "string" && (EXCLUSION_REASONS as readonly string[]).includes(value)
}

/** One row of `personalization.for_you_candidates`. */
export type CandidateRow = {
	entityId: string
	rankingScore: number | null
	topicIds: string[]
	excluded: ExclusionReason | null
}

export type ForYouReason = {
	topicId: string
	topicName: string | null
	kind: string
	count: number | null
	viaTopicId: string | null
	viaTopicName: string | null
	/** Short enough for a card, e.g. "4 votes on AI safety". */
	text: string
}

export type ForYouItem = {
	entityId: string
	/** Index in Best's order, as passed in. */
	bestPosition: number
	score: {
		/** Best's ranking_score; null when the entity has none stored (ranked as Best's lowest). */
		best: number | null
		/** The user's weight on the item's most-weighted topic, in interest units. */
		interest: number
		/** What that interest added, in ranking-score units. */
		boost: number
		total: number
	}
	/** The main reason it was boosted; null when nothing about it matches the user. */
	reason: ForYouReason | null
	exploration: boolean
	/** The chance it had of being picked for an exploration slot; null unless `exploration`. */
	explorationProbability: number | null
}

export type ForYouRanking = {
	items: ForYouItem[]
	excluded: {entityId: string; reason: ExclusionReason}[]
	exploration: {share: number; slots: number; poolSize: number; picked: number}
}

const SECONDS_PER_DAY = 86400

function plural(n: number, one: string, many: string): string {
	return `${n} ${n === 1 ? one : many}`
}

export function reasonText(
	kind: string,
	count: number | null,
	topicName: string | null,
	viaTopicName: string | null,
): string {
	const topic = topicName ?? "a topic you engage with"
	const n = count ?? 1
	switch (kind) {
		case "vote":
			return `${plural(n, "vote", "votes")} on ${topic}`
		case "comment":
			return `${plural(n, "comment", "comments")} on ${topic}`
		case "debate":
			return `${plural(n, "debate", "debates")} on ${topic}`
		case "follow":
			return `You follow ${topic}`
		case "interested":
			return `Interested in ${plural(n, "question", "questions")} on ${topic}`
		case "related":
			return viaTopicName ? `${topic}, related to ${viaTopicName}` : `Related to topics you engage with`
		default:
			return `Your activity on ${topic}`
	}
}

/** FNV-1a, for a stable 32-bit seed from a string. */
export function hashSeed(input: string): number {
	let h = 0x811c9dc5
	for (let i = 0; i < input.length; i += 1) {
		h ^= input.charCodeAt(i)
		h = Math.imul(h, 0x01000193)
	}
	return h >>> 0
}

/** mulberry32: small, fast, and good enough to pick a handful of slots. */
export function seededRandom(seed: number): () => number {
	let a = seed >>> 0
	return () => {
		a = (a + 0x6d2b79f5) >>> 0
		let t = a
		t = Math.imul(t ^ (t >>> 15), t | 1)
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296
	}
}

export function rankForYou(args: {
	candidates: readonly CandidateRow[]
	weights: readonly TopicWeight[]
	topicNames: ReadonlyMap<string, string>
	config: ForYouConfig
	tauSeconds: number
	seed: string
}): ForYouRanking {
	const {config} = args
	const weightOf = new Map(args.weights.map((w) => [w.topicId, w]))
	const unitsPerDay = SECONDS_PER_DAY / args.tauSeconds
	const maxBoost = config.maxBoostDays * unitsPerDay

	const excluded: ForYouRanking["excluded"] = []
	const kept: (CandidateRow & {bestPosition: number})[] = []
	for (const [bestPosition, c] of args.candidates.entries()) {
		if (c.excluded) excluded.push({entityId: c.entityId, reason: c.excluded})
		else kept.push({...c, bestPosition})
	}

	// A candidate with no stored score ranks as Best's lowest, not as zero: scores carry a large
	// recency offset, so zero would sink it below everything by years.
	const scored = kept.map((c) => c.rankingScore).filter((s): s is number => s !== null && Number.isFinite(s))
	const floor = scored.length > 0 ? Math.min(...scored) : 0

	const items: ForYouItem[] = kept.map((c) => {
		let top: TopicWeight | null = null
		for (const t of c.topicIds) {
			const w = weightOf.get(t)
			if (
				w &&
				w.weight > 0 &&
				(top === null || w.weight > top.weight || (w.weight === top.weight && w.topicId < top.topicId))
			)
				top = w
		}
		const interest = top?.weight ?? 0
		const boost = interest > 0 ? (maxBoost * interest) / (interest + config.interestHalfSaturation) : 0
		const best = c.rankingScore !== null && Number.isFinite(c.rankingScore) ? c.rankingScore : null
		const reason: ForYouReason | null = top
			? {
					topicId: top.topicId,
					topicName: args.topicNames.get(top.topicId) ?? null,
					kind: top.topKind,
					count: top.topKindCount,
					viaTopicId: top.relatedViaTopicId,
					viaTopicName: top.relatedViaTopicId ? (args.topicNames.get(top.relatedViaTopicId) ?? null) : null,
					text: reasonText(
						top.topKind,
						top.topKindCount,
						args.topicNames.get(top.topicId) ?? null,
						top.relatedViaTopicId ? (args.topicNames.get(top.relatedViaTopicId) ?? null) : null,
					),
				}
			: null
		return {
			entityId: c.entityId,
			bestPosition: c.bestPosition,
			score: {best, interest, boost, total: (best ?? floor) + boost},
			reason,
			exploration: false,
			explorationProbability: null,
		}
	})

	// Highest total first; Best's own order breaks ties, so a user with no matching weights gets
	// exactly Best's order back.
	items.sort((a, b) => b.score.total - a.score.total || a.bestPosition - b.bestPosition)

	const share = Math.max(0, Math.min(0.5, config.explorationShare))
	const slots = share > 0 ? Math.floor(items.length * share) : 0
	const pool = items.filter((i) => i.score.interest === 0)
	const picked = Math.min(slots, pool.length)
	if (picked === 0) {
		return {items, excluded, exploration: {share, slots, poolSize: pool.length, picked: 0}}
	}

	// Partial Fisher-Yates over the pool: each member is picked with probability picked / pool.
	const random = seededRandom(hashSeed(args.seed))
	const shuffled = [...pool]
	for (let i = 0; i < picked; i += 1) {
		const j = i + Math.floor(random() * (shuffled.length - i))
		const swap = shuffled[i]!
		shuffled[i] = shuffled[j]!
		shuffled[j] = swap
	}
	const chosen = new Set(shuffled.slice(0, picked))
	const probability = picked / pool.length

	const naturalIndex = new Map(items.map((item, index) => [item, index]))
	const rest = items.filter((item) => !chosen.has(item))
	const placements = [...chosen]
		.sort((a, b) => naturalIndex.get(a)! - naturalIndex.get(b)!)
		.map((item, j) => ({
			item,
			// Slot j sits at the (j+1)th 1/share-th position; an item already ranked above its slot
			// stays where it was rather than being pushed down by being explored.
			target: Math.min(naturalIndex.get(item)!, Math.ceil((j + 1) / share) - 1),
		}))
		.sort((a, b) => a.target - b.target)
	for (const {item, target} of placements) {
		rest.splice(Math.min(target, rest.length), 0, {
			...item,
			exploration: true,
			explorationProbability: probability,
		})
	}

	return {items: rest, excluded, exploration: {share, slots, poolSize: pool.length, picked}}
}
