/**
 * The proposal-diff shadow (GEODB_PROPOSAL_DIFF_SHADOW, off by default; flag-gated like the versions shadow).
 *
 * `GET /versioned/proposals/:id/diff` and `GET /versioned/proposal-groups/diff` answer from Postgres. Each diff is a
 * proposed edit set (read from the IPFS edit blob cached in `ipfs_cache`) laid against a base state. For an active
 * proposal (or an all-active group) that base is the live knowledge graph, which geo-db also holds. When the flag is on,
 * a sampled request reads the served entities' cards from geo-db (`readCards`, one Space) and counts, for every changed
 * statement that claims a base side (a value or relation change of type UPDATE or REMOVE), whether geo-db holds that
 * base statement. It runs after the response is built, fire-and-forget, never awaited by the request, and swallows
 * every error: it never changes, delays or fails a response. Metrics are counts only; the first 20 mismatches log the
 * route, the number of entities and the counts, never an entity, Space or proposal id.
 *
 * An executed or closed proposal, or a historical group, diffs against the versioned state at a resolved version key
 * (`resolveVersionKeyBeforeTimestamp`: the latest version strictly before the proposal's execution or end time, or a
 * group's earliest edit). geo-db reads a Space at a version, so the shadow resolves the same instant there: `version-at`
 * for the Space at the instant one nanosecond before that timestamp (`readVersionAt`), or, where the diff names the
 * edit that made the base, the version that edit sealed (`version-of-edit`, `readVersionOfEdit`, which needs no clock and
 * falls back to the instant where it answers nothing), then the cards read at that version's `seq` (`readCardsAt`), and counts the base statements as it does for the live base. Gaia's version key is
 * global across Spaces and its timestamps whole seconds, geo-db's versions are per Space and sealed in nanoseconds, so a
 * version sealed within the same second as the timestamp is where the two can disagree, and a `mismatch` then names
 * that boundary or a real gap alike.
 *
 * What it cannot answer, and why (each is counted `unsupported{reason}`, the gap named in the queue as its own item):
 * - `as_of`: a historical diff whose base timestamp the route did not hand over.
 * - `unmapped_space`: the request's Space is not loaded in geo-db.
 * - `no_base`: the page holds no entity with a base-side change (all ADD), so there is nothing to compare.
 * - `client`: a geo-db client without `readCards` (or, for a historical base, without `readVersionAt` and `readCardsAt`).
 * - `busy`: the pool or the in-flight bound.
 * The proposed (after) side is never compared; only the base side is. The proposal row and its actions have a geo-db
 * home (the keyed governance records, twinned by proposals/governance/queriesShadow.ts) that this shadow does not read;
 * the `ipfs_cache` edit blob the after side is decoded from has none.
 *
 * Statement matching follows the card shadow: a value matches on its property id, a relation on its type id and its
 * target, both after the loader's id remap (`conformingId`). Changes whose Space is not the request's Space, and block
 * changes, are not compared.
 *
 * `gaia_api_proposal_diff_shadow_total{result}`: match, mismatch, unsupported or failed.
 * `..._mismatch_total{field}`: base_missing (a base statement the served diff claims is not in geo-db's card).
 * `..._statements_total{outcome}`: found or missing, the base statements compared.
 * `..._unsupported_total{reason}` and `..._skipped_total{reason}` (sample, rate) as in the versions shadow.
 * `..._base_total{resolved}`: how a historical base was found, edit_id, timestamp, edit_miss or edit_unsupported.
 */

import {conformingId, type GeoCardRow} from "./card"
import {type GeoDbCardClient, GeoDbError, MAX_CARD_IDS} from "./client"

export type ProposalDiffShadowConfig = {
	enabled: boolean
	sampleEvery: number
	maxPerMinute: number
	maxInFlight: number
}

const DEFAULTS = {sampleEvery: 10, maxPerMinute: 30, maxInFlight: 2}
const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readProposalDiffShadowConfig(env: Record<string, string | undefined>): ProposalDiffShadowConfig {
	const flag = env.GEODB_PROPOSAL_DIFF_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		sampleEvery: wholeNumber(env.GEODB_PROPOSAL_DIFF_SHADOW_SAMPLE_EVERY, DEFAULTS.sampleEvery, 100_000),
		maxPerMinute: wholeNumber(env.GEODB_PROPOSAL_DIFF_SHADOW_MAX_PER_MINUTE, DEFAULTS.maxPerMinute, 6000),
		maxInFlight: wholeNumber(env.GEODB_PROPOSAL_DIFF_SHADOW_MAX_IN_FLIGHT, DEFAULTS.maxInFlight, 16),
	}
}

