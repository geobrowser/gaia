/**
 * Versioned entities router.
 *
 * Provides REST endpoints for querying entity state at specific versions
 * and computing diffs between versions.
 */

import { Hono } from "hono";
import type { NodePgDatabase } from "drizzle-orm/node-postgres";

import { isValidUuid } from "../utils/uuid";
import {
	resolveVersionKey,
	getEntitySnapshotAtVersion,
	getEntityVersions,
} from "./queries";
import { diffEntitySnapshots } from "./diff";
import { computeProposalDiff } from "./proposal-diff";

type Database = NodePgDatabase<Record<string, unknown>>;

/**
 * Create the versioned entities router.
 *
 * @param db - Drizzle database instance
 * @returns Configured Hono router
 */
export function createVersionedRouter(db: Database) {
	const router = new Hono();

	/**
	 * GET /versioned/entities/:id
	 *
	 * Get an entity snapshot at a specific version.
	 *
	 * Query Parameters:
	 * - editId: The edit ID to get the entity state at (required)
	 * - spaceId: Filter values/relations to a specific space (optional)
	 *
	 * Response:
	 * - 200: EntitySnapshot
	 * - 400: Invalid parameters
	 * - 404: Entity or edit not found
	 */
	router.get("/entities/:id", async (c) => {
		const entityId = c.req.param("id");
		const editId = c.req.query("editId");
		const spaceId = c.req.query("spaceId");

		// Validate entityId
		if (!isValidUuid(entityId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "Entity ID must be a valid UUID",
				},
				400
			);
		}

		// Validate editId is provided
		if (!editId) {
			return c.json(
				{
					error: "Missing required parameter",
					message: "editId query parameter is required",
				},
				400
			);
		}

		if (!isValidUuid(editId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "editId must be a valid UUID",
				},
				400
			);
		}

		// Validate spaceId if provided
		if (spaceId && !isValidUuid(spaceId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "spaceId must be a valid UUID",
				},
				400
			);
		}

		try {
			// Resolve edit to version key
			const versionKey = await resolveVersionKey(db, editId);
			if (versionKey === null) {
				return c.json(
					{
						error: "Not found",
						message: `Edit '${editId}' not found`,
					},
					404
				);
			}

			// Get entity snapshot at version
			const snapshot = await getEntitySnapshotAtVersion(
				db,
				entityId,
				versionKey,
				spaceId
			);

			return c.json(snapshot);
		} catch (error) {
			console.error("[VERSIONED] Error getting entity snapshot:", error);
			return c.json(
				{
					error: "Internal error",
					message:
						error instanceof Error ? error.message : "An unexpected error occurred",
				},
				500
			);
		}
	});

	/**
	 * GET /versioned/entities/:id/versions
	 *
	 * List versions (edits) that affected an entity.
	 *
	 * Query Parameters:
	 * - spaceId: Filter to changes in a specific space (optional)
	 * - limit: Maximum results (optional, default: 50, max: 100)
	 * - offset: Pagination offset (optional, default: 0)
	 *
	 * Response:
	 * - 200: Array of VersionEntry
	 * - 400: Invalid parameters
	 */
	router.get("/entities/:id/versions", async (c) => {
		const entityId = c.req.param("id");
		const spaceId = c.req.query("spaceId");
		const limitParam = c.req.query("limit");
		const offsetParam = c.req.query("offset");

		// Validate entityId
		if (!isValidUuid(entityId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "Entity ID must be a valid UUID",
				},
				400
			);
		}

		// Validate spaceId if provided
		if (spaceId && !isValidUuid(spaceId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "spaceId must be a valid UUID",
				},
				400
			);
		}

		// Parse and validate limit
		let limit = 50;
		if (limitParam) {
			const parsed = parseInt(limitParam, 10);
			if (Number.isNaN(parsed) || parsed < 1) {
				return c.json(
					{
						error: "Invalid parameter",
						message: "limit must be a positive integer",
					},
					400
				);
			}
			limit = Math.min(parsed, 100);
		}

		// Parse and validate offset
		let offset = 0;
		if (offsetParam) {
			const parsed = parseInt(offsetParam, 10);
			if (Number.isNaN(parsed) || parsed < 0) {
				return c.json(
					{
						error: "Invalid parameter",
						message: "offset must be a non-negative integer",
					},
					400
				);
			}
			offset = parsed;
		}

		try {
			const versions = await getEntityVersions(
				db,
				entityId,
				spaceId,
				limit,
				offset
			);

			return c.json({ versions });
		} catch (error) {
			console.error("[VERSIONED] Error getting entity versions:", error);
			return c.json(
				{
					error: "Internal error",
					message:
						error instanceof Error ? error.message : "An unexpected error occurred",
				},
				500
			);
		}
	});

	/**
	 * GET /versioned/entities/:id/diff
	 *
	 * Compute a diff between two versions of an entity.
	 *
	 * Query Parameters:
	 * - fromEditId: The starting edit ID (required)
	 * - toEditId: The ending edit ID (required)
	 * - spaceId: Filter to a specific space (required)
	 *
	 * Response:
	 * - 200: EntityDiff
	 * - 400: Invalid or missing parameters
	 * - 404: Edit not found
	 */
	router.get("/entities/:id/diff", async (c) => {
		const entityId = c.req.param("id");
		const fromEditId = c.req.query("fromEditId");
		const toEditId = c.req.query("toEditId");
		const spaceId = c.req.query("spaceId");

		// Validate entityId
		if (!isValidUuid(entityId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "Entity ID must be a valid UUID",
				},
				400
			);
		}

		// Validate required parameters
		if (!fromEditId) {
			return c.json(
				{
					error: "Missing required parameter",
					message: "fromEditId query parameter is required",
				},
				400
			);
		}

		if (!toEditId) {
			return c.json(
				{
					error: "Missing required parameter",
					message: "toEditId query parameter is required",
				},
				400
			);
		}

		if (!spaceId) {
			return c.json(
				{
					error: "Missing required parameter",
					message: "spaceId query parameter is required for diffs",
				},
				400
			);
		}

		// Validate UUIDs
		if (!isValidUuid(fromEditId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "fromEditId must be a valid UUID",
				},
				400
			);
		}

		if (!isValidUuid(toEditId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "toEditId must be a valid UUID",
				},
				400
			);
		}

		if (!isValidUuid(spaceId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "spaceId must be a valid UUID",
				},
				400
			);
		}

		try {
			// Resolve both edits to version keys
			const [fromVersionKey, toVersionKey] = await Promise.all([
				resolveVersionKey(db, fromEditId),
				resolveVersionKey(db, toEditId),
			]);

			if (fromVersionKey === null) {
				return c.json(
					{
						error: "Not found",
						message: `Edit '${fromEditId}' not found`,
					},
					404
				);
			}

			if (toVersionKey === null) {
				return c.json(
					{
						error: "Not found",
						message: `Edit '${toEditId}' not found`,
					},
					404
				);
			}

			// Get snapshots at both versions
			const [fromSnapshot, toSnapshot] = await Promise.all([
				getEntitySnapshotAtVersion(db, entityId, fromVersionKey, spaceId),
				getEntitySnapshotAtVersion(db, entityId, toVersionKey, spaceId),
			]);

			// Compute diff
			const diff = diffEntitySnapshots(entityId, fromSnapshot, toSnapshot);

			return c.json(diff);
		} catch (error) {
			console.error("[VERSIONED] Error computing entity diff:", error);
			return c.json(
				{
					error: "Internal error",
					message:
						error instanceof Error ? error.message : "An unexpected error occurred",
				},
				500
			);
		}
	});

	/**
	 * GET /versioned/proposals/:id/diff
	 *
	 * Compute diff between proposal's proposed changes and base state.
	 * - Active proposals: compare against current live state
	 * - Closed proposals: compare against versioned state at end_time
	 *
	 * Query Parameters:
	 * - spaceId: Filter to specific space (required)
	 * - cursor: Pagination cursor (optional)
	 * - limit: Max entities per page (optional, default: 50, max: 100)
	 *
	 * Response:
	 * - 200: PaginatedProposalDiff
	 * - 400: Invalid parameters
	 * - 404: Proposal not found or no Publish action
	 */
	router.get("/proposals/:id/diff", async (c) => {
		const proposalId = c.req.param("id");
		const spaceId = c.req.query("spaceId");
		const cursor = c.req.query("cursor");
		const limitParam = c.req.query("limit");

		// Validate proposalId
		if (!isValidUuid(proposalId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "Proposal ID must be a valid UUID",
				},
				400
			);
		}

		// Validate required spaceId
		if (!spaceId) {
			return c.json(
				{
					error: "Missing required parameter",
					message: "spaceId query parameter is required",
				},
				400
			);
		}

		if (!isValidUuid(spaceId)) {
			return c.json(
				{
					error: "Invalid parameter",
					message: "spaceId must be a valid UUID",
				},
				400
			);
		}

		// Parse and validate limit
		let limit = 50;
		if (limitParam) {
			const parsed = parseInt(limitParam, 10);
			if (Number.isNaN(parsed) || parsed < 1) {
				return c.json(
					{
						error: "Invalid parameter",
						message: "limit must be a positive integer",
					},
					400
				);
			}
			limit = Math.min(parsed, 100);
		}

		try {
			const result = await computeProposalDiff(
				db,
				proposalId,
				spaceId,
				cursor,
				limit
			);

			// Check if result is an error
			if ("error" in result && "code" in result) {
				return c.json(
					{
						error: result.code === 404 ? "Not found" : "Internal error",
						message: result.error,
					},
					result.code as 404 | 500
				);
			}

			return c.json(result);
		} catch (error) {
			console.error("[VERSIONED] Error computing proposal diff:", error);
			return c.json(
				{
					error: "Internal error",
					message:
						error instanceof Error ? error.message : "An unexpected error occurred",
				},
				500
			);
		}
	});
	return router;
}
