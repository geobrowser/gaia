/**
 * Database queries for fetching user profiles.
 *
 * Profiles are derived from personal spaces:
 * 1. Look up the space by wallet address or space ID
 * 2. Fetch the space entity's name (from values) and avatar (from relations)
 *
 * Uses raw SQL for complex correlated subqueries that would be inefficient
 * or verbose with the ORM query builder.
 *
 * Expected indexes for optimal performance:
 * - values(entity_id, property_id, space_id) INCLUDE (text)
 * - relations(from_entity_id, type_id, space_id) INCLUDE (to_entity_id)
 * - spaces(address, type) WHERE type = 'Personal'
 */

import {ContentIds, SystemIds} from "@graphprotocol/grc-20"
import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Data, Effect} from "effect"
import {normalizeUuid} from "../utils/uuid"
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
const AVATAR_PROPERTY = ContentIds.AVATAR_PROPERTY
const IMAGE_URL_PROPERTY = SystemIds.IMAGE_URL_PROPERTY

/**
 * Raw profile data from database query.
 */
type RawProfileRow = {
	space_id: string
	space_address: string
	entity_name: string | null
	avatar_url: string | null
}

/**
 * Map a database row to a Profile.
 * Normalizes space_id to undashed format for consistent API responses.
 */
function mapProfileRow(row: RawProfileRow): Profile {
	return {
		spaceId: normalizeUuid(row.space_id),
		name: row.entity_name,
		avatarUrl: row.avatar_url,
		address: row.space_address,
	}
}

/**
 * Create a default profile for a wallet address that has no space.
 */
export function defaultProfile(address: string, spaceId?: string): Profile {
	return {
		spaceId: spaceId ?? address,
		name: null,
		avatarUrl: null,
		address,
	}
}

// SystemIds for finding space front page entity
const TYPES_RELATION = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"
const SPACE_TYPE = "362c1dbd-dc64-44bb-a3c4-652f38a642d7"

/**
 * SQL fragment for selecting profile fields from a space.
 * Finds the front page entity (entity with Types relation to SPACE_TYPE),
 * then gets name and avatar from that entity.
 */
function profileSelectFields() {
	return sql`
		s.id AS space_id,
		s.address AS space_address,
		(
			SELECT v.text
			FROM "values" v
			WHERE v.entity_id = (
				SELECT r.from_entity_id
				FROM relations r
				WHERE r.space_id = s.id
				  AND r.type_id = ${TYPES_RELATION}::uuid
				  AND r.to_entity_id = ${SPACE_TYPE}::uuid
				LIMIT 1
			)
			  AND v.property_id = ${NAME_PROPERTY}::uuid
			  AND v.space_id = s.id
			LIMIT 1
		) AS entity_name,
		(
			SELECT img_val.text
			FROM relations r
			JOIN "values" img_val ON img_val.entity_id = r.to_entity_id
			  AND img_val.property_id = ${IMAGE_URL_PROPERTY}::uuid
			  AND img_val.space_id = s.id
			WHERE r.from_entity_id = (
				SELECT r2.from_entity_id
				FROM relations r2
				WHERE r2.space_id = s.id
				  AND r2.type_id = ${TYPES_RELATION}::uuid
				  AND r2.to_entity_id = ${SPACE_TYPE}::uuid
				LIMIT 1
			)
			  AND r.type_id = ${AVATAR_PROPERTY}::uuid
			  AND r.space_id = s.id
			LIMIT 1
		) AS avatar_url
	`
}

/**
 * Fetch a profile by wallet address.
 *
 * Looks up the user's personal space by address, then fetches the space entity's
 * name and avatar.
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
 * Directly looks up the space and fetches its entity's name and avatar.
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

			// Build map for O(1) lookup (keyed by undashed UUID for consistent lookups)
			const profileMap = new Map<string, Profile>()
			for (const row of result.rows) {
				profileMap.set(normalizeUuid(row.space_id), mapProfileRow(row))
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
