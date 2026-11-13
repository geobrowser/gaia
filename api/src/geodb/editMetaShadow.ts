/**
 * The edit-meta twin of `/versioned/entities/:id/versions` (GEODB_EDIT_META_SHADOW, off by default; plan 10.22 section 22,
 * KG-EDIT-BLOCK-META-HOME).
 *
 * kg-indexer keeps each edit's chain block, order in the block and name in `edit_versions` (`block_number`, `sequence`,
 * `name`). The geodb-feeder mirrors them, behind GEODB_EDIT_META, as one natural-key record per edit in the `governance`
 * system Space (geodb-feeder/src/editMetaKeyed.ts). This file is the read twin: `readEditMeta` reads those records for a set
 * of edit ids, and the shadow compares what a served `/versions` page says (`blockNumber`, `name`) with them. Nothing is
 * served from geo-db: the flag only counts.
 *
 * A sampled request is compared after the response is built, fire-and-forget, never awaited, and every error is swallowed
 * (the same shape as entityVersionsShadow.ts). At most `MAX_ROWS` of the served page's edits are read, in one batch read.
 *
 * `gaia_api_edit_meta_shadow_total{result}`: match (every served edit has a record that agrees), mismatch (a record
 * disagrees), missing (some edit has no record and none disagrees: the feeder has not reached it, or it predates the topic's
 * retention), unsupported (the `governance` Space is not in the system-space map, or the client cannot batch-read) or
 * failed (a geo-db read failed). `..._mismatch_total{field}`: block, name, ambiguous (a column holds two values).
 * `..._skipped_total{reason}`: sample or rate. A mismatch warn (first 20) logs the entity and edit id, never a value beyond the two blocks.
 */

import {idPart, recordEntityId} from "../proposals/governance/recordKey"
import type {AnySystemSpaceLookup} from "../proposals/governance/systemSpaces"
import type {GeoCardRow} from "./card"
import {type GeoDbCardClient, GeoDbError} from "./client"

/** The feeder's pins (geodb-feeder/src/recordKinds.ts); `__tests__/editMetaShadow.test.ts` holds them to that file. */
export const EDIT_META_KIND = "cc660587787041049babf9da7f029485"
export const EDIT_META_COLUMNS = {
	editId: "6f0f639e8505469fa4b789751a4e5c43",
	block: "27bd10d8489e4299a244afdfcbf079ed",
	sequence: "991fc3cc35a34d1393bbbecb613672cb",
	name: "7f65c9f311f346a984cb125e7d9db946",
} as const

export const editMetaRecordId = (editId: string): string => recordEntityId(EDIT_META_KIND, [idPart(editId)])

export type EditMeta = {block: number; sequence: number | null; name: string | null}

/** One column's value, null when the record has no such column; two different values are ambiguous (null and flagged). */
function column(rows: GeoCardRow[], property: string): {value: string | null; ambiguous: boolean} {
	const values = new Set(rows.filter((r) => r.prop === property).map((r) => r.raw))
	if (values.size > 1) return {value: null, ambiguous: true}
	return {value: [...values][0] ?? null, ambiguous: false}
}

/** A decoded record: null when the edit has no record (no rows), `ambiguous` when a column holds two values or the block is not a number. */
export type DecodedEditMeta = {meta: EditMeta} | {ambiguous: true} | null

export function decodeEditMeta(rows: GeoCardRow[]): DecodedEditMeta {
	if (rows.length === 0) return null
	const block = column(rows, EDIT_META_COLUMNS.block)
	const sequence = column(rows, EDIT_META_COLUMNS.sequence)
	const name = column(rows, EDIT_META_COLUMNS.name)
	const blockNumber = block.value === null || block.value.trim() === "" ? Number.NaN : Number(block.value)
	if (block.ambiguous || sequence.ambiguous || name.ambiguous || !Number.isFinite(blockNumber))
		return {ambiguous: true}
	const seq = sequence.value === null ? Number.NaN : Number(sequence.value)
	return {meta: {block: blockNumber, sequence: Number.isFinite(seq) ? seq : null, name: name.value}}
}

/** Thrown when the `governance` system Space is not in the system-space map. */
export class GovernanceSpaceMissing extends Error {}

