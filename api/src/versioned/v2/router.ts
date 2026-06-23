/**
 * v2 versioned router.
 *
 * Mounted at /v2/versioned. Adds enrichment on top of v1 logic without changing
 * v1 behavior. This first slice only ships the entity-diff endpoint with media
 * URL inlining; future slices will add name resolution across nested groups,
 * media-property entity filtering, data-block config merging, etc.
 */

import {decodeEditAuto} from "@geoprotocol/grc-20"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Data, Effect, Either} from "effect"
import {Hono} from "hono"
import {getProfilesBySpaceIds} from "../../profile/queries"
import type {Profile} from "../../profile/types"
import type {AppRuntime} from "../../services/runtime"
import {isValidUuid, normalizeUuid, toDashedUuid} from "../../utils/uuid"
import {diffGroupedEntitySnapshots} from "../diff"
import {EditDecodeError} from "../proposal-diff"
import {getGroupedEntitySnapshotAtVersion, type QueryError, resolveVersionKey} from "../queries"
import {mapGroupedProposalError, validateGroupedRequest} from "../router"
import type {GroupedEntitySnapshot} from "../types"
import {enrichWithMediaUrls} from "./enrich"
import {enrichBlockConfig} from "./enrich-block-config"
import {enrichBlocks} from "./enrich-blocks"
import {enrichNames} from "./enrich-names"
import {computeGroupedProposalDiffV2, computeProposalDiffV2, computeReviewDiffV2} from "./proposal-diff"
import type {DiffResponseV2} from "./types"

/**
 * Safety bound on a single review request's op count (untrusted input). Generous
 * headroom over real edits (largest observed published edit ≈ 2.8k ops). A
 * tighter, entity-aware cap is tracked in PRO-71.
 */
const MAX_REVIEW_OPS = 10_000

type AppEnv = {
	Variables: {
		requestId: string
	}
}

type Database = NodePgDatabase<Record<string, unknown>>

class ValidationError extends Data.TaggedError("ValidationError")<{message: string}> {}
class NotFoundError extends Data.TaggedError("NotFoundError")<{message: string}> {}

type V2Error = ValidationError | NotFoundError | QueryError

function resolveCreatorProfiles(
	db: Database,
	creatorIds: (string | null)[],
): Effect.Effect<Map<string, Profile>, never> {
	const unique = [...new Set(creatorIds.filter((id): id is string => id !== null))]
	if (unique.length === 0) return Effect.succeed(new Map())
	return getProfilesBySpaceIds(db, unique.map(toDashedUuid)).pipe(
		Effect.tapError((err) =>
			Effect.logWarning("Profile resolution failed, degrading gracefully", {
				cause: String(err),
			}),
		),
		Effect.catchAll(() => Effect.succeed(new Map())),
	)
}

