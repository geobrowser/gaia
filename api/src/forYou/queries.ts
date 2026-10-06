/**
 * The reads behind `/internal/for-you`. Four independent statements, run in parallel, so the
 * route costs one round trip of wall time. All of them read the private `personalization` schema
 * (migrations 0100 and 0102), which the public GraphQL API cannot reach.
 */

import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {type CandidateRow, type ForYouConfig, isExclusionReason, type TopicWeight} from "./rank"

export type Database = Pick<NodePgDatabase<Record<string, unknown>>, "execute">

const NAME_PROPERTY_ID = "a126ca53-0c8e-48d5-b888-82c734c38935"

export type ForYouSettings = {config: ForYouConfig; revision: number; tauSeconds: number}

export type FeedExperiment = {
	id: string
	arms: [string, string]
	interleaved: boolean
	assignment: "member" | "hash"
}

export type ForYouInputs = {
	settings: ForYouSettings
	weights: TopicWeight[]
	topicNames: Map<string, string>
	candidates: CandidateRow[]
	experiment: FeedExperiment | null
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
	args: {userId: string; candidateIds: string[]; asOf: Date},
): Promise<ForYouInputs> {
	const [settings, {weights, topicNames}, candidates, experiment] = await Promise.all([
		readSettings(db),
		readWeights(db, args.userId, args.asOf),
		readCandidates(db, args.userId, args.candidateIds),
		readExperiment(db, args.userId),
	])
	return {settings, weights, topicNames, candidates, experiment}
}
