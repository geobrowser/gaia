/**
 * The reads behind `/internal/for-you`. Up to five independent statements, run in parallel, so the
 * route costs one round trip of wall time. All of them read the private `personalization` schema
 * (migrations 0100 and 0102), which the public GraphQL API cannot reach.
 */

import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {
	type CandidateRow,
	type CandidateSource,
	type ForYouConfig,
	interestBoost,
	isExclusionReason,
	type TopicWeight,
} from "./rank"

export type Database = Pick<NodePgDatabase<Record<string, unknown>>, "execute">

const NAME_PROPERTY_ID = "a126ca53-0c8e-48d5-b888-82c734c38935"

export type ForYouSettings = {config: ForYouConfig; revision: number; tauSeconds: number}

export type FeedExperiment = {
	id: string
	arms: [string, string]
	interleaved: boolean
	assignment: "member" | "hash"
}

/**
 * GEO-3299: what to walk the user's topics for, so For you is not limited to Best's window. The
 * filters are Explore's, applied the way `entities_ranked_for_topics` applies them for topic pages.
 */
export type ForYouPool = {
	typeIds: string[]
	debateTaggedClaims: boolean
	/** The spaces the reader may see; null for no narrowing. */
	spaceIds: string[] | null
	/** Epoch seconds as text, like the GraphQL `createdAfter`; null for all time. */
	createdAfter: string | null
	/** How many topic candidates to take, by score plus boost, before exclusions. */
	depth: number
}

export type PoolSummary = {depth: number; best: number; topics: number; overlap: number}

/**
 * GEO-3299 step 3: what this user was shown on Explore since `since`, from the hourly feed signals
 * sync (0109), and how far that sync has read.
 */
export type SyncedSeen = {
	/** Views per item id (dashed), counted in whole UTC days as `feed_user_seen` counts them. */
	counts: Map<string, number>
	/** The last successful sync's watermark; null before the first one. */
	watermark: Date | null
}

export type ForYouInputs = {
	settings: ForYouSettings
	weights: TopicWeight[]
	topicNames: Map<string, string>
	candidates: CandidateRow[]
	experiment: FeedExperiment | null
	pool: PoolSummary | null
	/** Null when the request did not ask for seen hiding. */
	seen: SyncedSeen | null
}

const num = (value: unknown): number | null => {
	if (value === null || value === undefined) return null
	const n = Number(value)
	return Number.isFinite(n) ? n : null
}

function rowsOf(result: unknown): Record<string, unknown>[] {
	return ((result as {rows?: Record<string, unknown>[]}).rows ?? []) as Record<string, unknown>[]
}

export async function readSettings(db: Database): Promise<ForYouSettings> {
	const result = await db.execute(sql`
		SELECT c.max_boost_days, c.interest_half_saturation, c.exploration_share, c.max_topics,
		       r.revision,
		       (SELECT tau_seconds FROM public.entity_ranking_config LIMIT 1)::double precision AS tau_seconds
		FROM personalization.for_you_config c
		CROSS JOIN personalization.feed_config_revision r
	`)
	const row = rowsOf(result)[0]
	if (!row) throw new Error("for_you_config or feed_config_revision has no row")
	return {
		config: {
			maxBoostDays: num(row.max_boost_days) ?? 0,
			interestHalfSaturation: num(row.interest_half_saturation) ?? 1,
			explorationShare: num(row.exploration_share) ?? 0,
			maxTopics: num(row.max_topics) ?? 50,
		},
		revision: num(row.revision) ?? 0,
		tauSeconds: num(row.tau_seconds) ?? 100000,
	}
}

