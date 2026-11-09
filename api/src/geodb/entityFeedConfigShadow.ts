/**
 * The entity-feed config reader twins (ZPL-4; zero-Postgres critical path). Three operator-edited tables the ranking and
 * feed SQL reads inside Postgres, mirrored by the geodb-feeder as natural-key records in the `config` system Space
 * (geodb-feeder/src/entityFeedConfigKeyed.ts):
 *
 *   - `entity_type_weights`    (one record per type_id: weight, note)      flag GEODB_ENTITY_TYPE_WEIGHTS_SHADOW
 *   - `entity_type_exclusions` (one record per excluded type_id: note)     flag GEODB_ENTITY_TYPE_EXCLUSIONS_SHADOW
 *   - `entity_feed_blocklist`  (one record per blocked entity_id: reason)  flag GEODB_ENTITY_FEED_BLOCKLIST_SHADOW
 *
 * The twins (`createEntityFeedConfigReaders`) read one record's card from geo-db and decode it: a type's weight, whether a
 * type is excluded, whether an entity is blocked. They serve nothing: every flag is off by default and reads stay on
 * Postgres. When a flag is on, a background sampler reads up to `sample` rows of that table from Postgres every
 * `intervalMs`, asks the twin for the same key and counts the outcome:
 *
 * `gaia_api_entity_feed_config_shadow_total{table,result}`:
 *  - match: the record holds the row's value (weights within 1e-9 relative; note / reason exactly, absent for null);
 *  - mismatch: the record holds a different value, or two values for one column;
 *  - missing: geo-db has no record for a row Postgres has (the backfill Job was not run, or the row is newer than it);
 *  - unsupported: the `config` system Space is not in the system-space map;
 *  - failed: a geo-db read failed;
 *  - reference_timeout / reference_failed: the Postgres sample read hit its statement timeout, or failed otherwise (referenceSide.ts).
 * The timer is `createIntervalSampler` (geodb/intervalSampler.ts), shared with the other config shadows.
 * Blind spot: only rows Postgres has are sampled, so a record geo-db keeps after the row was deleted from Postgres is not
 * seen (the feeder's gate reports that). Logs counts only, never a key.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {idPart, recordEntityId} from "../proposals/governance/recordKey"
import type {AnySystemSpaceLookup} from "../proposals/governance/systemSpaces"
import type {GeoCardRow} from "./card"
import {type GeoDbCardClient, GeoDbError} from "./client"
import {createIntervalSampler, DEFAULT_SAMPLER_INTERVAL_MS, type IntervalSampler} from "./intervalSampler"
import {failureResult, viaReference} from "./referenceSide"

/** The feeder's pins (geodb-feeder/src/recordKinds.ts); `__tests__/entityFeedConfigShadow.test.ts` holds them to that file. */
export const ENTITY_TYPE_WEIGHT_KIND = "96a0897a18d240db9ce98df56649e1a6"
export const ENTITY_TYPE_EXCLUSION_KIND = "b3d4c18e3754433c95d3e31f628dca75"
export const ENTITY_FEED_BLOCK_KIND = "1f351a8ff80c4a8c9c6c01b830d60a6f"
export const ENTITY_TYPE_WEIGHT_COLUMNS = {
	typeId: "f8e4916a4af94f3784aaa2c3e55c8b7a",
	weight: "36b085283e13476e9ccb657f5dade50a",
	note: "fed9b4ddefba4f238e36d89e765f3a3c",
} as const
export const ENTITY_TYPE_EXCLUSION_COLUMNS = {
	typeId: "83cd54ae27b34a8688c3b3bc54d7d59c",
	note: "9c9d13fbf7e04b26bc2e841f227f3a10",
} as const
export const ENTITY_FEED_BLOCK_COLUMNS = {
	entityId: "e545438c24c54980a9aad2dedb9ff465",
	reason: "7c07f1706dbc44e3a654fd6e8c08f883",
} as const

