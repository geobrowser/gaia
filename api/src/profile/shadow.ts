/**
 * The profile shadow (GEODB_PROFILE_SHADOW, off by default; flag-gated like the governance shadow).
 *
 * `GET /profile/address/:address`, `GET /profile/space/:spaceId` and `POST /profile/batch` answer from Postgres: the
 * Space's front page entity (the one with a Types relation to the Space type), its name value and its avatar's image
 * url. When one of them has served a profile the shadow also reads the same entity's card from geo-db and counts whether
 * geo-db gives the same name and the same avatar url. It runs after the response is built, in the background, bounded by
 * `maxInFlight`, and swallows every error: it never changes, delays or fails a response.
 *
 * What it does not compare: which entity is the front page. The profile's `entityId` is Postgres's answer and geo-db is
 * asked about that entity, so a profile whose Postgres answer has no front page entity is counted
 * `unsupported{no_front_page}` (geo-db has no cheap way to find it from the Space alone). An entity whose card
 * geo-db does not hold in the Space is a `mismatch` of what Postgres served, not unsupported, unless the Space itself
 * is not loaded (`unmapped_space`).
 *
 * Counts only: no id, name or url is ever logged.
 */

import {ContentIds, SystemIds} from "@geoprotocol/geo-sdk"
import {conformingId, type GeoCardRow} from "../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../geodb/client"
import type {Profile} from "./types"

export type ProfileShadowConfig = {
	enabled: boolean
	/** Most profiles of one response compared, from the front. */
	maxPerResponse: number
	/** Most comparisons running at once; a new one past it is dropped (counted `busy`). */
	maxInFlight: number
}

const DEFAULTS = {maxPerResponse: 5, maxInFlight: 8}
const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readProfileShadowConfig(env: Record<string, string | undefined>): ProfileShadowConfig {
	const flag = env.GEODB_PROFILE_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		maxPerResponse: wholeNumber(env.GEODB_PROFILE_SHADOW_MAX_PER_RESPONSE, DEFAULTS.maxPerResponse, 100),
		maxInFlight: wholeNumber(env.GEODB_PROFILE_SHADOW_MAX_IN_FLIGHT, DEFAULTS.maxInFlight, 64),
	}
}

export const SURFACES = ["address", "space", "batch"] as const
export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
export const UNSUPPORTED_REASONS = ["no_front_page", "unmapped_space", "ambiguous", "busy"] as const
export type Surface = (typeof SURFACES)[number]
export type Result = (typeof RESULTS)[number]
export type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]

let active = false
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const bump = (map: Map<string, number>, key: string): void => {
	map.set(key, (map.get(key) ?? 0) + 1)
}
const recordResult = (surface: Surface, result: Result) => bump(results, `${surface}\t${result}`)
const recordUnsupported = (surface: Surface, reason: UnsupportedReason) => {
	recordResult(surface, "unsupported")
	bump(unsupported, `${surface}\t${reason}`)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderProfileShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_profile_shadow_total Profiles served from Postgres against geo-db's card of the same entity (name and avatar url), by surface and result: match, mismatch, unsupported (geo-db cannot answer) or failed (a geo-db read failed).",
		"# TYPE gaia_api_profile_shadow_total counter",
	]
	for (const s of SURFACES)
		for (const r of RESULTS)
			lines.push(`gaia_api_profile_shadow_total{surface="${s}",result="${r}"} ${results.get(`${s}\t${r}`) ?? 0}`)
	lines.push(
		"# HELP gaia_api_profile_shadow_unsupported_total Profile shadow comparisons geo-db could not answer, by surface and reason.",
		"# TYPE gaia_api_profile_shadow_unsupported_total counter",
	)
	for (const s of SURFACES)
		for (const r of UNSUPPORTED_REASONS)
			lines.push(
				`gaia_api_profile_shadow_unsupported_total{surface="${s}",reason="${r}"} ${unsupported.get(`${s}\t${r}`) ?? 0}`,
			)
	return lines.join("\n")
}