/** The user's weighted topics (GEO-3088's read), with each topic's name and its spread source's. */
export async function readWeights(
	db: Database,
	userId: string,
	asOf: Date,
): Promise<{weights: TopicWeight[]; topicNames: Map<string, string>}> {
	const result = await db.execute(sql`
		SELECT w.topic_id, w.weight, w.own_weight, w.related_weight, w.top_kind, w.top_kind_count,
		       w.top_object_id, w.related_via_topic_id, tn.name AS topic_name, vn.name AS via_name
		FROM personalization.user_topic_weights(
		       ${userId}::uuid,
		       (SELECT max_topics FROM personalization.for_you_config),
		       ${asOf.toISOString()}::timestamptz) w
		LEFT JOIN LATERAL (
		  SELECT v.text AS name FROM public.values v
		  WHERE v.entity_id = w.topic_id AND v.property_id = ${NAME_PROPERTY_ID}::uuid
		    AND v.text IS NOT NULL AND length(trim(v.text)) > 0
		  ORDER BY v.space_id LIMIT 1
		) tn ON true
		LEFT JOIN LATERAL (
		  SELECT v.text AS name FROM public.values v
		  WHERE v.entity_id = w.related_via_topic_id AND v.property_id = ${NAME_PROPERTY_ID}::uuid
		    AND v.text IS NOT NULL AND length(trim(v.text)) > 0
		  ORDER BY v.space_id LIMIT 1
		) vn ON true
	`)
	const weights: TopicWeight[] = []
	const topicNames = new Map<string, string>()
	for (const row of rowsOf(result)) {
		const topicId = String(row.topic_id)
		weights.push({
			topicId,
			weight: num(row.weight) ?? 0,
			ownWeight: num(row.own_weight) ?? 0,
			relatedWeight: num(row.related_weight) ?? 0,
			topKind: String(row.top_kind),
			topKindCount: num(row.top_kind_count),
			topObjectId: row.top_object_id ? String(row.top_object_id) : null,
			relatedViaTopicId: row.related_via_topic_id ? String(row.related_via_topic_id) : null,
		})
		if (typeof row.topic_name === "string") topicNames.set(topicId, row.topic_name.trim())
		if (row.related_via_topic_id && typeof row.via_name === "string")
			topicNames.set(String(row.related_via_topic_id), row.via_name.trim())
	}
	return {weights, topicNames}
}

export async function readCandidates(db: Database, userId: string, candidateIds: string[]): Promise<CandidateRow[]> {
	const result = await db.execute(sql`
		SELECT entity_id, ranking_score, topic_ids, excluded
		FROM personalization.for_you_candidates(${userId}::uuid, ${`{${candidateIds.join(",")}}`}::uuid[])
	`)
	return rowsOf(result).map((row) => ({
		entityId: String(row.entity_id),
		rankingScore: num(row.ranking_score),
		topicIds: parseUuidArray(row.topic_ids),
		excluded: isExclusionReason(row.excluded) ? row.excluded : null,
	}))
}

const isSource = (value: unknown): value is CandidateSource =>
	value === "best" || value === "topics" || value === "both"

const uuidArray = (ids: readonly string[]) => `{${ids.join(",")}}`

/**
 * Best's window plus the user's topic candidates, in that order, as `for_you_candidates` rows.
 *
 * The topic walk is `entities_ranked_for_topics` with each topic weighted by the boost the ranking
 * will give it, so its cap keeps the `depth` candidates with the highest best + boost: exactly the
 * ones the ranking would put first. One statement, so the walk costs no extra round trip.
 */
export async function readPooledCandidates(
	db: Database,
	args: {
		userId: string
		bestIds: string[]
		pool: ForYouPool
		weights: readonly TopicWeight[]
		settings: ForYouSettings
	},
): Promise<{candidates: CandidateRow[]; summary: PoolSummary}> {
	const topics = args.weights.filter((w) => w.weight > 0)
	const boosts = topics.map((w) => interestBoost(w.weight, args.settings.config, args.settings.tauSeconds))
	const {pool} = args
	const result = await db.execute(sql`
		WITH topic AS (
		  SELECT t.id, t.ordinality AS ord
		  FROM public.entities_ranked_for_topics(
		         topic_ids            => ${uuidArray(topics.map((w) => w.topicId))}::uuid[],
		         topic_weights        => ${`{${boosts.join(",")}}`}::double precision[],
		         type_ids             => ${uuidArray(pool.typeIds)}::uuid[],
		         created_after        => ${pool.createdAfter}::text,
		         space_ids            => ${pool.spaceIds ? uuidArray(pool.spaceIds) : null}::uuid[],
		         max_per_topic        => ${pool.depth}::integer,
		         debate_tagged_claims => ${pool.debateTaggedClaims}::boolean
		       ) WITH ORDINALITY AS t
		  WHERE ${topics.length} > 0
		),
		best AS (
		  SELECT b.id, b.ord FROM unnest(${uuidArray(args.bestIds)}::uuid[]) WITH ORDINALITY AS b(id, ord)
		),
		pooled AS (
		  SELECT id, ord FROM best
		  UNION ALL
		  SELECT t.id, (SELECT count(*) FROM best) + t.ord FROM topic t
		  WHERE NOT EXISTS (SELECT 1 FROM best b WHERE b.id = t.id)
		)
		SELECT c.entity_id, c.ranking_score, c.topic_ids, c.excluded,
		       CASE WHEN NOT EXISTS (SELECT 1 FROM topic t WHERE t.id = c.entity_id) THEN 'best'
		            WHEN EXISTS (SELECT 1 FROM best b WHERE b.id = c.entity_id) THEN 'both'
		            ELSE 'topics' END AS source
		FROM personalization.for_you_candidates(
		       ${args.userId}::uuid, ARRAY(SELECT id FROM pooled ORDER BY ord)) AS c
	`)
	const candidates: CandidateRow[] = rowsOf(result).map((row) => ({
		entityId: String(row.entity_id),
		rankingScore: num(row.ranking_score),
		topicIds: parseUuidArray(row.topic_ids),
		excluded: isExclusionReason(row.excluded) ? row.excluded : null,
		source: isSource(row.source) ? row.source : "best",
	}))
	const count = (source: CandidateSource) => candidates.filter((c) => c.source === source).length
	return {
		candidates,
		summary: {
			depth: pool.depth,
			best: args.bestIds.length,
			topics: count("topics") + count("both"),
			overlap: count("both"),
		},
	}
}