export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
export const MISMATCH_FIELDS = ["base_missing"] as const
export const OUTCOMES = ["found", "missing"] as const
export const UNSUPPORTED_REASONS = ["as_of", "unmapped_space", "no_base", "client", "busy"] as const
export const SKIP_REASONS = ["sample", "rate"] as const
/** How a historical base was resolved: by the id of the edit that made it, or by the timestamp (and why not the id). */
export const BASE_RESOLVED = ["edit_id", "timestamp", "edit_miss", "edit_unsupported"] as const
type Result = (typeof RESULTS)[number]
type MismatchField = (typeof MISMATCH_FIELDS)[number]
type Outcome = (typeof OUTCOMES)[number]
type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]
type SkipReason = (typeof SKIP_REASONS)[number]
type BaseResolved = (typeof BASE_RESOLVED)[number]

/** Most entities compared from one page: one geo-db card line. */
const MAX_ENTITIES = MAX_CARD_IDS

let active = false
const results = new Map<Result, number>()
const mismatches = new Map<MismatchField, number>()
const outcomes = new Map<Outcome, number>()
const unsupported = new Map<UnsupportedReason, number>()
const skipped = new Map<SkipReason, number>()
const baseResolved = new Map<BaseResolved, number>()
const bump = <K>(map: Map<K, number>, key: K, by = 1): void => {
	map.set(key, (map.get(key) ?? 0) + by)
}
const recordUnsupported = (reason: UnsupportedReason) => {
	bump(results, "unsupported")
	bump(unsupported, reason)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderProposalDiffShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_proposal_diff_shadow_total Sampled proposal and proposal-group diff responses served from Postgres, their base-side statements checked against geo-db's cards, by result: match, mismatch, unsupported (geo-db cannot answer) or failed (a geo-db read failed).",
		"# TYPE gaia_api_proposal_diff_shadow_total counter",
	]
	for (const r of RESULTS) lines.push(`gaia_api_proposal_diff_shadow_total{result="${r}"} ${results.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_proposal_diff_shadow_mismatch_total Proposal diff shadow mismatches by the field that differs.",
		"# TYPE gaia_api_proposal_diff_shadow_mismatch_total counter",
	)
	for (const f of MISMATCH_FIELDS)
		lines.push(`gaia_api_proposal_diff_shadow_mismatch_total{field="${f}"} ${mismatches.get(f) ?? 0}`)
	lines.push(
		"# HELP gaia_api_proposal_diff_shadow_statements_total Base-side statements compared, by whether geo-db held them.",
		"# TYPE gaia_api_proposal_diff_shadow_statements_total counter",
	)
	for (const o of OUTCOMES)
		lines.push(`gaia_api_proposal_diff_shadow_statements_total{outcome="${o}"} ${outcomes.get(o) ?? 0}`)
	lines.push(
		"# HELP gaia_api_proposal_diff_shadow_unsupported_total Proposal diff shadow comparisons geo-db could not answer, by reason.",
		"# TYPE gaia_api_proposal_diff_shadow_unsupported_total counter",
	)
	for (const u of UNSUPPORTED_REASONS)
		lines.push(`gaia_api_proposal_diff_shadow_unsupported_total{reason="${u}"} ${unsupported.get(u) ?? 0}`)
	lines.push(
		"# HELP gaia_api_proposal_diff_shadow_skipped_total Eligible diff requests not compared, by reason: sample (not the 1-in-N picked) or rate (over the per-minute cap).",
		"# TYPE gaia_api_proposal_diff_shadow_skipped_total counter",
	)
	for (const k of SKIP_REASONS)
		lines.push(`gaia_api_proposal_diff_shadow_skipped_total{reason="${k}"} ${skipped.get(k) ?? 0}`)
	lines.push(
		"# HELP gaia_api_proposal_diff_shadow_base_total Historical bases resolved in geo-db, by how: edit_id (the version that edit sealed), timestamp (version-at the instant; no edit id known), edit_miss (the Space holds no version of that edit, so the timestamp) or edit_unsupported (the client or node cannot read it, so the timestamp).",
		"# TYPE gaia_api_proposal_diff_shadow_base_total counter",
	)
	for (const b of BASE_RESOLVED)
		lines.push(`gaia_api_proposal_diff_shadow_base_total{resolved="${b}"} ${baseResolved.get(b) ?? 0}`)
	return lines.join("\n")
}

export function __resetProposalDiffShadowMetricsForTests(): void {
	active = false
	results.clear()
	mismatches.clear()
	outcomes.clear()
	unsupported.clear()
	skipped.clear()
	baseResolved.clear()
}

/** The part of a served entity diff the shadow reads. */
export type ServedDiffEntity = {
	entityId: string
	values: readonly {propertyId: string; spaceId: string; before: string | null}[]
	relations: readonly {
		typeId: string
		spaceId: string
		before?: {toEntityId: string} | null
	}[]
}

/** What a diff route served. `mode` is the base the diff was laid against. */
export type ServedProposalDiff = {
	route: "proposal" | "group"
	spaceId: string
	mode: "active" | "historical"
	/** A historical base's timestamp in whole seconds: the diff was laid against the state strictly before it. */
	baseTimestamp?: bigint
	/** The edit whose version key the base resolved to (the latest edit before `baseTimestamp`), when the row carried it. */
	baseEditId?: string
	entities: readonly ServedDiffEntity[]
}

export type ProposalDiffShadow = {
	/** Compare a served diff's base side with geo-db, in the background. */
	diff(served: ServedProposalDiff): void
}

type BaseStatements = {entityId: string; values: string[]; relations: {prop: string; target: string}[]}

/** The base-side statements (UPDATE and REMOVE changes in the request's Space) of the first entities that have any. */
export function baseStatements(served: ServedProposalDiff): BaseStatements[] {
	const out: BaseStatements[] = []
	for (const e of served.entities) {
		const values = e.values
			.filter((v) => v.before !== null && v.spaceId === served.spaceId)
			.map((v) => conformingId("entity", v.propertyId))
		const relations = e.relations
			.filter((r) => r.before && r.spaceId === served.spaceId)
			.map((r) => ({
				prop: conformingId("entity", r.typeId),
				target: conformingId("entity", (r.before as {toEntityId: string}).toEntityId),
			}))
		if (values.length + relations.length === 0) continue
		out.push({entityId: conformingId("entity", e.entityId), values, relations})
		if (out.length >= MAX_ENTITIES) break
	}
	return out
}

/** How many base statements geo-db's rows hold, and how many it lacks. */
export function compareBase(
	base: readonly BaseStatements[],
	rows: readonly GeoCardRow[],
): {found: number; missing: number} {
	const byEntity = new Map<string, GeoCardRow[]>()
	for (const row of rows) {
		if (row.entity === undefined) continue
		const list = byEntity.get(row.entity)
		if (list) list.push(row)
		else byEntity.set(row.entity, [row])
	}
	let found = 0
	let missing = 0
	for (const b of base) {
		const card = byEntity.get(b.entityId) ?? []
		for (const prop of b.values) card.some((r) => r.prop === prop) ? found++ : missing++
		for (const r of b.relations) card.some((c) => c.prop === r.prop && c.raw === r.target) ? found++ : missing++
	}
	return {found, missing}
}

/**
 * The cards of `entityIds` as geo-db's Space stood at the base. With the id of the base's last edit, the version that edit
 * sealed in the Space (`version-of-edit`, exact: no clock), where gaia's whole seconds against geo-db's nanoseconds blur a
 * boundary. Without one, or where that read answers nothing (no such edit in the Space, a client without the read, a node
 * older than the opcode), the version at the instant one nanosecond before `baseTimestamp` (`version-at`). Then the cards
 * at its `seq`. Any other failure (a timeout, overload) is the comparison's, not a reason to read it another way.
 */
async function readBaseAt(
	geoDb: GeoDbCardClient,
	geoSpaceId: string,
	entityIds: string[],
	baseTimestamp: bigint,
	baseEditId: string | undefined,
): Promise<GeoCardRow[]> {
	let seq: number | undefined
	let resolved: BaseResolved = "timestamp"
	if (baseEditId !== undefined) {
		if (!geoDb.readVersionOfEdit) resolved = "edit_unsupported"
		else {
			try {
				const found = await geoDb.readVersionOfEdit({
					geoSpaceId,
					editId: baseEditId.replaceAll("-", "").toLowerCase(),
				})
				if (found) {
					seq = found.seq
					resolved = "edit_id"
				} else resolved = "edit_miss"
			} catch (error) {
				if (!(error instanceof GeoDbError && error.reason === "client_failed")) throw error
				resolved = "edit_unsupported"
			}
		}
	}
	if (seq === undefined) {
		const version = await (geoDb.readVersionAt as NonNullable<GeoDbCardClient["readVersionAt"]>)({
			geoSpaceId,
			instantNs: (baseTimestamp * 1_000_000_000n - 1n).toString(),
		})
		seq = version.seq
	}
	bump(baseResolved, resolved)
	return (geoDb.readCardsAt as NonNullable<GeoDbCardClient["readCardsAt"]>)({geoSpaceId, entityIds, seq})
}

export function createProposalDiffShadow(deps: {
	config: ProposalDiffShadowConfig
	geoDb: GeoDbCardClient
	/** gaia Space id (compact) -> geo-db Space id, or null when geo-db does not hold the Space. */
	geoSpaceOf: (gaiaSpaceId: string) => Promise<string | null>
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): ProposalDiffShadow {
	const {config, geoDb, log} = deps
	const now = deps.now ?? Date.now
	active = true
	let inFlight = 0
	let seen = 0
	let logged = 0
	let windowStart = now()
	let windowCount = 0

	async function compare(served: ServedProposalDiff, base: BaseStatements[]): Promise<void> {
		try {
			const historical = served.mode === "historical"
			if (historical ? !geoDb.readVersionAt || !geoDb.readCardsAt : !geoDb.readCards)
				return recordUnsupported("client")
			const geoSpaceId = await deps.geoSpaceOf(served.spaceId.replaceAll("-", "").toLowerCase())
			if (!geoSpaceId) return recordUnsupported("unmapped_space")
			const entityIds = base.map((b) => b.entityId)
			const rows =
				historical && served.baseTimestamp !== undefined
					? await readBaseAt(geoDb, geoSpaceId, entityIds, served.baseTimestamp, served.baseEditId)
					: await (geoDb.readCards as NonNullable<GeoDbCardClient["readCards"]>)({
							geoSpaceIds: [geoSpaceId],
							entityIds,
						})
			const {found, missing} = compareBase(base, rows)
			bump(outcomes, "found", found)
			bump(outcomes, "missing", missing)
			bump(results, missing === 0 ? "match" : "mismatch")
			if (missing > 0) {
				bump(mismatches, "base_missing")
				if (logged < 20) {
					logged++
					log.warn("proposal diff shadow mismatch", {
						route: served.route,
						entities: base.length,
						found,
						missing,
					})
				}
			}
		} catch (error) {
			if (error instanceof GeoDbError && error.reason === "unmapped_space")
				return recordUnsupported("unmapped_space")
			if (error instanceof GeoDbError && error.reason === "overloaded") return recordUnsupported("busy")
			bump(results, "failed")
			if (!(error instanceof GeoDbError)) log.warn("proposal diff shadow failed", {error: String(error)})
		}
	}

	return {
		diff(served) {
			if (!config.enabled) return
			try {
				if (served.mode !== "active" && served.baseTimestamp === undefined) return recordUnsupported("as_of")
				if (seen++ % config.sampleEvery !== 0) return bump(skipped, "sample")
				const base = baseStatements(served)
				if (base.length === 0) return recordUnsupported("no_base")
				const t = now()
				if (t - windowStart >= 60_000) {
					windowStart = t
					windowCount = 0
				}
				if (windowCount >= config.maxPerMinute) return bump(skipped, "rate")
				if (inFlight >= config.maxInFlight) return recordUnsupported("busy")
				windowCount++
				inFlight++
				void compare(served, base)
					.catch(() => {})
					.finally(() => {
						inFlight--
					})
			} catch {
				// Never let the shadow reach the response.
			}
		},
	}
}
