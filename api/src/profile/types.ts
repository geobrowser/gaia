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
 * profile data (name, avatar, cover).
 */
export type Profile = {
	/** The user's personal space ID */
	spaceId: string
	/** Display name from the NAME_PROPERTY value */
	name: string | null
	/** Avatar image URL from the AVATAR_PROPERTY relation */
	avatarUrl: string | null
	/** Cover image URL from the COVER_PROPERTY relation */
	coverUrl: string | null
	/** The user's wallet address (0x prefixed) */
	address: string
	/** Link to the user's space */
	profileLink: string
}