export function __resetProfileShadowMetricsForTests(): void {
	active = false
	results.clear()
	unsupported.clear()
}

const NAME = conformingId("entity", SystemIds.NAME_PROPERTY)
const AVATAR = conformingId("entity", ContentIds.AVATAR_PROPERTY)
const IMAGE_URL = conformingId("entity", SystemIds.IMAGE_URL_PROPERTY)

export type ProfileShadow = {
	/** Compare served profiles (the first `maxPerResponse`) with geo-db's cards, in the background. */
	profiles(surface: Surface, served: readonly Profile[]): void
}

export function createProfileShadow(deps: {
	config: ProfileShadowConfig
	geoDb: GeoDbCardClient
	/** gaia space id (compact) -> geo-db space id, or null when the space is not loaded. */
	geoSpaceOf: (gaiaSpaceId: string) => Promise<string | null>
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
}): ProfileShadow {
	const {config, geoDb, geoSpaceOf, log} = deps
	active = true
	let inFlight = 0
	let logged = 0

	const read = (geoSpaceId: string, entityId: string): Promise<GeoCardRow[]> => geoDb.readCard({geoSpaceId, entityId})

	/** Postgres serves an empty text as a name; geo-db holds no empty text, so both mean none. */
	const textOf = (rows: GeoCardRow[], prop: string): string[] => rows.filter((r) => r.prop === prop).map((r) => r.raw)

	async function compare(surface: Surface, p: Profile): Promise<void> {
		if (!p.entityId) return recordUnsupported(surface, "no_front_page")
		try {
			const geoSpaceId = await geoSpaceOf(p.spaceId)
			if (!geoSpaceId) return recordUnsupported(surface, "unmapped_space")
			const entityId = conformingId("entity", p.entityId)
			const card = await read(geoSpaceId, entityId)

			const names = textOf(card, NAME).filter((n) => n !== "")
			const servedName = p.name === null || p.name === "" ? null : p.name
			// Postgres picks one of several names arbitrarily (LIMIT 1): any held name is the served one.
			const nameOk = servedName === null ? names.length === 0 : names.includes(servedName)

			const avatars = [...new Set(textOf(card, AVATAR))]
			let avatarOk: boolean
			if (avatars.length > 1) return recordUnsupported(surface, "ambiguous")
			if (avatars.length === 0) {
				avatarOk = p.avatarUrl === null
			} else {
				const urls = textOf(await read(geoSpaceId, avatars[0] as string), IMAGE_URL).filter((u) => u !== "")
				avatarOk = p.avatarUrl === null ? urls.length === 0 : urls.includes(p.avatarUrl)
			}

			const match = nameOk && avatarOk
			recordResult(surface, match ? "match" : "mismatch")
			if (!match && logged < 20) {
				logged++
				log.warn("profile shadow mismatch", {
					surface,
					name: nameOk ? "match" : "mismatch",
					avatar: avatarOk ? "match" : "mismatch",
				})
			}
		} catch (error) {
			if (error instanceof GeoDbError && error.reason === "unmapped_space")
				return recordUnsupported(surface, "unmapped_space")
			recordResult(surface, "failed")
			if (!(error instanceof GeoDbError)) log.warn("profile shadow failed", {error: String(error)})
		}
	}

	return {
		profiles(surface, served) {
			if (!config.enabled) return
			// A default profile (no such Space) has its address or id in both fields: nothing was served from a Space.
			for (const p of served.filter((x) => x.address !== x.spaceId).slice(0, config.maxPerResponse)) {
				if (inFlight >= config.maxInFlight) {
					recordUnsupported(surface, "busy")
					continue
				}
				inFlight++
				// compare() catches everything it can throw; the .catch is the last line of defence for the response.
				void compare(surface, p)
					.catch(() => {})
					.finally(() => {
						inFlight--
					})
			}
		},
	}
}
