/**
 * The for_you_config shadow (GEODB_FOR_YOU_CONFIG_SHADOW, off by default).
 *
 * `readSettings` serves the For you boost settings from `personalization.for_you_config`. The geodb-feeder mirrors that
 * one row into the `config` system Space (GEODB_FOR_YOU_CONFIG backfill). When the flag is on, the For you route reads the
 * record back after it has read the settings, in the background, and counts whether the four values equal the ones it
 * served: `gaia_api_for_you_config_shadow_total{result}`. It never changes, delays or fails a response: bounded by
 * `maxInFlight`, throttled to one comparison per `intervalMs` (the row changes by migration only), and every error is
 * swallowed into a counter.
 */
import type {GeoCardRow} from "../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../geodb/client"
import {intPart, recordEntityId} from "../proposals/governance/recordKey"
import type {AnySystemSpaceLookup} from "../proposals/governance/systemSpaces"
import type {ForYouConfig} from "./rank"

/** The feeder's pins (geodb-feeder/src/recordKinds.ts, forYouConfig); a test holds them to that file. */
export const FOR_YOU_CONFIG_KIND = "8a0e2bd56c834195b30db6a7eb966b50"
export const FOR_YOU_CONFIG_COLUMNS = {
	maxBoostDays: "08242c8938ef411a900081d548a247b6",
	interestHalfSaturation: "3aa038d4dd1c49778e4a11987a13d7d4",
	explorationShare: "9113b88fdcb24875a95175268f80aa57",
	maxTopics: "53d2c96a35d4404e98c144aa1593ec2b",
} as const

/** Entity id of the one record, keyed by the table's single row (`id = true`, the feeder writes key 1). */
export const forYouConfigRecordId = (): string => recordEntityId(FOR_YOU_CONFIG_KIND, [intPart(1)])

export class AmbiguousConfigError extends Error {}

function single(rows: GeoCardRow[], property: string): number | null {
	const values = new Set(rows.filter((r) => r.prop === property).map((r) => r.raw))
	if (values.size > 1) throw new AmbiguousConfigError(`column ${property} holds ${values.size} values`)
	const raw = [...values][0]
	if (raw === undefined) return null
	const n = Number(raw)
	if (raw.trim() === "" || !Number.isFinite(n)) throw new AmbiguousConfigError(`not a number: ${raw}`)
	return n
}

/** The four values from the record's card, or null when the record does not exist (no row at all). */
export function decodeForYouConfig(rows: GeoCardRow[]): ForYouConfig | null {
	if (rows.length === 0) return null
	const need = (property: string, name: string): number => {
		const v = single(rows, property)
		if (v === null) throw new AmbiguousConfigError(`column ${name} missing`)
		return v
	}
	return {
		maxBoostDays: need(FOR_YOU_CONFIG_COLUMNS.maxBoostDays, "max_boost_days"),
		interestHalfSaturation: need(FOR_YOU_CONFIG_COLUMNS.interestHalfSaturation, "interest_half_saturation"),
		explorationShare: need(FOR_YOU_CONFIG_COLUMNS.explorationShare, "exploration_share"),
		maxTopics: need(FOR_YOU_CONFIG_COLUMNS.maxTopics, "max_topics"),
	}
}

export const sameConfig = (a: ForYouConfig, b: ForYouConfig): boolean =>
	a.maxBoostDays === b.maxBoostDays &&
	a.interestHalfSaturation === b.interestHalfSaturation &&
	a.explorationShare === b.explorationShare &&
	a.maxTopics === b.maxTopics

export type ForYouConfigShadowConfig = {enabled: boolean; maxInFlight: number; intervalMs: number}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readForYouConfigShadowConfig(env: Record<string, string | undefined>): ForYouConfigShadowConfig {
	const flag = env.GEODB_FOR_YOU_CONFIG_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		maxInFlight: wholeNumber(env.GEODB_FOR_YOU_CONFIG_SHADOW_MAX_IN_FLIGHT, 2, 16),
		intervalMs: wholeNumber(env.GEODB_FOR_YOU_CONFIG_SHADOW_INTERVAL_MS, 10_000, 3_600_000),
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
export function renderForYouConfigMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_for_you_config_shadow_total The for_you_config values geo-db holds against the ones the For you route served from Postgres, by result: match, mismatch, unsupported (geo-db cannot answer) or failed (geo-db read failed).",
		"# TYPE gaia_api_for_you_config_shadow_total counter",
	]
	for (const r of RESULTS) lines.push(`gaia_api_for_you_config_shadow_total{result="${r}"} ${results.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_for_you_config_shadow_unsupported_total For you config comparisons geo-db could not answer, by reason.",
		"# TYPE gaia_api_for_you_config_shadow_unsupported_total counter",
	)
	for (const r of UNSUPPORTED_REASONS)
		lines.push(`gaia_api_for_you_config_shadow_unsupported_total{reason="${r}"} ${unsupported.get(r) ?? 0}`)
	return lines.join("\n")
}

export function __resetForYouConfigMetricsForTests(): void {
	active = false
	results.clear()
	unsupported.clear()
}

class Unsupported extends Error {
	constructor(readonly reason: UnsupportedReason) {
		super(reason)
	}
}

export type ForYouConfigShadow = {compare(served: ForYouConfig): void}

export function createForYouConfigShadow(deps: {
	config: ForYouConfigShadowConfig
	geoDb: GeoDbCardClient
	systemSpaceOf: AnySystemSpaceLookup
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): ForYouConfigShadow {
	const {config, geoDb, systemSpaceOf, log} = deps
	const now = deps.now ?? Date.now
	active = true
	let inFlight = 0
	let lastAt = Number.NEGATIVE_INFINITY
	let logged = 0

	async function run(served: ForYouConfig) {
		try {
			const geoSpaceId = await systemSpaceOf("config")
			if (!geoSpaceId) throw new Unsupported("system_space_missing")
			const held = decodeForYouConfig(await geoDb.readCard({geoSpaceId, entityId: forYouConfigRecordId()}))
			if (!held) throw new Unsupported("record_missing")
			const match = sameConfig(served, held)
			bump(results, match ? "match" : "mismatch")
			if (!match && logged < 20) {
				logged++
				log.warn("for-you config shadow mismatch", {served, geoDb: held})
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
				if (!(error instanceof GeoDbError)) log.warn("for-you config shadow failed", {error: String(error)})
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
