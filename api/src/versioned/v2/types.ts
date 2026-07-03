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

import type {NormalizedUuid} from "../../utils/uuid"
import type {DiffResponse, DynamicGroupItem, GroupedEntityDiff, ProposalStatus, RelationChange} from "../types"

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

// ============================================================================
// Proposal diff (v2)
// ============================================================================

/**
 * A single enriched, context-aware entity diff in a proposal response.
 *
 * Same enrichment as the entity-diff endpoint (`DiffResponseV2`) minus the
 * per-edit metadata: relations carry media URLs (`RelationChangeV2`), blocks are
 * folded under `blocks[]` with their values/relations/config, names are
 * resolved, and dynamic group keys are spread at the entity level. Block and
 * media "property" child entities are folded into their parent rather than
 * returned as separate top-level entries.
 */
export type EntityDiffV2 = Omit<GroupedEntityDiff, "relations" | "groups"> & {
	relations: RelationChangeV2[]
} & Record<NormalizedUuid, DynamicGroupItem[]>

export interface PaginationV2 {
	cursor: string | null
	hasMore: boolean
	totalEntities: number
}

/** Response for `GET /v2/versioned/proposals/:id/diff`. */
export interface PaginatedProposalDiffV2 {
	proposalId: NormalizedUuid
	spaceId: NormalizedUuid
	proposalStatus: ProposalStatus
	entities: EntityDiffV2[]
	pagination: PaginationV2
}

/** Response for `GET /v2/versioned/proposal-groups/diff`. */
export interface PaginatedGroupedProposalDiffV2 {
	proposalIds: NormalizedUuid[]
	spaceId: NormalizedUuid
	mode: "active" | "historical"
	entities: EntityDiffV2[]
	pagination: PaginationV2
}

/**
 * Response for `POST /v2/versioned/review`.
 *
 * Same enriched `EntityDiffV2[]` shape as the proposal diff, for a space's
 * unpublished local edit diffed against current live state. No proposal/edit
 * metadata since there is no persisted proposal.
 */
export interface PaginatedReviewDiffV2 {
	spaceId: NormalizedUuid
	entities: EntityDiffV2[]
	pagination: PaginationV2
}