export const entityTypeWeightRecordId = (typeId: string): string =>
	recordEntityId(ENTITY_TYPE_WEIGHT_KIND, [idPart(typeId)])
export const entityTypeExclusionRecordId = (typeId: string): string =>
	recordEntityId(ENTITY_TYPE_EXCLUSION_KIND, [idPart(typeId)])
export const entityFeedBlockRecordId = (entityId: string): string =>
	recordEntityId(ENTITY_FEED_BLOCK_KIND, [idPart(entityId)])

export class AmbiguousConfigError extends Error {}

/** One column's value, null when the record has no such column; two different values are ambiguous. */
function single(rows: GeoCardRow[], property: string): string | null {
	const values = new Set(rows.filter((r) => r.prop === property).map((r) => r.raw))
	if (values.size > 1) throw new AmbiguousConfigError(`column ${property} holds ${values.size} values`)
	return [...values][0] ?? null
}

export type TypeWeight = {weight: number; note: string | null}

/** A type's weight record, or null when the card has no rows. A record with no (or a non-numeric) weight is ambiguous. */
export function decodeEntityTypeWeight(rows: GeoCardRow[]): TypeWeight | null {
	if (rows.length === 0) return null
	const raw = single(rows, ENTITY_TYPE_WEIGHT_COLUMNS.weight)
	const weight = raw === null || raw.trim() === "" ? Number.NaN : Number(raw)
	if (!Number.isFinite(weight)) throw new AmbiguousConfigError("weight missing or not a number")
	return {weight, note: single(rows, ENTITY_TYPE_WEIGHT_COLUMNS.note)}
}

/** An exclusion record (presence plus an optional note), or null when the card has no rows. */
export function decodeEntityTypeExclusion(rows: GeoCardRow[]): {note: string | null} | null {
	if (rows.length === 0) return null
	return {note: single(rows, ENTITY_TYPE_EXCLUSION_COLUMNS.note)}
}

/** A blocklist record (presence plus an optional reason), or null when the card has no rows. */
export function decodeEntityFeedBlock(rows: GeoCardRow[]): {reason: string | null} | null {
	if (rows.length === 0) return null
	return {reason: single(rows, ENTITY_FEED_BLOCK_COLUMNS.reason)}
}

/** Thrown when the `config` system Space is not in the system-space map. */
export class ConfigSpaceMissing extends Error {}

export type EntityFeedConfigReaders = {
	/** The weight record of a type, or null when geo-db holds none (the SQL: no row, so weight 1). */
	typeWeight(typeId: string): Promise<TypeWeight | null>
	/** The exclusion record of a type, or null when the type is not excluded. */
	typeExclusion(typeId: string): Promise<{note: string | null} | null>
	/** The blocklist record of an entity, or null when it is not blocked. */
	feedBlock(entityId: string): Promise<{reason: string | null} | null>
}

export function createEntityFeedConfigReaders(deps: {
	geoDb: Pick<GeoDbCardClient, "readCard">
	systemSpaceOf: AnySystemSpaceLookup
}): EntityFeedConfigReaders {
	const card = async (entityId: string): Promise<GeoCardRow[]> => {
		const geoSpaceId = await deps.systemSpaceOf("config")
		if (!geoSpaceId) throw new ConfigSpaceMissing()
		return deps.geoDb.readCard({geoSpaceId, entityId})
	}
	return {
		async typeWeight(typeId) {
			return decodeEntityTypeWeight(await card(entityTypeWeightRecordId(typeId)))
		},
		async typeExclusion(typeId) {
			return decodeEntityTypeExclusion(await card(entityTypeExclusionRecordId(typeId)))
		},
		async feedBlock(entityId) {
			return decodeEntityFeedBlock(await card(entityFeedBlockRecordId(entityId)))
		},
	}
}

