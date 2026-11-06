/**
 * The entity_ranking_config shadow (GEODB_ENTITY_RANKING_CONFIG_SHADOW, off by default).
 *
 * The ranking plpgsql functions read `entity_ranking_config` inside Postgres, which cannot read geo-db, so they are not
 * shadowed. The one TypeScript reader is `readSettings` (queries.ts), which serves `tau_seconds` to the For you route.
 * The geodb-feeder mirrors the table's one row into the `config` system Space (GEODB_ENTITY_RANKING_CONFIG backfill).
 * When the flag is on, the For you route reads the record back after it has read the settings, in the background, and
 * counts whether the record's `tau_seconds` equals the one it served: `gaia_api_entity_ranking_config_shadow_total{result}`.
 * The other 30 columns are checked by the feeder's gate (entityRankingConfigGate.ts), not live. It never changes, delays
 * or fails a response: bounded by `maxInFlight`, throttled to one comparison per `intervalMs` (the row changes by
 * migration or operator SQL only), and every error is swallowed into a counter.
 */
import type {GeoCardRow} from "../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../geodb/client"
import {intPart, recordEntityId} from "../proposals/governance/recordKey"
import type {AnySystemSpaceLookup} from "../proposals/governance/systemSpaces"

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

export type EntityRankingConfigShadowConfig = {enabled: boolean; maxInFlight: number; intervalMs: number}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readEntityRankingConfigShadowConfig(
	env: Record<string, string | undefined>,
): EntityRankingConfigShadowConfig {
	const flag = env.GEODB_ENTITY_RANKING_CONFIG_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		maxInFlight: wholeNumber(env.GEODB_ENTITY_RANKING_CONFIG_SHADOW_MAX_IN_FLIGHT, 2, 16),
		intervalMs: wholeNumber(env.GEODB_ENTITY_RANKING_CONFIG_SHADOW_INTERVAL_MS, 10_000, 3_600_000),
	}
}

export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
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
		"# HELP gaia_api_entity_ranking_config_shadow_total The entity_ranking_config tau_seconds geo-db holds against the one the For you route served from Postgres, by result: match, mismatch, unsupported (geo-db cannot answer) or failed (geo-db read failed).",
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

export type EntityRankingConfigShadow = {compare(servedTauSeconds: number): void}

export function createEntityRankingConfigShadow(deps: {
	config: EntityRankingConfigShadowConfig
	geoDb: GeoDbCardClient
	systemSpaceOf: AnySystemSpaceLookup
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): EntityRankingConfigShadow {
	const {config, geoDb, systemSpaceOf, log} = deps
	const now = deps.now ?? Date.now
	active = true
	let inFlight = 0
	let lastAt = Number.NEGATIVE_INFINITY
	let logged = 0

	async function run(served: number) {
		try {
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
				bump(results, "failed")
				if (!(error instanceof GeoDbError))
					log.warn("entity-ranking config shadow failed", {error: String(error)})
			}
		}
	}

	return {
		compare(served) {
			if (!config.enabled) return
			const t = now()
			if (t - lastAt < config.intervalMs) return
			lastAt = t
			if (inFlight >= config.maxInFlight) {
				bump(results, "unsupported")
				bump(unsupported, "busy")
				return
			}
			inFlight++
			void run(served)
				.catch(() => {})
				.finally(() => {
					inFlight--
				})
		},
	}
}
