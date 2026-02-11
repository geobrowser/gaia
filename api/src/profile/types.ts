/**
 * Profile types matching the geogenesis web model.
 *
 * A profile represents a user's identity in the knowledge graph,
 * derived from their personal space entity.
 */

/**
 * A user profile in the knowledge graph.
 *
 * Profiles are derived from personal spaces - each user's wallet address
 * is mapped to a personal space, and the space's entity contains the
 * profile data (name, avatar).
 */
export type Profile = {
	/** The entity ID of the space's front page entity (the profile entity) */
	entityId: string | null
	/** The user's personal space ID */
	spaceId: string
	/** Display name from the NAME_PROPERTY value */
	name: string | null
	/** Avatar image URL from the AVATAR_PROPERTY relation */
	avatarUrl: string | null
	/** The user's wallet address (0x prefixed) */
	address: string
}
