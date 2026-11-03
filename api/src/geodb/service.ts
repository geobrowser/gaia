/**
 * Entity-card read with the geo-db route behind a flag.
 *
 * - "on": answer from geo-db; any geo-db error or timeout falls back to Postgres for that request.
 * - "shadow": answer from Postgres; geo-db is asked after the answer is ready and a structured diff
 *   is logged when the two differ. The shadow read never delays or fails the response.
 */

import type {CardStatement, RelationRow, ValueRow} from "./card"
import {conformingId, diffCards, isEmptyDiff, statementsFromPostgres} from "./card"
import type {GeoDbCardClient} from "./client"
import {GeoDbError} from "./client"
import type {GeoDbCardMode} from "./config"
import {observeGeoDbRead, recordFallback, recordServed, recordShadow, type ShadowResult} from "./metrics"

export type CardStatementView = {property: string; value: string}

export type EntityCard = {
	entityId: string
	spaceId: string
	/** Which store produced `statements`. */
	source: "geodb" | "postgres"
	/** Why a geo-db answer was not used, when `mode` was "on". */
	fallback?: string
	/** Both stores answer in geo-db's id space (conforming ids pass through unchanged). */
	statements: CardStatementView[]
}

export type CardLogger = {
	info: (message: string, data?: Record<string, unknown>) => void
	warn: (message: string, data?: Record<string, unknown>) => void
}

export type CardDeps = {
	mode: GeoDbCardMode
	geoDb: GeoDbCardClient
	/** gaia space id (compact) -> geo-db space id, or null when the space is not loaded. */
	geoSpaceOf: (gaiaSpaceId: string) => Promise<string | null>
	/** Postgres card rows for (entity, space), both compact ids. */
	readPostgres: (entityId: string, spaceId: string) => Promise<{values: ValueRow[]; relations: RelationRow[]}>
	log: CardLogger
}

const SAMPLE = 5

const view = (s: CardStatement): CardStatementView => ({property: s.prop, value: s.norm})

async function askGeoDb(deps: CardDeps, entityId: string, spaceId: string) {
	const geoSpaceId = await deps.geoSpaceOf(spaceId)
	if (!geoSpaceId) throw new GeoDbError("unmapped_space", `space ${spaceId} is not loaded in geo-db`)
	const started = performance.now()
	try {
		const rows = await deps.geoDb.readCard({geoSpaceId, entityId})
		observeGeoDbRead((performance.now() - started) / 1000, "ok")
		return rows
	} catch (error) {
		observeGeoDbRead((performance.now() - started) / 1000, "error")
		throw error
	}
}

/** Compare the Postgres answer with geo-db's and log what differs. Never throws. */
export async function shadowCompare(
	deps: CardDeps,
	entityId: string,
	spaceId: string,
	postgres: ReturnType<typeof statementsFromPostgres>,
	conformedEntityId: string,
): Promise<void> {
	const started = performance.now()
	try {
		const rows = await askGeoDb(deps, conformedEntityId, spaceId)
		const diff = diffCards(postgres, rows)
		const result: ShadowResult = isEmptyDiff(diff)
			? "match"
			: diff.onlyPostgres.length > 0 && diff.onlyGeoDb.length > 0
				? "both"
				: diff.onlyPostgres.length > 0
					? "only_postgres"
					: "only_geodb"
		recordShadow(result)
		if (!isEmptyDiff(diff)) {
			deps.log.warn("geodb card diff", {
				entityId,
				spaceId,
				onlyPostgres: diff.onlyPostgres.length,
				onlyGeoDb: diff.onlyGeoDb.length,
				skipped: diff.skipped,
				samplePostgres: diff.onlyPostgres.slice(0, SAMPLE),
				sampleGeoDb: diff.onlyGeoDb.slice(0, SAMPLE),
				geoDbMs: Math.round(performance.now() - started),
			})
		}
	} catch (error) {
		recordShadow("failed")
		deps.log.warn("geodb card shadow read failed", {
			entityId,
			spaceId,
			reason: error instanceof GeoDbError ? error.reason : "error",
			message: String(error),
		})
	}
}

async function postgresStatements(deps: CardDeps, entityId: string, spaceId: string) {
	const {values, relations} = await deps.readPostgres(entityId, spaceId)
	return statementsFromPostgres(values, relations)
}

/**
 * `shadow` is the in-flight geo-db comparison in shadow mode; the route does not await it, tests do.
 */
export async function resolveCard(
	deps: CardDeps,
	entityId: string,
	spaceId: string,
): Promise<{card: EntityCard; shadow?: Promise<void>}> {
	const geoEntityId = conformingId("entity", entityId)
	if (deps.mode === "on") {
		try {
			const rows = await askGeoDb(deps, geoEntityId, spaceId)
			const statements = rows.map((r) => ({property: r.prop, value: r.raw}))
			recordServed("geodb")
			return {card: {entityId, spaceId, source: "geodb", statements}}
		} catch (error) {
			const reason = error instanceof GeoDbError ? error.reason : "error"
			recordFallback(reason)
			recordServed("postgres")
			deps.log.warn("geodb card read failed, falling back to postgres", {
				entityId,
				spaceId,
				reason,
				message: String(error),
			})
			const postgres = await postgresStatements(deps, entityId, spaceId)
			return {
				card: {
					entityId,
					spaceId,
					source: "postgres",
					fallback: reason,
					statements: postgres.statements.map(view),
				},
			}
		}
	}
	const postgres = await postgresStatements(deps, entityId, spaceId)
	recordServed("postgres")
	const card: EntityCard = {entityId, spaceId, source: "postgres", statements: postgres.statements.map(view)}
	return deps.mode === "shadow"
		? {card, shadow: shadowCompare(deps, entityId, spaceId, postgres, geoEntityId)}
		: {card}
}
