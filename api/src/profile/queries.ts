/**
 * Database queries for fetching user profiles.
 *
 * Profiles are derived from personal spaces:
 * 1. Look up the space by wallet address or space ID
 * 2. Fetch the space entity's name (from values) and avatar/cover (from relations)
 *
 * Uses raw SQL for complex correlated subqueries that would be inefficient
 * or verbose with the ORM query builder.
 *
 * Expected indexes for optimal performance:
 * - values(entity_id, property_id, space_id) INCLUDE (text)
 * - relations(from_entity_id, type_id, space_id) INCLUDE (to_entity_id)
 * - spaces(address, type) WHERE type = 'Personal'
 */

import {Data, Effect} from "effect"
import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {ContentIds, SystemIds} from "@graphprotocol/grc-20"

import type {Profile} from "./types"

/**
 * Error type for database query failures.
 * Uses Data.TaggedError for consistency with other Effect errors in the codebase.
 */
export class QueryError extends Data.TaggedError("QueryError")<{
	operation: string
	cause: unknown
}> {}

// Generic database type
type Database = NodePgDatabase<Record<string, unknown>>

// GRC-20 system property IDs
const NAME_PROPERTY = SystemIds.NAME_PROPERTY
const COVER_PROPERTY = SystemIds.COVER_PROPERTY
const AVATAR_PROPERTY = ContentIds.AVATAR_PROPERTY
const IMAGE_URL_PROPERTY = SystemIds.IMAGE_URL_PROPERTY

/**
 * Raw profile data from database query.
 */
type RawProfileRow = {
	space_id: string
	space_address: string
	space_type: string
	entity_name: string | null
	avatar_url: string | null
	cover_url: string | null
}

/**
 * Map a database row to a Profile.
 */
function mapProfileRow(row: RawProfileRow): Profile {
	return {
		id: row.space_id,
		spaceId: row.space_id,
		name: row.entity_name,
		avatarUrl: row.avatar_url,
		coverUrl: row.cover_url,
		address: row.space_address,
		profileLink: `/space/${row.space_id}`,
	}
}

/**
 * Create a default profile for a wallet address that has no space.
 */
export function defaultProfile(address: string, spaceId?: string): Profile {
	return {
		id: address,
		spaceId: spaceId ?? address,
		name: null,
		avatarUrl: null,
		coverUrl: null,
		address,
		profileLink: spaceId ? `/space/${spaceId}` : "",
	}
}

/**
 * SQL fragment for selecting profile fields from a space.
 * Uses ORDER BY id for deterministic results when multiple values exist.
 * Constrains image lookups by space_id to avoid cross-space data leakage.
 */
function profileSelectFields() {
	return sql`
		s.id AS space_id,
		s.address AS space_address,
		s.type AS space_type,
		-- Get name from values (ORDER BY for deterministic results)
		(
			SELECT v.text
			FROM "values" v
			WHERE v.entity_id = s.id
			  AND v.property_id = ${NAME_PROPERTY}::uuid
			  AND v.space_id = s.id
			ORDER BY v.id
			LIMIT 1
		) AS entity_name,
		-- Get avatar URL from relations (constrain by space_id for data integrity)
		(
			SELECT img_val.text
			FROM relations r
			JOIN "values" img_val ON img_val.entity_id = r.to_entity_id
			  AND img_val.property_id = ${IMAGE_URL_PROPERTY}::uuid
			  AND img_val.space_id = COALESCE(r.to_space_id, s.id)
			WHERE r.from_entity_id = s.id
			  AND r.type_id = ${AVATAR_PROPERTY}::uuid
			  AND r.space_id = s.id
			ORDER BY r.id
			LIMIT 1
		) AS avatar_url,
		-- Get cover URL from relations (constrain by space_id for data integrity)
		(
			SELECT img_val.text
			FROM relations r
			JOIN "values" img_val ON img_val.entity_id = r.to_entity_id
			  AND img_val.property_id = ${IMAGE_URL_PROPERTY}::uuid
			  AND img_val.space_id = COALESCE(r.to_space_id, s.id)
			WHERE r.from_entity_id = s.id
			  AND r.type_id = ${COVER_PROPERTY}::uuid
			  AND r.space_id = s.id
			ORDER BY r.id
			LIMIT 1
		) AS cover_url
	`
}

/**
 * Fetch a profile by wallet address.
 *
 * Looks up the user's personal space by address, then fetches the space entity's
 * name, avatar, and cover.
 */
export function getProfileByAddress(db: Database, address: string): Effect.Effect<Profile | null, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			const result = await db.execute<RawProfileRow>(sql`
				SELECT ${profileSelectFields()}
				FROM spaces s
				WHERE s.address = ${address}
				  AND s.type = 'Personal'
				LIMIT 1
			`)

			const row = result.rows[0]
			if (!row) {
				return null
			}

			return mapProfileRow(row)
		},
		catch: (error) => new QueryError({operation: "getProfileByAddress", cause: error}),
	}).pipe(
		Effect.withSpan("queries.getProfileByAddress", {
			attributes: {"query.address": address},
		}),
	)
}

/**
 * Fetch a profile by space ID.
 *
 * Directly looks up the space and fetches its entity's name, avatar, and cover.
 */
export function getProfileBySpaceId(db: Database, spaceId: string): Effect.Effect<Profile | null, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			const result = await db.execute<RawProfileRow>(sql`
				SELECT ${profileSelectFields()}
				FROM spaces s
				WHERE s.id = ${spaceId}::uuid
				LIMIT 1
			`)

			const row = result.rows[0]
			if (!row) {
				return null
			}

			return mapProfileRow(row)
		},
		catch: (error) => new QueryError({operation: "getProfileBySpaceId", cause: error}),
	}).pipe(
		Effect.withSpan("queries.getProfileBySpaceId", {
			attributes: {"query.space_id": spaceId},
		}),
	)
}

/**
 * Batch fetch profiles by space IDs.
 *
 * Efficiently fetches multiple profiles in a single query using proper
 * parameterization (not string concatenation) to prevent SQL injection.
 */
export function getProfilesBySpaceIds(
	db: Database,
	spaceIds: string[],
): Effect.Effect<Map<string, Profile>, QueryError> {
	if (spaceIds.length === 0) {
		return Effect.succeed(new Map())
	}

	return Effect.tryPromise({
		try: async () => {
			// Use proper parameterization via sql.join instead of string concatenation
			// This prevents SQL injection even if UUID validation is bypassed
			const spaceIdParams = sql.join(
				spaceIds.map((id) => sql`${id}::uuid`),
				sql`, `,
			)

			const result = await db.execute<RawProfileRow>(sql`
				SELECT ${profileSelectFields()}
				FROM spaces s
				WHERE s.id = ANY(ARRAY[${spaceIdParams}])
			`)

			// Build map for O(1) lookup
			const profileMap = new Map<string, Profile>()
			for (const row of result.rows) {
				profileMap.set(row.space_id, mapProfileRow(row))
			}

			return profileMap
		},
		catch: (error) => new QueryError({operation: "getProfilesBySpaceIds", cause: error}),
	}).pipe(
		Effect.withSpan("queries.getProfilesBySpaceIds", {
			attributes: {"query.space_ids_count": spaceIds.length},
		}),
	)
}
