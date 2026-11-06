/**
 * The feed_experiments shadow (GEODB_FEED_EXPERIMENT_SHADOW, off by default).
 *
 * `personalization.feed_experiment_for_user` (plpgsql, api/drizzle/0102) picks the active experiment and the user's
 * arm; `readExperiment` (queries.ts) is its one TypeScript caller. The geodb-feeder mirrors `feed_experiments` into
 * the `config` system Space, one record per experiment id (GEODB_FEED_EXPERIMENTS backfill). When the flag is on, the
 * For you route reads the record of the experiment it served, in the background, and counts whether geo-db agrees:
 * the record is active, holds the same two arms and, for a user assigned by hash (not a member), recomputes the same
 * `interleaved` from the record's salt and share. For a member (`assignment === "member"`) it reads the
 * `feed_experiment_members` record (keyed experiment, user) and compares its `interleaved` with the served one; a missing
 * member record counts as unsupported (`member_record_missing`). It does not check that a hash-assigned user has no member
 * record (an absent record cannot tell "not a member" from "not backfilled"). `gaia_api_feed_experiment_shadow_total{result}`. It never changes, delays or fails a response:
 * bounded by `maxInFlight`, throttled to one comparison per `intervalMs`, and every error is swallowed into a counter.
 */
import {createHash} from "node:crypto"
import type {GeoCardRow} from "../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../geodb/client"
import {idPart, recordEntityId, textPart} from "../proposals/governance/recordKey"
import type {AnySystemSpaceLookup} from "../proposals/governance/systemSpaces"
import type {FeedExperiment} from "./queries"

/** The feeder's pins (geodb-feeder/src/recordKinds.ts, feedExperiment); a test holds them to that file. */
export const FEED_EXPERIMENT_KIND = "285dde3545964ab28ee580344b816704"
export const FEED_EXPERIMENT_COLUMNS = {
	id: "5302a53981a84da88c3949abe8165ec0",
	active: "d6a38e2b1f804353bceced2d49641d75",
	armA: "5823d865cf6c480181ce0d3979b1a5ad",
	armB: "62f4056881f34fcbbb26174932352e70",
	share: "20a413847f914c2ea382c24c69ffeaa9",
	salt: "e3bc692a46a644179788ca4e9de3b6c5",
} as const

/** The member record's kind and column (geodb-feeder recordKinds.ts, feedExperimentMember). */
export const FEED_EXPERIMENT_MEMBER_KIND = "38b80c6e92bb4209838afdff3d7a0f01"
export const FEED_EXPERIMENT_MEMBER_COLUMNS = {
	experiment: "5478f85923d94274a98d6fbdb527e5f5",
	user: "07d8f8c9c7884c039ae0ae4fc8f18f4c",
	interleaved: "b1e33f48965b4c90af9b45ce368019e8",
} as const

/** Keyed (experiment id, user uuid); the user part is the 16-byte uuid, dashed or not. */
export const feedExperimentMemberRecordId = (experiment: string, userId: string): string =>
	recordEntityId(FEED_EXPERIMENT_MEMBER_KIND, [textPart(experiment), idPart(userId)])

/** A member's `interleaved` from its record's card, or null when the record does not exist. */
export function decodeMemberInterleaved(rows: GeoCardRow[]): boolean | null {
	if (rows.length === 0) return null
	const values = new Set(rows.filter((r) => r.prop === FEED_EXPERIMENT_MEMBER_COLUMNS.interleaved).map((r) => r.raw))
	if (values.size > 1) throw new AmbiguousExperimentError(`member interleaved holds ${values.size} values`)
	const raw = [...values][0]
	if (raw !== "true" && raw !== "false") throw new AmbiguousExperimentError(`member interleaved: ${raw}`)
	return raw === "true"
}

export const feedExperimentRecordId = (id: string): string => recordEntityId(FEED_EXPERIMENT_KIND, [textPart(id)])

export class AmbiguousExperimentError extends Error {}

export type HeldExperiment = {active: boolean; armA: string; armB: string; share: number; salt: string}

/** The record's columns from its card, or null when the record does not exist. */
export function decodeExperiment(rows: GeoCardRow[]): HeldExperiment | null {
	if (rows.length === 0) return null
	const one = (name: keyof typeof FEED_EXPERIMENT_COLUMNS): string => {
		const values = new Set(rows.filter((r) => r.prop === FEED_EXPERIMENT_COLUMNS[name]).map((r) => r.raw))
		if (values.size > 1) throw new AmbiguousExperimentError(`column ${name} holds ${values.size} values`)
		const raw = [...values][0]
		if (raw === undefined) throw new AmbiguousExperimentError(`column ${name} missing`)
		return raw
	}
	const active = one("active")
	if (active !== "true" && active !== "false") throw new AmbiguousExperimentError(`active: ${active}`)
	const shareRaw = one("share")
	const share = Number(shareRaw)
	if (shareRaw.trim() === "" || !Number.isFinite(share)) throw new AmbiguousExperimentError(`share: ${shareRaw}`)
	return {active: active === "true", armA: one("armA"), armB: one("armB"), share, salt: one("salt")}
}

