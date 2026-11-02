/**
 * GET /geodb/entities/:entityId/card?spaceId=<space>
 *
 * Opt-in entity-card read answered from geo-db (see config.ts for the modes). Mounted only when
 * GEODB_ENTITY_CARDS is "on" or "shadow".
 */

import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Hono} from "hono"
import {describeRoute} from "hono-openapi"

import {log} from "../services/telemetry"
import {isValidUuid, normalizeUuid} from "../utils/uuid"
import type {GeoDbCardClient} from "./client"
import {createCliGeoDbClient, createSpaceMap} from "./client"
import type {GeoDbCardConfig} from "./config"
import {getCardRows} from "./queries"
import type {CardDeps} from "./service"
import {resolveCard} from "./service"

type Database = NodePgDatabase<Record<string, unknown>>

export function createGeoDbRouter(
	db: Database,
	config: GeoDbCardConfig,
	overrides: Partial<Pick<CardDeps, "geoSpaceOf" | "log">> & {geoDb?: GeoDbCardClient} = {},
) {
	const deps: CardDeps = {
		mode: config.mode,
		geoDb: overrides.geoDb ?? createCliGeoDbClient(config),
		geoSpaceOf: overrides.geoSpaceOf ?? createSpaceMap(config.spaceMapPath),
		readPostgres: (entityId, spaceId) => getCardRows(db, entityId, spaceId),
		log: overrides.log ?? log,
	}
	const router = new Hono()

	router.get(
		"/entities/:entityId/card",
		describeRoute({
			tags: ["GeoDB"],
			summary: "Get an entity card (geo-db read, opt-in)",
			description:
				"Returns an entity's statements in one space. Behind GEODB_ENTITY_CARDS: answered from geo-db with a per-request Postgres fallback, or from Postgres with a geo-db shadow comparison.",
			responses: {200: {description: "Entity card"}, 400: {description: "Invalid entity or space id"}},
		}),
		async (c) => {
			const rawEntity = c.req.param("entityId")
			const rawSpace = c.req.query("spaceId") ?? ""
			if (!isValidUuid(rawEntity) || !isValidUuid(rawSpace)) {
				return c.json({error: "Invalid parameter", message: "entityId and spaceId must be UUIDs"}, 400)
			}
			const {card} = await resolveCard(deps, normalizeUuid(rawEntity), normalizeUuid(rawSpace))
			return c.json(card)
		},
	)

	return router
}