export async function readExperiment(db: Database, userId: string): Promise<FeedExperiment | null> {
	const result = await db.execute(sql`
		SELECT experiment_id, arm_a, arm_b, interleaved, assignment
		FROM personalization.feed_experiment_for_user(${userId}::uuid)
	`)
	const row = rowsOf(result)[0]
	if (!row) return null
	return {
		id: String(row.experiment_id),
		arms: [String(row.arm_a), String(row.arm_b)],
		interleaved: row.interleaved === true,
		assignment: row.assignment === "member" ? "member" : "hash",
	}
}

/**
 * One statement: the user's seen items since `since` (one range scan of the primary key), and the
 * watermark as a column of every row. The left join keeps a row when nothing was seen, so the
 * watermark always comes back.
 */
export async function readSeen(db: Database, userId: string, since: Date): Promise<SyncedSeen> {
	const result = await db.execute(sql`
		SELECT s.item_id, s.seen_count,
		       (SELECT max(r.data_through) FROM personalization.feed_signal_runs r
		        WHERE r.status = 'succeeded') AS watermark
		FROM (SELECT 1) one
		LEFT JOIN personalization.feed_user_seen(${userId}::uuid, ${since.toISOString()}::timestamptz) s ON true
	`)
	const counts = new Map<string, number>()
	let watermark: Date | null = null
	for (const row of rowsOf(result)) {
		if (row.watermark !== null && row.watermark !== undefined) {
			const at = new Date(row.watermark as string | Date)
			if (!Number.isNaN(at.getTime())) watermark = at
		}
		const views = num(row.seen_count)
		if (row.item_id && views !== null && views > 0) counts.set(String(row.item_id), views)
	}
	return {counts, watermark}
}

/** node-postgres returns uuid[] as a JS array, or as `{a,b}` text when the type parser is absent. */
export function parseUuidArray(value: unknown): string[] {
	if (Array.isArray(value)) return value.map(String)
	if (typeof value === "string") {
		const inner = value.replace(/^\{|\}$/g, "")
		return inner ? inner.split(",").map((s) => s.trim()) : []
	}
	return []
}

export async function readForYouInputs(
	db: Database,
	args: {userId: string; candidateIds: string[]; asOf: Date; pool?: ForYouPool | null; seenSince?: Date | null},
): Promise<ForYouInputs> {
	const {pool, seenSince} = args
	// Independent of everything else, so it runs alongside the first round of reads.
	const seenRead = seenSince ? readSeen(db, args.userId, seenSince) : Promise.resolve(null)
	if (!pool) {
		const [settings, {weights, topicNames}, candidates, experiment, seen] = await Promise.all([
			readSettings(db),
			readWeights(db, args.userId, args.asOf),
			readCandidates(db, args.userId, args.candidateIds),
			readExperiment(db, args.userId),
			seenRead,
		])
		return {settings, weights, topicNames, candidates, experiment, pool: null, seen}
	}
	// Pooled: the topic walk needs the weights and the boost settings, so it runs second. A user
	// with no weights is served Best by the caller, so there is nothing to walk or rank.
	const [settings, {weights, topicNames}, experiment, seen] = await Promise.all([
		readSettings(db),
		readWeights(db, args.userId, args.asOf),
		readExperiment(db, args.userId),
		seenRead,
	])
	if (weights.length === 0) {
		return {
			settings,
			weights,
			topicNames,
			candidates: [],
			experiment,
			pool: {depth: pool.depth, best: args.candidateIds.length, topics: 0, overlap: 0},
			seen,
		}
	}
	const {candidates, summary} = await readPooledCandidates(db, {
		userId: args.userId,
		bestIds: args.candidateIds,
		pool,
		weights,
		settings,
	})
	return {settings, weights, topicNames, candidates, experiment, pool: summary, seen}
}
