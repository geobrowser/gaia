/**
 * The reads behind `/internal/pair-fit` (GEO-3224). Three independent statements run in parallel,
 * then one for the names of whatever the reasons point at. Nothing is written: pair scores are
 * computed on request and never stored.
 *
 * Positions are compared IN SQL and only whether two people are on opposite sides leaves the
 * database, so no per-user stance ever reaches this process, its logs or its response.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {parseUuidArray} from "../forYou/queries"
import type {OwnTopicWeight, SharedClaim} from "./score"

export type {Database}

const NAME_PROPERTY_ID = "a126ca53-0c8e-48d5-b888-82c734c38935"
const TOPICS_RELATION_TYPE_ID = "806d52bc-27e9-4c91-93c0-57978b093351"

const num = (value: unknown): number | null => {
	if (value === null || value === undefined) return null
	const n = Number(value)
	return Number.isFinite(n) ? n : null
}

function rowsOf(result: unknown): Record<string, unknown>[] {
	return ((result as {rows?: Record<string, unknown>[]}).rows ?? []) as Record<string, unknown>[]
}

const uuidArray = (ids: string[]) => `{${ids.join(",")}}`

/** Each person's OWN positive topic weights (GEO-3088's read, without spreading), keyed by user. */
export async function readOwnWeights(db: Database, userIds: string[]): Promise<Map<string, OwnTopicWeight[]>> {
	const result = await db.execute(sql`
		SELECT u.id AS user_id, w.topic_id, w.own_weight
		FROM unnest(${uuidArray(userIds)}::uuid[]) AS u(id)
		CROSS JOIN LATERAL personalization.user_topic_weights(
		  u.id, (SELECT max_topics FROM personalization.for_you_config), now()) w
		WHERE w.own_weight > 0
	`)
	const byUser = new Map<string, OwnTopicWeight[]>()
	for (const row of rowsOf(result)) {
		const userId = String(row.user_id)
		const list = byUser.get(userId) ?? []
		list.push({topicId: String(row.topic_id), weight: num(row.own_weight) ?? 0})
		byUser.set(userId, list)
	}
	return byUser
}

/**
 * Claims the user and each candidate both hold a stance on (Agree/Disagree, vote_kind 1; a removal,
 * vote_type 2, is no position), whether they are on opposite sides, and each claim's topics. A
 * stance cast in several spaces counts once, at its latest.
 */
export async function readSharedClaims(
	db: Database,
	userId: string,
	candidateIds: string[],
): Promise<Map<string, SharedClaim[]>> {
	const result = await db.execute(sql`
		WITH held AS (
		  SELECT DISTINCT ON (uv.user_id, uv.object_id) uv.user_id, uv.object_id, uv.vote_type
		  FROM public.user_votes uv
		  WHERE uv.user_id = ANY(${uuidArray([userId, ...candidateIds])}::uuid[])
		    AND uv.object_type = 0 AND uv.vote_kind = 1 AND uv.vote_type IN (0, 1)
		  ORDER BY uv.user_id, uv.object_id, uv.voted_at DESC
		)
		SELECT c.user_id AS candidate_id, c.object_id AS claim_id, (c.vote_type <> u.vote_type) AS opposite,
		       coalesce((
		         SELECT array_agg(DISTINCT r.to_entity_id ORDER BY r.to_entity_id)
		         FROM public.relations r
		         WHERE r.type_id = ${TOPICS_RELATION_TYPE_ID}::uuid AND r.from_entity_id = c.object_id
		       ), '{}'::uuid[]) AS topic_ids
		FROM held u
		JOIN held c ON c.object_id = u.object_id AND c.user_id <> u.user_id
		WHERE u.user_id = ${userId}::uuid
	`)
	const byCandidate = new Map<string, SharedClaim[]>()
	for (const row of rowsOf(result)) {
		const candidateId = String(row.candidate_id)
		const list = byCandidate.get(candidateId) ?? []
		list.push({
			claimId: String(row.claim_id),
			opposite: row.opposite === true,
			topicIds: parseUuidArray(row.topic_ids),
		})
		byCandidate.set(candidateId, list)
	}
	return byCandidate
}

/** Each candidate's account weight (0101) and whether it is on the exclusion list (test or junk). */
export async function readAccounts(
	db: Database,
	candidateIds: string[],
): Promise<Map<string, {weight: number | null; excluded: boolean}>> {
	const result = await db.execute(sql`
		SELECT u.id AS user_id, w.weight, (x.user_id IS NOT NULL) AS excluded
		FROM unnest(${uuidArray(candidateIds)}::uuid[]) AS u(id)
		LEFT JOIN public.account_weights w ON w.user_id = u.id
		LEFT JOIN public.account_exclusions x ON x.user_id = u.id
	`)
	const byUser = new Map<string, {weight: number | null; excluded: boolean}>()
	for (const row of rowsOf(result)) {
		byUser.set(String(row.user_id), {weight: num(row.weight), excluded: row.excluded === true})
	}
	return byUser
}

/** Names of the claims and topics the reasons name. */
export async function readNames(db: Database, entityIds: string[]): Promise<Map<string, string>> {
	if (entityIds.length === 0) return new Map()
	const result = await db.execute(sql`
		SELECT DISTINCT ON (v.entity_id) v.entity_id, v.text AS name
		FROM public.values v
		WHERE v.entity_id = ANY(${uuidArray(entityIds)}::uuid[]) AND v.property_id = ${NAME_PROPERTY_ID}::uuid
		  AND v.text IS NOT NULL AND length(trim(v.text)) > 0
		ORDER BY v.entity_id, v.space_id
	`)
	const names = new Map<string, string>()
	for (const row of rowsOf(result)) names.set(String(row.entity_id), String(row.name).trim())
	return names
}
