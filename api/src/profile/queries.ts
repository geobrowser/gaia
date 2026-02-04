/**
 * Database queries for fetching user profiles.
 *
 * Profiles are derived from personal spaces:
 * 1. Look up the space by wallet address or space ID
 * 2. Fetch the space entity's name (from values) and avatar/cover (from relations)
 */

import {Effect} from "effect"
import {sql} from "drizzle-orm"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {ContentIds, SystemIds} from "@graphprotocol/grc-20"

import type {Profile} from "./types"

// Error type for database query failures
export class QueryError {
	readonly _tag = "QueryError"
	constructor(
		readonly operation: string,
		readonly cause: unknown,
	) {}
}

// Generic database type
type Database = NodePgDatabase<Record<string, unknown>>

// GRC-20 system property IDs
const NAME_PROPERTY = SystemIds.NAME_PROPERTY
const COVER_PROPERTY = SystemIds.COVER_PROPERTY
const AVATAR_PROPERTY = ContentIds.AVATAR_PROPERTY

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
 * Fetch a profile by wallet address.
 *
 * Looks up the user's personal space by address, then fetches the space entity's
 * name, avatar, and cover.
 */
export function getProfileByAddress(db: Database, address: string): Effect.Effect<Profile | null, QueryError> {
	return Effect.tryPromise({
		try: async () => {
			const result = await db.execute<RawProfileRow>(sql`
				SELECT
					s.id AS space_id,
					s.address AS space_address,
					s.type AS space_type,
					-- Get name from values
					(
						SELECT v.text
						FROM "values" v
						WHERE v.entity_id = s.id
						  AND v.property_id = ${NAME_PROPERTY}::uuid
						  AND v.space_id = s.id
						LIMIT 1
					) AS entity_name,
					-- Get avatar URL from relations (AVATAR_PROPERTY relation -> to_entity's IMAGE_URL_PROPERTY value)
					(
						SELECT img_val.text
						FROM relations r
						JOIN "values" img_val ON img_val.entity_id = r.to_entity_id
						  AND img_val.property_id = ${SystemIds.IMAGE_URL_PROPERTY}::uuid
						WHERE r.from_entity_id = s.id
						  AND r.type_id = ${AVATAR_PROPERTY}::uuid
						  AND r.space_id = s.id
						LIMIT 1
					) AS avatar_url,
					-- Get cover URL from relations (COVER_PROPERTY relation -> to_entity's IMAGE_URL_PROPERTY value)
					(
						SELECT img_val.text
						FROM relations r
						JOIN "values" img_val ON img_val.entity_id = r.to_entity_id
						  AND img_val.property_id = ${SystemIds.IMAGE_URL_PROPERTY}::uuid
						WHERE r.from_entity_id = s.id
						  AND r.type_id = ${COVER_PROPERTY}::uuid
						  AND r.space_id = s.id
						LIMIT 1
					) AS cover_url
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
		catch: (error) => new QueryError("getProfileByAddress", error),
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
				SELECT
					s.id AS space_id,
					s.address AS space_address,
					s.type AS space_type,
					-- Get name from values
					(
						SELECT v.text
						FROM "values" v
						WHERE v.entity_id = s.id
						  AND v.property_id = ${NAME_PROPERTY}::uuid
						  AND v.space_id = s.id
						LIMIT 1
					) AS entity_name,
					-- Get avatar URL from relations
					(
						SELECT img_val.text
						FROM relations r
						JOIN "values" img_val ON img_val.entity_id = r.to_entity_id
						  AND img_val.property_id = ${SystemIds.IMAGE_URL_PROPERTY}::uuid
						WHERE r.from_entity_id = s.id
						  AND r.type_id = ${AVATAR_PROPERTY}::uuid
						  AND r.space_id = s.id
						LIMIT 1
					) AS avatar_url,
					-- Get cover URL from relations
					(
						SELECT img_val.text
						FROM relations r
						JOIN "values" img_val ON img_val.entity_id = r.to_entity_id
						  AND img_val.property_id = ${SystemIds.IMAGE_URL_PROPERTY}::uuid
						WHERE r.from_entity_id = s.id
						  AND r.type_id = ${COVER_PROPERTY}::uuid
						  AND r.space_id = s.id
						LIMIT 1
					) AS cover_url
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
		catch: (error) => new QueryError("getProfileBySpaceId", error),
	}).pipe(
		Effect.withSpan("queries.getProfileBySpaceId", {
			attributes: {"query.space_id": spaceId},
		}),
	)
}

/**
 * Batch fetch profiles by space IDs.
 *
 * Efficiently fetches multiple profiles in a single query.
 * Returns profiles in the same order as the input array.
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
			// Build array literal for PostgreSQL
			const spaceIdsArray = `{${spaceIds.join(",")}}`

			const result = await db.execute<RawProfileRow>(sql`
				SELECT
					s.id AS space_id,
					s.address AS space_address,
					s.type AS space_type,
					-- Get name from values
					(
						SELECT v.text
						FROM "values" v
						WHERE v.entity_id = s.id
						  AND v.property_id = ${NAME_PROPERTY}::uuid
						  AND v.space_id = s.id
						LIMIT 1
					) AS entity_name,
					-- Get avatar URL from relations
					(
						SELECT img_val.text
						FROM relations r
						JOIN "values" img_val ON img_val.entity_id = r.to_entity_id
						  AND img_val.property_id = ${SystemIds.IMAGE_URL_PROPERTY}::uuid
						WHERE r.from_entity_id = s.id
						  AND r.type_id = ${AVATAR_PROPERTY}::uuid
						  AND r.space_id = s.id
						LIMIT 1
					) AS avatar_url,
					-- Get cover URL from relations
					(
						SELECT img_val.text
						FROM relations r
						JOIN "values" img_val ON img_val.entity_id = r.to_entity_id
						  AND img_val.property_id = ${SystemIds.IMAGE_URL_PROPERTY}::uuid
						WHERE r.from_entity_id = s.id
						  AND r.type_id = ${COVER_PROPERTY}::uuid
						  AND r.space_id = s.id
						LIMIT 1
					) AS cover_url
				FROM spaces s
				WHERE s.id = ANY(${spaceIdsArray}::uuid[])
			`)

			// Build map for O(1) lookup
			const profileMap = new Map<string, Profile>()
			for (const row of result.rows) {
				profileMap.set(row.space_id, mapProfileRow(row))
			}

			return profileMap
		},
		catch: (error) => new QueryError("getProfilesBySpaceIds", error),
	}).pipe(
		Effect.withSpan("queries.getProfilesBySpaceIds", {
			attributes: {"query.space_ids_count": spaceIds.length},
		}),
	)
}