export function createVersionedV2Router(db: Database, runtime: AppRuntime) {
	const router = new Hono<AppEnv>()

	/**
	 * GET /v2/versioned/entities/:id/diff
	 *
	 * Identical to v1 /versioned/entities/:id/diff but with `imageUrl` / `videoUrl`
	 * inlined on relation changes whose target is an IMAGE_TYPE or VIDEO_TYPE entity.
	 */
	router.get("/entities/:id/diff", async (c) => {
		const rawEntityId = c.req.param("id")
		const rawFromEditId = c.req.query("fromEditId")
		const rawToEditId = c.req.query("toEditId")
		const rawSpaceId = c.req.query("spaceId")
		const requestId = c.get("requestId") ?? "unknown"

		const program = Effect.gen(function* () {
			if (!isValidUuid(rawEntityId)) {
				return yield* Effect.fail(new ValidationError({message: "Entity ID must be a valid UUID"}))
			}
			if (!rawToEditId) {
				return yield* Effect.fail(new ValidationError({message: "toEditId query parameter is required"}))
			}
			if (!rawSpaceId) {
				return yield* Effect.fail(
					new ValidationError({message: "spaceId query parameter is required for diffs"}),
				)
			}
			if (!isValidUuid(rawToEditId)) {
				return yield* Effect.fail(new ValidationError({message: "toEditId must be a valid UUID"}))
			}
			if (!isValidUuid(rawSpaceId)) {
				return yield* Effect.fail(new ValidationError({message: "spaceId must be a valid UUID"}))
			}
			// fromEditId is OPTIONAL: when omitted the endpoint runs in snapshot
			// mode, returning the entity's state at toEditId as an all-added diff.
			if (rawFromEditId && !isValidUuid(rawFromEditId)) {
				return yield* Effect.fail(new ValidationError({message: "fromEditId must be a valid UUID"}))
			}

			const entityId = normalizeUuid(rawEntityId)
			const toEditId = normalizeUuid(rawToEditId)
			const spaceId = normalizeUuid(rawSpaceId)
			const fromEditId = rawFromEditId ? normalizeUuid(rawFromEditId) : null

			const toResolved = yield* resolveVersionKey(db, toEditId)
			if (toResolved === null) {
				return yield* Effect.fail(new NotFoundError({message: `Edit '${toEditId}' not found`}))
			}
			const fromResolved = fromEditId ? yield* resolveVersionKey(db, fromEditId) : null
			if (fromEditId && fromResolved === null) {
				return yield* Effect.fail(new NotFoundError({message: `Edit '${fromEditId}' not found`}))
			}

			// Snapshot mode (no fromEditId) diffs against an empty "before" → all added.
			const emptyBefore: GroupedEntitySnapshot = {
				id: entityId,
				values: [],
				relations: [],
				blocks: [],
				groupKeys: [],
				groups: {},
			}
			const [beforeSnapshot, afterSnapshot] = yield* Effect.all([
				fromResolved
					? getGroupedEntitySnapshotAtVersion(db, entityId, fromResolved.versionKey, spaceId, "v2")
					: Effect.succeed(emptyBefore),
				getGroupedEntitySnapshotAtVersion(db, entityId, toResolved.versionKey, spaceId, "v2"),
			])

			// The enrichment chain (blocks → blockConfig → names → media) is a true
			// pipeline — each step folds into the diff the previous produced — so it stays
			// sequential. Creator-profile resolution depends only on the resolved edit
			// metadata, not the diff, so run it concurrently to keep its DB round trip off
			// the enrichment critical path.
			const enrichmentChain = Effect.gen(function* () {
				const rawDiff = yield* diffGroupedEntitySnapshots(entityId, beforeSnapshot, afterSnapshot)
				const richBlocksDiff = yield* enrichBlocks(rawDiff, beforeSnapshot, afterSnapshot)
				const blockConfigDiff = yield* enrichBlockConfig(
					db,
					richBlocksDiff,
					entityId,
					beforeSnapshot,
					afterSnapshot,
					fromResolved?.versionKey ?? null,
					toResolved.versionKey,
					spaceId,
				)
				const namedDiff = yield* enrichNames(db, blockConfigDiff, spaceId)
				return yield* enrichWithMediaUrls(db, namedDiff, {
					// In snapshot mode there is no before side, so the from key is unused;
					// fall back to the to version key to keep the lookup well-formed.
					fromVersionKey: fromResolved?.versionKey ?? toResolved.versionKey,
					toVersionKey: toResolved.versionKey,
					spaceId,
				})
			})

			const [enrichedDiff, profileMap] = yield* Effect.all(
				[
					enrichmentChain,
					resolveCreatorProfiles(db, [fromResolved?.createdById ?? null, toResolved.createdById]),
				],
				{concurrency: "unbounded"},
			)

			// Spread groups at root level to match v1 DiffResponse shape (groups is
			// recorded as Record<NormalizedUuid, DynamicGroupItem[]> at the JSON root).
			const {groups, ...rest} = enrichedDiff
			const response = {
				...rest,
				...groups,
				fromEditName: fromResolved?.name ?? null,
				fromCreatedById: fromResolved?.createdById ?? null,
				fromCreatedBy: fromResolved?.createdById ? (profileMap.get(fromResolved.createdById) ?? null) : null,
				toEditName: toResolved.name,
				toCreatedById: toResolved.createdById,
				toCreatedBy: toResolved.createdById ? (profileMap.get(toResolved.createdById) ?? null) : null,
			} as unknown as DiffResponseV2

			return response
		}).pipe(
			Effect.tapError((error) => {
				if (error._tag === "QueryError") {
					return Effect.logError(`Database error: operation=${error.operation}, cause=${String(error.cause)}`)
				}
				return Effect.void
			}),
			Effect.withSpan("GET /v2/versioned/entities/:id/diff"),
			Effect.annotateSpans({
				requestId,
				entityId: rawEntityId,
				fromEditId: rawFromEditId,
				toEditId: rawToEditId,
				spaceId: rawSpaceId,
			}),
		)

		const result = await runtime.runPromise(Effect.either(program))
		return Either.match(result, {
			onLeft: (error: V2Error) => {
				switch (error._tag) {
					case "ValidationError":
						return c.json({error: "Invalid parameter", message: error.message}, 400)
					case "NotFoundError":
						return c.json({error: "Not found", message: error.message}, 404)
					case "QueryError":
						return c.json({error: "Internal server error", message: "An unexpected error occurred"}, 500)
				}
			},
			onRight: (response: DiffResponseV2) => c.json(response as unknown as Record<string, unknown>, 200),
		})
	})

	/**
	 * GET /v2/versioned/proposals/:id/diff
	 *
	 * Enriched, context-aware variant of the v1 proposal diff: each entity is a
	 * grouped diff with resolved names, media URLs on relations, and folded block
	 * values/config — same enrichment as the entity-diff endpoint.
	 */
	router.get("/proposals/:id/diff", async (c) => {
		const rawProposalId = c.req.param("id")
		const rawSpaceId = c.req.query("spaceId")
		const cursor = c.req.query("cursor")
		const rawLimit = c.req.query("limit")

		if (!isValidUuid(rawProposalId)) {
			return c.json({error: "Invalid parameter", message: "Proposal ID must be a valid UUID"}, 400)
		}
		if (!rawSpaceId || !isValidUuid(rawSpaceId)) {
			return c.json({error: "Invalid parameter", message: "spaceId must be a valid UUID"}, 400)
		}
		let limit = 50
		if (rawLimit !== undefined) {
			const parsed = Number.parseInt(rawLimit, 10)
			if (Number.isNaN(parsed) || parsed < 1) {
				return c.json({error: "Invalid parameter", message: "limit must be a positive integer"}, 400)
			}
			limit = Math.min(parsed, 100)
		}

		const program = computeProposalDiffV2(
			db,
			normalizeUuid(rawProposalId),
			normalizeUuid(rawSpaceId),
			cursor,
			limit,
		).pipe(Effect.withSpan("GET /v2/versioned/proposals/:id/diff"))

		const result = await runtime.runPromise(Effect.either(program))
		return Either.match(result, {
			onLeft: (error) => {
				const mapped = mapGroupedProposalError(error)
				return c.json(mapped.body, mapped.status)
			},
			onRight: (diff) => c.json(diff as unknown as Record<string, unknown>, 200),
		})
	})

	/**
	 * GET /v2/versioned/proposal-groups/diff
	 *
	 * Enriched, context-aware variant of the v1 grouped (multi-proposal) diff.
	 */
	router.get("/proposal-groups/diff", async (c) => {
		const validation = validateGroupedRequest({
			spaceId: c.req.query("spaceId"),
			proposalIds: c.req.query("proposalIds"),
			cursor: c.req.query("cursor"),
			limit: c.req.query("limit"),
		})
		if (!validation.ok) {
			return c.json(validation.failure.body, validation.failure.status)
		}
		const {spaceId, proposalIds, cursor, limit} = validation.value

		const program = computeGroupedProposalDiffV2(db, proposalIds, spaceId, cursor, limit).pipe(
			Effect.withSpan("GET /v2/versioned/proposal-groups/diff"),
		)

		const result = await runtime.runPromise(Effect.either(program))
		return Either.match(result, {
			onLeft: (error) => {
				const mapped = mapGroupedProposalError(error)
				return c.json(mapped.body, mapped.status)
			},
			onRight: (diff) => c.json(diff as unknown as Record<string, unknown>, 200),
		})
	})

	/**
	 * POST /v2/versioned/review
	 *
	 * Diff a space's UNPUBLISHED local edit against current live state and return
	 * the same enriched `EntityDiffV2[]` shape as the proposal diff. Non-mutating —
	 * computes only, persists nothing.
	 *
	 * Body: `{ spaceId: string, edit: string, cursor?: string, limit?: number }`
	 * where `edit` is the base64-encoded GRC-20 edit blob the SDK would publish.
	 * First release accepts the encoded blob (decoded via `decodeEditAuto`) for
	 * exact publish-parity; raw JSON `ops[]` support is a follow-up (op ids are
	 * 16-byte binary, not hex strings, so they need a conversion layer).
	 */
	router.post("/review", async (c) => {
		const body = (await c.req.json().catch(() => null)) as {
			spaceId?: unknown
			edit?: unknown
			cursor?: unknown
			limit?: unknown
		} | null

		const program = Effect.gen(function* () {
			if (!body || typeof body !== "object") {
				return yield* Effect.fail(new ValidationError({message: "Request body must be a JSON object"}))
			}
			if (typeof body.spaceId !== "string" || !isValidUuid(body.spaceId)) {
				return yield* Effect.fail(new ValidationError({message: "spaceId must be a valid UUID"}))
			}
			if (typeof body.edit !== "string" || body.edit.length === 0) {
				return yield* Effect.fail(new ValidationError({message: "edit (base64-encoded edit blob) is required"}))
			}
			if (body.cursor !== undefined && typeof body.cursor !== "string") {
				return yield* Effect.fail(new ValidationError({message: "cursor must be a string"}))
			}
			let limit = 50
			if (body.limit !== undefined) {
				if (typeof body.limit !== "number" || !Number.isInteger(body.limit) || body.limit < 1) {
					return yield* Effect.fail(new ValidationError({message: "limit must be a positive integer"}))
				}
				limit = Math.min(body.limit, 100)
			}

			const blob = yield* Effect.try({
				try: () => new Uint8Array(Buffer.from(body.edit as string, "base64")),
				catch: () => new ValidationError({message: "edit must be valid base64"}),
			})
			const ops = yield* Effect.tryPromise({
				try: async () => (await decodeEditAuto(blob)).ops,
				catch: (error) => new EditDecodeError(error),
			})
			if (ops.length > MAX_REVIEW_OPS) {
				return yield* Effect.fail(
					new ValidationError({
						message: `edit exceeds the ${MAX_REVIEW_OPS}-op review limit (${ops.length})`,
					}),
				)
			}

			return yield* computeReviewDiffV2(
				db,
				ops,
				normalizeUuid(body.spaceId),
				body.cursor as string | undefined,
				limit,
			)
		}).pipe(Effect.withSpan("POST /v2/versioned/review"))

		const result = await runtime.runPromise(Effect.either(program))
		return Either.match(result, {
			onLeft: (error) => {
				if (error._tag === "ValidationError") {
					return c.json({error: "Invalid parameter", message: error.message}, 400)
				}
				const mapped = mapGroupedProposalError(error)
				return c.json(mapped.body, mapped.status)
			},
			onRight: (diff) => c.json(diff as unknown as Record<string, unknown>, 200),
		})
	})

	return router
}