/** Postgres's `('x' || substr(md5(salt || ':' || user_id::text), 1, 8))::bit(32)::bigint / 4294967296.0 < share`. */
export function hashInterleaved(salt: string, userId: string, share: number): boolean {
	const bucket = Number.parseInt(createHash("md5").update(`${salt}:${userId}`).digest("hex").slice(0, 8), 16)
	return bucket / 4294967296 < share
}

export type FeedExperimentShadowConfig = {enabled: boolean; maxInFlight: number; intervalMs: number}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readFeedExperimentShadowConfig(env: Record<string, string | undefined>): FeedExperimentShadowConfig {
	const flag = env.GEODB_FEED_EXPERIMENT_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		maxInFlight: wholeNumber(env.GEODB_FEED_EXPERIMENT_SHADOW_MAX_IN_FLIGHT, 2, 16),
		intervalMs: wholeNumber(env.GEODB_FEED_EXPERIMENT_SHADOW_INTERVAL_MS, 10_000, 3_600_000),
	}
}

export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
export const UNSUPPORTED_REASONS = [
	"system_space_missing",
	"record_missing",
	"ambiguous_record",
	"member_record_missing",
	"busy",
] as const
export type Result = (typeof RESULTS)[number]
export type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]

let active = false
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const bump = (map: Map<string, number>, key: string): void => {
	map.set(key, (map.get(key) ?? 0) + 1)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderFeedExperimentMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_feed_experiment_shadow_total The feed experiment geo-db holds (active, arms, hash assignment) against the one the For you route served from Postgres, by result: match, mismatch, unsupported (geo-db cannot answer) or failed (geo-db read failed).",
		"# TYPE gaia_api_feed_experiment_shadow_total counter",
	]
	for (const r of RESULTS) lines.push(`gaia_api_feed_experiment_shadow_total{result="${r}"} ${results.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_feed_experiment_shadow_unsupported_total feed experiment comparisons geo-db could not answer, by reason.",
		"# TYPE gaia_api_feed_experiment_shadow_unsupported_total counter",
	)
	for (const r of UNSUPPORTED_REASONS)
		lines.push(`gaia_api_feed_experiment_shadow_unsupported_total{reason="${r}"} ${unsupported.get(r) ?? 0}`)
	return lines.join("\n")
}

export function __resetFeedExperimentMetricsForTests(): void {
	active = false
	results.clear()
	unsupported.clear()
}

class Unsupported extends Error {
	constructor(readonly reason: UnsupportedReason) {
		super(reason)
	}
}

/** What differs between the served experiment and the record, as field names. Pure. */
export function experimentDiffs(served: FeedExperiment, held: HeldExperiment, userId: string): string[] {
	const diffs: string[] = []
	if (!held.active) diffs.push("active")
	if (served.arms[0] !== held.armA || served.arms[1] !== held.armB) diffs.push("arms")
	if (served.assignment === "hash" && served.interleaved !== hashInterleaved(held.salt, userId, held.share))
		diffs.push("interleaved")
	return diffs
}

export type FeedExperimentShadow = {compare(served: FeedExperiment, userId: string): void}

export function createFeedExperimentShadow(deps: {
	config: FeedExperimentShadowConfig
	geoDb: GeoDbCardClient
	systemSpaceOf: AnySystemSpaceLookup
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): FeedExperimentShadow {
	const {config, geoDb, systemSpaceOf, log} = deps
	const now = deps.now ?? Date.now
	active = true
	let inFlight = 0
	let lastAt = Number.NEGATIVE_INFINITY
	let logged = 0

	async function run(served: FeedExperiment, userId: string) {
		try {
			const geoSpaceId = await systemSpaceOf("config")
			if (!geoSpaceId) throw new Unsupported("system_space_missing")
			const held = decodeExperiment(
				await geoDb.readCard({geoSpaceId, entityId: feedExperimentRecordId(served.id)}),
			)
			if (held === null) throw new Unsupported("record_missing")
			const diffs = experimentDiffs(served, held, userId)
			// A member's value comes from feed_experiment_members, which wins over the hash: compare it with the member record.
			if (served.assignment === "member") {
				const member = decodeMemberInterleaved(
					await geoDb.readCard({geoSpaceId, entityId: feedExperimentMemberRecordId(served.id, userId)}),
				)
				if (member === null) throw new Unsupported("member_record_missing")
				if (member !== served.interleaved) diffs.push("member_interleaved")
			}
			bump(results, diffs.length === 0 ? "match" : "mismatch")
			if (diffs.length > 0 && logged < 20) {
				logged++
				log.warn("feed experiment shadow mismatch", {experiment: served.id, fields: diffs})
			}
		} catch (error) {
			if (error instanceof Unsupported) {
				bump(results, "unsupported")
				bump(unsupported, error.reason)
			} else if (error instanceof AmbiguousExperimentError) {
				bump(results, "unsupported")
				bump(unsupported, "ambiguous_record")
			} else {
				bump(results, "failed")
				if (!(error instanceof GeoDbError)) log.warn("feed experiment shadow failed", {error: String(error)})
			}
		}
	}

	return {
		compare(served, userId) {
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
			void run(served, userId)
				.catch(() => {})
				.finally(() => {
					inFlight--
				})
		},
	}
}
