/**
 * Entity-card read with the geo-db route behind a flag.
 *
 * - "on": answer from geo-db; any geo-db error or timeout falls back to Postgres for that request.
 * - "shadow": answer from Postgres; geo-db is asked after the answer is ready and a structured diff
 *   is logged when the two differ. The shadow read never delays or fails the response.
 */

import type {CardStatement, GeoCardRow, RelationRow, ValueRow} from "./card"
import {conformingId, diffCards, isEmptyDiff, statementsFromPostgres} from "./card"
import type {GeoDbCardClient} from "./client"
import {GeoDbError} from "./client"
import type {GeoDbCardMode} from "./config"
import {
	observeGeoDbRead,
	observePostgresCardRead,
	recordFallback,
	recordServed,
	recordShadow,
	type ShadowResult,
} from "./metrics"

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

/**
 * One geo-db card read: maps the gaia space, reads through the pipe pool and times the read. Shared by the
 * card route and the GraphQL gateway's entity shadow, so both use the same space map, pool and read metric.
 */
export async function askGeoDb(
	deps: Pick<CardDeps, "geoDb" | "geoSpaceOf">,
	entityId: string,
	spaceId: string,
	options: {names?: boolean} = {},
) {
	const geoSpaceId = await deps.geoSpaceOf(spaceId)
	if (!geoSpaceId) throw new GeoDbError("unmapped_space", `space ${spaceId} is not loaded in geo-db`)
	const started = performance.now()
	try {
		const rows = await deps.geoDb.readCard(
			options.names ? {geoSpaceId, entityId, names: true} : {geoSpaceId, entityId},
		)
		observeGeoDbRead((performance.now() - started) / 1000, "ok")
		return rows
	} catch (error) {
		observeGeoDbRead((performance.now() - started) / 1000, "error")
		throw error
	}
}

/**
 * One geo-db read of an entity's card across `spaceIds` (gaia ids, two or more), as one pipe line. Every Space
 * is mapped first, and one geo-db has not loaded throws `unmapped_space` before anything is sent: a Space the
 * node does not hold would refuse the whole line and cost the worker its connection. Returns the rows by gaia
 * space, each requested Space present (empty when geo-db holds nothing for the entity there). Needs a client
 * with `readCardSpaces`.
 */
export async function askGeoDbSpaces(
	deps: Pick<CardDeps, "geoDb" | "geoSpaceOf">,
	entityId: string,
	spaceIds: string[],
	options: {names?: boolean} = {},
): Promise<Map<string, GeoCardRow[]>> {
	const readCardSpaces = deps.geoDb.readCardSpaces?.bind(deps.geoDb)
	if (!readCardSpaces) throw new GeoDbError("bad_request", "the geo-db client cannot read several spaces at once")
	const geoOf = new Map<string, string>()
	for (const spaceId of spaceIds) {
		const geoSpaceId = await deps.geoSpaceOf(spaceId)
		if (!geoSpaceId) throw new GeoDbError("unmapped_space", `space ${spaceId} is not loaded in geo-db`)
		geoOf.set(spaceId, geoSpaceId)
	}
	const started = performance.now()
	let rows: GeoCardRow[]
	try {
		rows = await readCardSpaces({geoSpaceIds: [...geoOf.values()], entityId, names: options.names})
		observeGeoDbRead((performance.now() - started) / 1000, "ok")
	} catch (error) {
		observeGeoDbRead((performance.now() - started) / 1000, "error")
		throw error
	}
	const bySpace = new Map<string, GeoCardRow[]>(spaceIds.map((spaceId) => [spaceId, []]))
	const gaiaOf = new Map([...geoOf].map(([gaia, geo]) => [geo, gaia]))
	for (const row of rows) {
		const gaia = row.space ? gaiaOf.get(row.space) : undefined
		if (!gaia)
			throw new GeoDbError("client_failed", `geo-db answered a row from an unrequested space: ${row.space}`)
		const {space: _space, ...rest} = row
		bySpace.get(gaia)?.push(rest)
	}
	return bySpace
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
		// A Space geo-db has not loaded is not a question it failed: there is nothing to compare, so nothing to count.
		if (error instanceof GeoDbError && error.reason === "unmapped_space") return
		recordShadow("failed")
		deps.log.warn("geodb card shadow read failed", {
			entityId,
			spaceId,
			reason: error instanceof GeoDbError ? error.reason : "error",
			message: String(error),
		})
	}
}

async function postgresStatements(deps: CardDeps, entityId: string, spaceId: string, timed = false) {
	const started = performance.now()
	try {
		const {values, relations} = await deps.readPostgres(entityId, spaceId)
		// The conversion is inside the bracket: geo-db's timer includes turning the pipe's text into rows, so
		// both stores are timed from the request to usable statements.
		const statements = statementsFromPostgres(values, relations)
		if (timed) observePostgresCardRead((performance.now() - started) / 1000, "ok")
		return statements
	} catch (error) {
		if (timed) observePostgresCardRead((performance.now() - started) / 1000, "error")
		throw error
	}
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
	// Only the primary read is timed; the fallback above runs after a geo-db failure and would skew the comparison.
	const postgres = await postgresStatements(deps, entityId, spaceId, deps.mode === "shadow")
	recordServed("postgres")
	const card: EntityCard = {entityId, spaceId, source: "postgres", statements: postgres.statements.map(view)}
	return deps.mode === "shadow"
		? {card, shadow: shadowCompare(deps, entityId, spaceId, postgres, geoEntityId)}
		: {card}
}
