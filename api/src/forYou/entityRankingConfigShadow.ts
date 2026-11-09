/**
 * The entity_ranking_config shadow (GEODB_ENTITY_RANKING_CONFIG_SHADOW, off by default).
 *
 * The ranking plpgsql functions read `entity_ranking_config` inside Postgres, which cannot read geo-db, so they are not
 * shadowed. The one TypeScript reader is `readSettings` (queries.ts), which serves `tau_seconds` to the For you route.
 * The geodb-feeder mirrors the table's one row into the `config` system Space (GEODB_ENTITY_RANKING_CONFIG backfill).
 * When the flag is on, a background sampler (`createIntervalSampler`, every GEODB_ENTITY_RANKING_CONFIG_SHADOW_INTERVAL_MS,
 * default five minutes) reads the settings from Postgres the way the route does, reads the record from geo-db and counts
 * whether `tau_seconds` is equal: `gaia_api_entity_ranking_config_shadow_total{result}`. The other 30 columns are checked by
 * the feeder's gate (entityRankingConfigGate.ts), not live. It used to run inside /for-you requests, throttled per pod, and
 * so gave next to no samples; it now costs a request nothing. Never changes a response; every error is swallowed into a
 * counter: `failed` is geo-db's, `reference_timeout` / `reference_failed` are the Postgres read's (referenceSide.ts).
 */
import type {GeoCardRow} from "../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../geodb/client"
import {createIntervalSampler, type IntervalSampler, readIntervalSamplerConfig} from "../geodb/intervalSampler"
import {failureResult, viaReference} from "../geodb/referenceSide"
import {intPart, recordEntityId} from "../proposals/governance/recordKey"
import type {AnySystemSpaceLookup} from "../proposals/governance/systemSpaces"
import {type Database, type ForYouSettings, readSettings} from "./queries"

/** The feeder's pins (geodb-feeder/src/recordKinds.ts, entityRankingConfig); a test holds them to that file. */
export const ENTITY_RANKING_CONFIG_KIND = "ee4e36fde6334be9b6d45085b5f6fb34"
export const ENTITY_RANKING_CONFIG_COLUMNS = {
	tauSeconds: "5c83400bff444efca45f68eb07fa9b09",
} as const

/** Entity id of the one record, keyed by the table's single row (`id = true`, the feeder writes key 1). */
export const entityRankingConfigRecordId = (): string => recordEntityId(ENTITY_RANKING_CONFIG_KIND, [intPart(1)])

export class AmbiguousConfigError extends Error {}

/** `tau_seconds` from the record's card, or null when the record does not exist (no row at all). */
export function decodeTauSeconds(rows: GeoCardRow[]): number | null {
	if (rows.length === 0) return null
	const property = ENTITY_RANKING_CONFIG_COLUMNS.tauSeconds
	const values = new Set(rows.filter((r) => r.prop === property).map((r) => r.raw))
	if (values.size > 1) throw new AmbiguousConfigError(`column tau_seconds holds ${values.size} values`)
	const raw = [...values][0]
	if (raw === undefined) throw new AmbiguousConfigError("column tau_seconds missing")
	const n = Number(raw)
	if (raw.trim() === "" || !Number.isFinite(n)) throw new AmbiguousConfigError(`not a number: ${raw}`)
	return n
}

export type EntityRankingConfigShadowConfig = {enabled: boolean; intervalMs: number}

export function readEntityRankingConfigShadowConfig(
	env: Record<string, string | undefined>,
): EntityRankingConfigShadowConfig {
	return readIntervalSamplerConfig(
		env,
		"GEODB_ENTITY_RANKING_CONFIG_SHADOW",
		"GEODB_ENTITY_RANKING_CONFIG_SHADOW_INTERVAL_MS",
	)
}

export const RESULTS = ["match", "mismatch", "unsupported", "failed", "reference_timeout", "reference_failed"] as const
export const UNSUPPORTED_REASONS = ["system_space_missing", "record_missing", "ambiguous_record", "busy"] as const
export type Result = (typeof RESULTS)[number]
export type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]

let active = false
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const bump = (map: Map<string, number>, key: string): void => {
	map.set(key, (map.get(key) ?? 0) + 1)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderEntityRankingConfigMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_entity_ranking_config_shadow_total The entity_ranking_config tau_seconds geo-db holds against the one the For you route served from Postgres, by result: match, mismatch, unsupported (geo-db cannot answer) failed (geo-db read failed), reference_timeout (the Postgres read hit its statement timeout) or reference_failed (any other Postgres error).",
		"# TYPE gaia_api_entity_ranking_config_shadow_total counter",
	]
	for (const r of RESULTS)
		lines.push(`gaia_api_entity_ranking_config_shadow_total{result="${r}"} ${results.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_entity_ranking_config_shadow_unsupported_total entity_ranking_config comparisons geo-db could not answer, by reason.",
		"# TYPE gaia_api_entity_ranking_config_shadow_unsupported_total counter",
	)
	for (const r of UNSUPPORTED_REASONS)
		lines.push(`gaia_api_entity_ranking_config_shadow_unsupported_total{reason="${r}"} ${unsupported.get(r) ?? 0}`)
	return lines.join("\n")
}

export function __resetEntityRankingConfigMetricsForTests(): void {
	active = false
	results.clear()
	unsupported.clear()
}

class Unsupported extends Error {
	constructor(readonly reason: UnsupportedReason) {
		super(reason)
	}
}

export type EntityRankingConfigShadow = IntervalSampler

export function createEntityRankingConfigShadow(deps: {
	config: EntityRankingConfigShadowConfig
	db: Database
	geoDb: GeoDbCardClient
	systemSpaceOf: AnySystemSpaceLookup
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	/** The Postgres reference; the route's own `readSettings` unless a test supplies one. */
	readSettings?: (db: Database) => Promise<ForYouSettings>
}): EntityRankingConfigShadow {
	const {config, db, geoDb, systemSpaceOf, log} = deps
	const read = deps.readSettings ?? readSettings
	active = true
	let logged = 0

	async function pass() {
		try {
			const served = (await viaReference(read(db))).tauSeconds
			const geoSpaceId = await systemSpaceOf("config")
			if (!geoSpaceId) throw new Unsupported("system_space_missing")
			const held = decodeTauSeconds(await geoDb.readCard({geoSpaceId, entityId: entityRankingConfigRecordId()}))
			if (held === null) throw new Unsupported("record_missing")
			const match = served === held
			bump(results, match ? "match" : "mismatch")
			if (!match && logged < 20) {
				logged++
				log.warn("entity-ranking config shadow mismatch", {tauSeconds: {served, geoDb: held}})
			}
		} catch (error) {
			if (error instanceof Unsupported) {
				bump(results, "unsupported")
				bump(unsupported, error.reason)
			} else if (error instanceof AmbiguousConfigError) {
				bump(results, "unsupported")
				bump(unsupported, "ambiguous_record")
			} else {
				const result = failureResult(error)
				bump(results, result)
				if (result !== "failed" || !(error instanceof GeoDbError))
					log.warn("entity-ranking config shadow failed", {result, error: String(error)})
			}
		}
	}

	return createIntervalSampler({
		enabled: config.enabled,
		intervalMs: config.intervalMs,
		pass,
		onBusy: () => {
			bump(results, "unsupported")
			bump(unsupported, "busy")
		},
		log,
		label: "entity-ranking config shadow",
	})
}