export type EditMetaReader = {
	/** The record of each edit id, keyed by edit id; an edit with no record is absent from the map. */
	read(editIds: readonly string[]): Promise<Map<string, DecodedEditMeta & object>>
}

export function createEditMetaReader(deps: {
	geoDb: Pick<GeoDbCardClient, "readCards">
	systemSpaceOf: AnySystemSpaceLookup
}): EditMetaReader {
	return {
		async read(editIds) {
			const out = new Map<string, DecodedEditMeta & object>()
			if (editIds.length === 0) return out
			const geoSpaceId = await deps.systemSpaceOf("governance")
			if (!geoSpaceId) throw new GovernanceSpaceMissing()
			if (!deps.geoDb.readCards) throw new GeoDbError("client_failed", "readCards is not available")
			const byRecord = new Map(editIds.map((id) => [editMetaRecordId(id), id]))
			const rows = await deps.geoDb.readCards({geoSpaceIds: [geoSpaceId], entityIds: [...byRecord.keys()]})
			const perRecord = new Map<string, GeoCardRow[]>()
			for (const r of rows) if (r.entity) perRecord.set(r.entity, [...(perRecord.get(r.entity) ?? []), r])
			for (const [record, editId] of byRecord) {
				const decoded = decodeEditMeta(perRecord.get(record) ?? [])
				if (decoded) out.set(editId, decoded)
			}
			return out
		},
	}
}

export type EditMetaShadowConfig = {
	enabled: boolean
	/** One request in this many is compared. */
	sampleEvery: number
	/** Most comparisons started a minute. */
	maxPerMinute: number
	/** Most comparisons running at once; a new one past it is dropped (counted `unsupported`). */
	maxInFlight: number
}

const DEFAULTS = {sampleEvery: 10, maxPerMinute: 30, maxInFlight: 2}
const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readEditMetaShadowConfig(env: Record<string, string | undefined>): EditMetaShadowConfig {
	const flag = env.GEODB_EDIT_META_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		sampleEvery: wholeNumber(env.GEODB_EDIT_META_SHADOW_SAMPLE_EVERY, DEFAULTS.sampleEvery, 100_000),
		maxPerMinute: wholeNumber(env.GEODB_EDIT_META_SHADOW_MAX_PER_MINUTE, DEFAULTS.maxPerMinute, 6000),
		maxInFlight: wholeNumber(env.GEODB_EDIT_META_SHADOW_MAX_IN_FLIGHT, DEFAULTS.maxInFlight, 16),
	}
}

export const RESULTS = ["match", "mismatch", "missing", "unsupported", "failed"] as const
export const MISMATCH_FIELDS = ["block", "name", "ambiguous"] as const
export const SKIP_REASONS = ["sample", "rate"] as const
type Result = (typeof RESULTS)[number]
type MismatchField = (typeof MISMATCH_FIELDS)[number]
type SkipReason = (typeof SKIP_REASONS)[number]

/** Most of a served page's edits read in one comparison. */
export const MAX_ROWS = 32