export const TABLES = ["entity_type_weights", "entity_type_exclusions", "entity_feed_blocklist"] as const
export type Table = (typeof TABLES)[number]
export const RESULTS = [
	"match",
	"mismatch",
	"missing",
	"unsupported",
	"failed",
	"reference_timeout",
	"reference_failed",
] as const
export type Result = (typeof RESULTS)[number]

export type EntityFeedConfigShadowConfig = {
	enabled: Record<Table, boolean>
	intervalMs: number
	sample: number
}

const FLAGS: Record<Table, string> = {
	entity_type_weights: "GEODB_ENTITY_TYPE_WEIGHTS_SHADOW",
	entity_type_exclusions: "GEODB_ENTITY_TYPE_EXCLUSIONS_SHADOW",
	entity_feed_blocklist: "GEODB_ENTITY_FEED_BLOCKLIST_SHADOW",
}

const wholeNumberEnv = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readEntityFeedConfigShadowConfig(
	env: Record<string, string | undefined>,
): EntityFeedConfigShadowConfig {
	const on = (name: string) => ["1", "on", "true"].includes(env[name]?.trim().toLowerCase() ?? "")
	return {
		enabled: {
			entity_type_weights: on(FLAGS.entity_type_weights),
			entity_type_exclusions: on(FLAGS.entity_type_exclusions),
			entity_feed_blocklist: on(FLAGS.entity_feed_blocklist),
		},
		intervalMs: wholeNumberEnv(
			env.GEODB_ENTITY_FEED_CONFIG_SHADOW_INTERVAL_MS,
			DEFAULT_SAMPLER_INTERVAL_MS,
			86_400_000,
		),
		sample: wholeNumberEnv(env.GEODB_ENTITY_FEED_CONFIG_SHADOW_SAMPLE, 20, 200),
	}
}

/** A sampled Postgres row, as text (`weight` is the numeric cast to text). */
export type ConfigRow =
	| {table: "entity_type_weights"; key: string; weight: string; note: string | null}
	| {table: "entity_type_exclusions"; key: string; note: string | null}
	| {table: "entity_feed_blocklist"; key: string; reason: string | null}

const sameNumber = (a: number, b: number) => Math.abs(a - b) <= 1e-9 * Math.max(1, Math.abs(a), Math.abs(b))

/** Whether geo-db's decoded record says what the Postgres row says. Pure. */
export function compareConfigRow(
	row: ConfigRow,
	held: TypeWeight | {note: string | null} | {reason: string | null} | null,
): "match" | "mismatch" | "missing" {
	if (held === null) return "missing"
	if (row.table === "entity_type_weights") {
		const h = held as TypeWeight
		return sameNumber(Number(row.weight), h.weight) && row.note === h.note ? "match" : "mismatch"
	}
	if (row.table === "entity_type_exclusions")
		return row.note === (held as {note: string | null}).note ? "match" : "mismatch"
	return row.reason === (held as {reason: string | null}).reason ? "match" : "mismatch"
}

let active = new Set<Table>()
const results = new Map<string, number>()
const bump = (table: Table, result: Result): void => {
	const k = `${table}\t${result}`
	results.set(k, (results.get(k) ?? 0) + 1)
}

/** Empty until a table's shadow is mounted, so a flag-off deploy exports nothing; once on, its series exist at zero. */
export function renderEntityFeedConfigShadowMetrics(): string {
	if (active.size === 0) return ""
	const lines = [
		"# HELP gaia_api_entity_feed_config_shadow_total Sampled entity_type_weights / entity_type_exclusions / entity_feed_blocklist rows from Postgres against geo-db's config Space records, by table and result: match, mismatch, missing (no record), unsupported (no config Space) failed (a geo-db read failed), reference_timeout or reference_failed (the Postgres sample read).",
		"# TYPE gaia_api_entity_feed_config_shadow_total counter",
	]
	for (const t of TABLES)
		if (active.has(t))
			for (const r of RESULTS)
				lines.push(
					`gaia_api_entity_feed_config_shadow_total{table="${t}",result="${r}"} ${results.get(`${t}\t${r}`) ?? 0}`,
				)
	return lines.join("\n")
}

