/**
 * Integration tests for the v2 review endpoint (POST /v2/versioned/review).
 *
 * The review endpoint diffs a space's UNPUBLISHED local edit (the base64-encoded
 * GRC-20 edit blob the SDK would publish) against current live state, returning
 * the same enriched EntityDiffV2[] shape as the proposal diff — without a
 * persisted proposal.
 *
 * Runs against a real PostgreSQL DB. Prerequisites:
 *   - DATABASE_URL set, migrations applied (bun run db:migrate)
 * Skipped automatically when DATABASE_URL is unset (mirrors the other suites).
 *
 * NOTE: authored to the same conventions as v2-entity-diff.test.ts /
 * proposal-diff-edit-flow.test.ts but not yet executed locally (no test DB
 * provisioned in this environment) — relies on the CI integration job.
 */

import {EditBuilder, encodeEdit, type Id, parseId} from "@geoprotocol/grc-20"
import {SystemIds} from "@graphprotocol/grc-20"
import {drizzle} from "drizzle-orm/node-postgres"
import {Hono} from "hono"
import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {runtime} from "../../services/runtime"
import {createVersionedV2Router} from "../v2"

const DATABASE_URL = process.env.DATABASE_URL
const SKIP = !DATABASE_URL

const uuidToId = (u: string): Id => {
	const id = parseId(u)
	if (!id) throw new Error(`invalid uuid: ${u}`)
	return id
}

// Fixture ids — 9b000000-* prefix for isolation.
const SPACE = "9b000000-0000-4000-8000-000000000001"
const EDIT = "9b000000-000e-4000-8000-000000000001"
const NEW_ENTITY = "9b000000-0001-4000-8000-000000000001"
const NEW_TARGET = "9b000000-0002-4000-8000-000000000001"
const REL = "9b000000-0003-4000-8000-000000000001"
const REL_TYPE = "9b000000-0004-4000-8000-000000000001"

/** Build → encode → base64 an edit, the exact shape the SDK publishes. */
function editBlob(build: (e: EditBuilder) => EditBuilder): string {
	const edit = build(new EditBuilder(uuidToId(EDIT)).setName("review test").setCreatedNow()).build()
	return Buffer.from(encodeEdit(edit)).toString("base64")
}

describe.skipIf(SKIP)("POST /v2/versioned/review", () => {
	let pool: Pool
	let app: Hono

	beforeAll(() => {
		pool = new Pool({connectionString: DATABASE_URL})
		app = new Hono()
		// biome-ignore lint/suspicious/noExplicitAny: test wiring mirrors the other v2 suites
		app.route("/v2/versioned", createVersionedV2Router(drizzle(pool) as any, runtime))
	})

	afterAll(async () => {
		await pool.end()
	})

	const post = (body: unknown) =>
		app.request("/v2/versioned/review", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify(body),
		})

	describe("validation", () => {
		it("400 when spaceId is missing", async () => {
			const res = await post({edit: editBlob((e) => e.createEmptyEntity(uuidToId(NEW_ENTITY)))})
			expect(res.status).toBe(400)
		})

		it("400 when spaceId is not a UUID", async () => {
			const res = await post({spaceId: "not-a-uuid", edit: "AAAA"})
			expect(res.status).toBe(400)
		})

		it("400 when edit is missing", async () => {
			const res = await post({spaceId: SPACE})
			expect(res.status).toBe(400)
		})

		it("400 when edit is not a decodable GRC-20 blob", async () => {
			const res = await post({spaceId: SPACE, edit: "not-base64-or-a-real-edit!!!"})
			expect(res.status).toBe(400)
		})

		it("400 when limit is invalid", async () => {
			const res = await post({
				spaceId: SPACE,
				edit: editBlob((e) => e.createEmptyEntity(uuidToId(NEW_ENTITY))),
				limit: 0,
			})
			expect(res.status).toBe(400)
		})
	})

	describe("create-only edit (all-added, no base state)", () => {
		it("returns the new entity with its value added and relation added", async () => {
			const res = await post({
				spaceId: SPACE,
				edit: editBlob((e) =>
					e
						.createEntity(uuidToId(NEW_ENTITY), (b) =>
							b.text(uuidToId(SystemIds.NAME_PROPERTY), "Brand New Entity"),
						)
						.createRelationSimple(
							uuidToId(REL),
							uuidToId(NEW_ENTITY),
							uuidToId(NEW_TARGET),
							uuidToId(REL_TYPE),
						),
				),
			})
			expect(res.status).toBe(200)
			const body = (await res.json()) as {
				spaceId: string
				entities: {entityId: string; values: {after: unknown}[]; relations: {changeType: string}[]}[]
				pagination: {hasMore: boolean; totalEntities: number}
			}

			expect(body.spaceId).toBe(SPACE.replace(/-/g, ""))
			const entity = body.entities.find((e) => e.entityId === NEW_ENTITY.replace(/-/g, ""))
			expect(entity).toBeDefined()
			// Name value is added.
			expect(entity?.values.some((v) => v.after === "Brand New Entity")).toBe(true)
			// Relation is added.
			expect(entity?.relations.some((r) => r.changeType === "ADD")).toBe(true)
			expect(body.pagination.hasMore).toBe(false)
		})
	})
})