let active = false
const results = new Map<Result, number>()
const mismatches = new Map<MismatchField, number>()
const skipped = new Map<SkipReason, number>()
const bump = <K>(map: Map<K, number>, key: K): void => {
	map.set(key, (map.get(key) ?? 0) + 1)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderEditMetaShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_edit_meta_shadow_total Sampled GET /versioned/entities/:id/versions pages (blockNumber, name) from Postgres against geo-db's edit_meta records, by result: match, mismatch, missing (an edit has no record), unsupported (no governance Space, or the client cannot batch-read) or failed (a geo-db read failed).",
		"# TYPE gaia_api_edit_meta_shadow_total counter",
	]
	for (const r of RESULTS) lines.push(`gaia_api_edit_meta_shadow_total{result="${r}"} ${results.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_edit_meta_shadow_mismatch_total Edit-meta shadow mismatches by the field that differs (one comparison can differ in several).",
		"# TYPE gaia_api_edit_meta_shadow_mismatch_total counter",
	)
	for (const f of MISMATCH_FIELDS)
		lines.push(`gaia_api_edit_meta_shadow_mismatch_total{field="${f}"} ${mismatches.get(f) ?? 0}`)
	lines.push(
		"# HELP gaia_api_edit_meta_shadow_skipped_total Eligible version requests not compared, by reason: sample (not the 1-in-N picked) or rate (over the per-minute cap).",
		"# TYPE gaia_api_edit_meta_shadow_skipped_total counter",
	)
	for (const k of SKIP_REASONS)
		lines.push(`gaia_api_edit_meta_shadow_skipped_total{reason="${k}"} ${skipped.get(k) ?? 0}`)
	return lines.join("\n")
}

export function __resetEditMetaShadowMetricsForTests(): void {
	active = false
	results.clear()
	mismatches.clear()
	skipped.clear()
}

/** What the route served: the page's edits with the columns this twin checks. `name` is checked only when present. */
export type ServedEditMeta = {
	entityId: string
	versions: readonly {editId: string; blockNumber?: string | undefined; name?: string | null | undefined}[]
}

/** Whether `blockNumber` strings denote the same number (the record prints a number; Postgres a bigint as text). */
const sameBlock = (served: string, held: number): boolean => {
	const n = Number(served)
	return Number.isFinite(n) ? n === held : false
}

/** The fields on which the records differ from the served rows, and whether any edit has no record. Pure. */
export function compareEditMeta(
	served: ServedEditMeta["versions"],
	held: ReadonlyMap<string, DecodedEditMeta & object>,
): {fields: MismatchField[]; missing: boolean} {
	const fields = new Set<MismatchField>()
	let missing = false
	for (const v of served) {
		const record = held.get(v.editId)
		if (!record) {
			missing = true
			continue
		}
		if ("ambiguous" in record) {
			fields.add("ambiguous")
			continue
		}
		if (v.blockNumber !== undefined && !sameBlock(v.blockNumber, record.meta.block)) fields.add("block")
		if (v.name !== undefined && (v.name ?? null) !== record.meta.name) fields.add("name")
	}
	return {fields: MISMATCH_FIELDS.filter((f) => fields.has(f)), missing}
}

export type EditMetaShadow = {
	/** Compare a served page's `blockNumber` and `name` with geo-db's edit_meta records, in the background. */
	versions(served: ServedEditMeta): void
}

export function createEditMetaShadow(deps: {
	config: EditMetaShadowConfig
	geoDb: Pick<GeoDbCardClient, "readCards">
	systemSpaceOf: AnySystemSpaceLookup
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	now?: () => number
}): EditMetaShadow {
	const {config, log} = deps
	const now = deps.now ?? Date.now
	const reader = createEditMetaReader(deps)
	active = true
	let inFlight = 0
	let seen = 0
	let logged = 0
	let windowStart = now()
	let windowCount = 0

	async function compare(served: ServedEditMeta): Promise<void> {
		try {
			const page = served.versions.slice(0, MAX_ROWS)
			const held = await reader.read(page.map((v) => v.editId))
			const {fields, missing} = compareEditMeta(page, held)
			bump(results, fields.length > 0 ? "mismatch" : missing ? "missing" : "match")
			for (const f of fields) bump(mismatches, f)
			if ((fields.length > 0 || missing) && logged < 20) {
				logged++
				log.warn("edit meta shadow differs", {fields, missing, entityId: served.entityId, served: page.length})
			}
		} catch (error) {
			if (error instanceof GovernanceSpaceMissing) return bump(results, "unsupported")
			if (error instanceof GeoDbError && error.reason === "overloaded") return bump(results, "unsupported")
			bump(results, "failed")
			if (!(error instanceof GeoDbError)) log.warn("edit meta shadow failed", {error: String(error)})
		}
	}

	return {
		versions(served) {
			if (!config.enabled) return
			try {
				if (served.versions.length === 0) return
				if (seen++ % config.sampleEvery !== 0) return bump(skipped, "sample")
				const t = now()
				if (t - windowStart >= 60_000) {
					windowStart = t
					windowCount = 0
				}
				if (windowCount >= config.maxPerMinute) return bump(skipped, "rate")
				if (inFlight >= config.maxInFlight) return bump(results, "unsupported")
				windowCount++
				inFlight++
				// compare() catches everything it can throw; the .catch is the last line of defence for the response.
				void compare(served)
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