export function __resetEntityFeedConfigShadowMetricsForTests(): void {
	active = new Set()
	results.clear()
}

type Row = Record<string, unknown>
const rowsOf = (result: unknown): Row[] => ((result as {rows?: Row[]}).rows ?? []) as Row[]
const text = (v: unknown): string | null => (typeof v === "string" ? v : null)

/** Up to `limit` rows of `table`, a random sample (the tables are small and operator-edited). */
export async function sampleConfigRows(db: Database, table: Table, limit: number): Promise<ConfigRow[]> {
	if (table === "entity_type_weights") {
		const r = await db.execute(
			sql`SELECT type_id::text AS key, weight::text AS weight, note FROM public.entity_type_weights ORDER BY random() LIMIT ${limit}`,
		)
		return rowsOf(r).flatMap((x) =>
			text(x.key) && text(x.weight)
				? [{table, key: x.key as string, weight: x.weight as string, note: text(x.note)} as ConfigRow]
				: [],
		)
	}
	if (table === "entity_type_exclusions") {
		const r = await db.execute(
			sql`SELECT type_id::text AS key, note FROM public.entity_type_exclusions ORDER BY random() LIMIT ${limit}`,
		)
		return rowsOf(r).flatMap((x) =>
			text(x.key) ? [{table, key: x.key as string, note: text(x.note)} as ConfigRow] : [],
		)
	}
	const r = await db.execute(
		sql`SELECT entity_id::text AS key, reason FROM public.entity_feed_blocklist ORDER BY random() LIMIT ${limit}`,
	)
	return rowsOf(r).flatMap((x) =>
		text(x.key) ? [{table, key: x.key as string, reason: text(x.reason)} as ConfigRow] : [],
	)
}

export type EntityFeedConfigShadow = IntervalSampler

export function createEntityFeedConfigShadow(deps: {
	config: EntityFeedConfigShadowConfig
	db: Database
	readers: EntityFeedConfigReaders
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	sampleRows?: typeof sampleConfigRows
}): EntityFeedConfigShadow {
	const {config, db, readers, log} = deps
	const sampleRows = deps.sampleRows ?? sampleConfigRows
	active = new Set(TABLES.filter((t) => config.enabled[t]))
	let logged = 0

	const held = (row: ConfigRow) =>
		row.table === "entity_type_weights"
			? readers.typeWeight(row.key)
			: row.table === "entity_type_exclusions"
				? readers.typeExclusion(row.key)
				: readers.feedBlock(row.key)

	async function pass(table: Table): Promise<void> {
		let sampled: ConfigRow[]
		try {
			sampled = await viaReference(sampleRows(db, table, config.sample))
		} catch (error) {
			bump(table, failureResult(error))
			log.warn("entity feed config shadow sample failed", {table, error: String(error)})
			return
		}
		for (const row of sampled) {
			try {
				const outcome = compareConfigRow(row, await held(row))
				bump(table, outcome)
				if (outcome === "mismatch" && logged < 20) {
					logged++
					log.warn("entity feed config shadow mismatch", {table})
				}
			} catch (error) {
				if (error instanceof ConfigSpaceMissing) bump(table, "unsupported")
				else if (error instanceof AmbiguousConfigError) bump(table, "mismatch")
				else {
					bump(table, "failed")
					if (!(error instanceof GeoDbError))
						log.warn("entity feed config shadow failed", {table, error: String(error)})
				}
			}
		}
	}

	return createIntervalSampler({
		enabled: TABLES.some((t) => config.enabled[t]),
		intervalMs: config.intervalMs,
		pass: async () => {
			for (const t of TABLES) if (config.enabled[t]) await pass(t)
		},
		log,
		label: "entity feed config shadow",
	})
}
