/**
 * The feed and primer half of the group B ranked reads' shadow (ZP-GROUPB-READS-2; the runner and metrics live in
 * `rankedReadsShadow.ts`, which mounts this). Reads stay on Postgres; nothing here touches a request.
 *
 * Each pass takes a bounded sample so the twins run on a few hundred rows, not the whole ranking tables, and the Postgres function
 * is called with arguments that make its answer exactly the sampled set:
 *  - `threshold`: the ranking score of the 200th-best entity. Every call passes it as `min_ranking_score`, so only entities scoring
 *    at least that are candidates and the twin only needs those rows.
 *  - `types` (3) and `topics` (2): the most populated at or above the threshold, walked with `max_per_*` = 100.
 *  - `spaces` and `userId`: the primer is asked for the first two sampled spaces and one user who has stance votes.
 *
 * A write between the sampling statements and the Postgres call is a benign mismatch, as for the trending reads. Scores are read
 * as float8; a score with more digits than a double holds could differ in its last place, which also shows as a mismatch.
 */

import {type FeedData, Feeds} from "./rankedReads/feeds"
import {nextPrimerClaim, type PrimerData, type PrimerRow, primerCandidates} from "./rankedReads/primer"

export const FEED_READS = [
	"entities_ranked_for_feed",
	"entities_ranked_for_feed_by_type",
	"entities_ranked_for_topics",
	"primer_candidates",
	"next_primer_claim",
] as const
export type FeedRead = (typeof FEED_READS)[number]

export const SAMPLE_CAP = 100

export type FeedsSample = {
	/** The numeric text of the threshold, so Postgres sees exactly what the sample saw. */
	threshold: string
	types: string[]
	topics: string[]
	spaces: string[]
	userId: string | null
	now: Date
}

/** Where the feed shadow gets its rows and the Postgres answers. The default reads Postgres; tests inject. */
export type FeedsShadowSource = {
	/** Null when there is nothing to sample (no ranking rows). */
	sample(spaceSample: string[]): Promise<FeedsSample | null>
	feedData(s: FeedsSample): Promise<FeedData>
	primerData(s: FeedsSample): Promise<PrimerData>
	pgFeed(s: FeedsSample): Promise<string[]>
	pgFeedByType(s: FeedsSample): Promise<string[]>
	pgTopics(s: FeedsSample): Promise<string[]>
	pgPrimer(s: FeedsSample): Promise<PrimerRow[]>
	pgNextPrimer(s: FeedsSample): Promise<string | null>
}

export type FeedsReport = {
	record(read: FeedRead, ok: boolean): void
	fail(read: FeedRead, error: unknown): void
}

const sameIds = (a: string[], b: string[]) => a.length === b.length && a.every((x, i) => x === b[i])
const sameRows = (a: PrimerRow[], b: PrimerRow[]) => JSON.stringify(a) === JSON.stringify(b)

export async function runFeedsShadow(
	source: FeedsShadowSource,
	spaceSample: string[],
	report: FeedsReport,
): Promise<void> {
	let sample: FeedsSample | null
	let feeds: Feeds | undefined
	try {
		sample = await source.sample(spaceSample)
		if (sample === null) return
		feeds = new Feeds(await source.feedData(sample))
	} catch (error) {
		for (const read of FEED_READS) report.fail(read, error)
		return
	}
	const s = sample
	const min = Number(s.threshold)
	const attempt = async (read: FeedRead, check: () => Promise<boolean>) => {
		try {
			report.record(read, await check())
		} catch (error) {
			report.fail(read, error)
		}
	}
	await attempt("entities_ranked_for_feed", async () =>
		sameIds(
			feeds.entitiesRankedForFeed(min, null, null, null, null).map((e) => e.id),
			await source.pgFeed(s),
		),
	)
	await attempt("entities_ranked_for_feed_by_type", async () =>
		sameIds(
			feeds.entitiesRankedForFeedByType(s.types, min, null, null, null, SAMPLE_CAP).map((e) => e.id),
			await source.pgFeedByType(s),
		),
	)
	await attempt("entities_ranked_for_topics", async () =>
		sameIds(
			feeds
				.entitiesRankedForTopics({
					topics: s.topics,
					weights: null,
					types: null,
					min,
					after: null,
					before: null,
					spaces: null,
					maxPerTopic: SAMPLE_CAP,
					debateTaggedClaims: false,
					sortBy: "best",
					matchAll: false,
				})
				.map((e) => e.id),
			await source.pgTopics(s),
		),
	)
	if (s.userId === null) return
	const userId = s.userId
	let primer: PrimerData
	try {
		primer = await source.primerData(s)
	} catch (error) {
		report.fail("primer_candidates", error)
		report.fail("next_primer_claim", error)
		return
	}
	const args = {userId, spaceIds: s.spaces.slice(0, 2), answered: [], skipped: []}
	await attempt("primer_candidates", async () =>
		sameRows(primerCandidates(primer, args, s.now), await source.pgPrimer(s)),
	)
	await attempt(
		"next_primer_claim",
		async () => (nextPrimerClaim(primer, args, s.now)?.id ?? null) === (await source.pgNextPrimer(s)),
	)
}
