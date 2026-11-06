import {readFileSync} from "node:fs"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {FeedArgumentError, type FeedData, Feeds} from "../feeds"
import {nextPrimerClaim, type PrimerData, primerCandidates} from "../primer"

interface Case {
	fn: string
	args: Record<string, any>
	rows: any[]
}
interface Golden {
	now: string
	seeds: {seed: number; tables: Record<string, any[]>; cases: Case[]}[]
}

const golden = JSON.parse(readFileSync(join(__dirname, "fixtures", "golden-feeds.json"), "utf8")) as Golden
const now = new Date(golden.now)

const feedData = (t: Record<string, any[]>): FeedData => ({
	entities: t.entities!,
	relations: t.relations!,
	names: t.names!,
	scores: t.scores!,
	blocklist: t.blocklist!,
	exclusions: t.exclusions!,
	typeRanking: t.typeRanking!,
	topicRanking: t.topicRanking!,
})
const primerData = (t: Record<string, any[]>): PrimerData => ({
	spaces: t.spaces!,
	votes: t.votes!,
	stats: t.stats!,
	anchorSets: t.anchorSets!,
	anchorMembers: t.anchorMembers!,
	entities: t.entities!,
})

/** The generator picks answered and skipped as consecutive slices of the stats table by claim id. */
function slices(t: Record<string, any[]>, answered: number, skipped: number) {
	const ids = t.stats!.map((s) => s.claim_id as string).sort()
	return {answered: ids.slice(0, answered), skipped: ids.slice(answered, answered + skipped)}
}

describe("group B feed and primer twins against Postgres golden output", () => {
	it("covers every function with non-empty output", () => {
		const nonEmpty = new Map<string, number>()
		for (const s of golden.seeds)
			for (const c of s.cases) if (c.rows.length > 0) nonEmpty.set(c.fn, (nonEmpty.get(c.fn) ?? 0) + 1)
		for (const fn of [
			"entities_ranked_for_feed",
			"entities_ranked_for_feed_by_type",
			"entities_ranked_for_topics",
			"primer_candidates",
			"next_primer_claim",
			"entities_ranked_for_topics_error",
		])
			expect(nonEmpty.get(fn) ?? 0, fn).toBeGreaterThan(10)
	})

	for (const s of golden.seeds) {
		it(`replays every case for seed ${s.seed}`, () => {
			const feeds = new Feeds(feedData(s.tables))
			const pd = primerData(s.tables)
			for (const c of s.cases) {
				const a = c.args
				const label = `${c.fn} ${JSON.stringify(a)}`
				switch (c.fn) {
					case "entities_ranked_for_feed":
						expect(
							feeds.entitiesRankedForFeed(a.min, a.after, a.before, a.spaces, a.types).map((e) => e.id),
							label,
						).toEqual(c.rows.map((r) => r.id))
						break
					case "entities_ranked_for_feed_by_type":
						expect(
							feeds
								.entitiesRankedForFeedByType(a.types, a.min, a.after, null, a.spaces, a.cap)
								.map((e) => e.id),
							label,
						).toEqual(c.rows.map((r) => r.id))
						break
					case "entities_ranked_for_topics": {
						const got = feeds.entitiesRankedForTopics({
							topics: a.topics,
							weights: a.weights,
							types: a.types,
							min: a.min,
							after: a.after,
							before: null,
							spaces: a.spaces,
							maxPerTopic: a.cap,
							debateTaggedClaims: a.debateClaims,
							sortBy: a.sort,
							matchAll: a.matchAll,
						})
						expect(
							got.map((e) => e.id),
							label,
						).toEqual(c.rows.map((r) => r.id))
						break
					}
					case "entities_ranked_for_topics_error": {
						let got = {code: "ok", msg: ""}
						try {
							feeds.entitiesRankedForTopics({
								topics: a.topics,
								weights: a.weights,
								types: null,
								min: null,
								after: null,
								before: null,
								spaces: null,
								maxPerTopic: null,
								debateTaggedClaims: false,
								sortBy: a.sort,
								matchAll: false,
							})
						} catch (e) {
							expect(e, label).toBeInstanceOf(FeedArgumentError)
							got = {code: (e as FeedArgumentError).code, msg: (e as Error).message}
						}
						expect(got, label).toEqual(c.rows[0])
						break
					}
					case "primer_candidates": {
						const {answered, skipped} = slices(s.tables, a.answered, a.skipped)
						const got = primerCandidates(
							pd,
							{
								userId: a.user,
								spaceIds: a.spaces,
								answered,
								skipped,
								primerSize: a.size,
								minAnchors: a.minAnchors,
								poolMin: a.poolMin,
								minVoters: a.minVoters,
								noveltyWeight: a.noveltyWeight,
								maxNameChars: a.maxNameChars,
							},
							now,
						)
						expect(got, label).toEqual(c.rows)
						break
					}
					case "next_primer_claim": {
						const {answered, skipped} = slices(s.tables, a.answered, a.skipped)
						const got = nextPrimerClaim(pd, {userId: a.user, spaceIds: a.spaces, answered, skipped}, now)
						expect(got ? [got.id] : [], label).toEqual(c.rows.map((r) => r.id))
						break
					}
					default:
						throw new Error(`unknown function ${c.fn}`)
				}
			}
		})
	}
})
