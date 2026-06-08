/**
 * v2 diff response types.
 *
 * Extends v1 types with media-URL inlining: when a relation's target is an
 * IMAGE_TYPE or VIDEO_TYPE entity, the v2 enrichment resolves its URL value
 * and attaches `imageUrl` / `videoUrl` on the relation's before/after.
 *
 * Backward-compatible with v1: every v2 response is also a valid v1 response,
 * since the added fields are optional.
 */

import type {NormalizedUuid} from "../utils/uuid"
import type {DiffResponse, RelationChange} from "../versioned/types"

/**
 * v2 relation-change endpoint payload.
 *
 * Adds optional `imageUrl` / `videoUrl` on the before/after sub-objects.
 * Only one of imageUrl/videoUrl is set per side, matching the target entity's type.
 */
export interface RelationChangeV2 extends Omit<RelationChange, "before" | "after"> {
	before?:
		| (NonNullable<RelationChange["before"]> & {
				imageUrl?: string | null
				videoUrl?: string | null
		  })
		| null
	after?:
		| (NonNullable<RelationChange["after"]> & {
				imageUrl?: string | null
				videoUrl?: string | null
		  })
		| null
}

/**
 * v2 single-entity diff response. Identical to v1 except RelationChange
 * may carry inlined media URLs.
 */
export type DiffResponseV2 = Omit<DiffResponse, "relations"> & {
	relations: RelationChangeV2[]
}

/**
 * Internal: a resolved media-entity URL bucket used during enrichment.
 */
export interface MediaEntity {
	entityId: NormalizedUuid
	url: string
	mediaType: "image" | "video"
}
