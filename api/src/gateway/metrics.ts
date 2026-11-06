/**
 * Prometheus metrics for the GraphQL gateway's shadow comparisons, in the text format `/health/metrics` serves.
 *
 * Empty until a shadow is mounted (`activateGatewayMetrics`), so a flag-off deploy exports nothing. Once
 * active, every series for a registered field is created at zero so `increase()` sees the first event.
 */

import {__resetSearchMetricsForTests, renderSearchMetrics} from "./searchMetrics"

export const SHADOW_RESULTS = ["match", "mismatch", "unsupported_selection", "failed"] as const
/** Why a selection cannot be answered from geo-db. A closed set, so it is safe as a label. */
export const UNSUPPORTED_REASONS = [
	"not_single_root",
	"args",
	"directives",
	"selection",
	"no_space",
	"truncated",
	"variables",
	"filter",
	"batch_size",
	"many_spaces",
	"unmapped_space",
	"throttled",
	/** `metas`: the indexer cursor rows (id, cursor, block number) live only in Postgres; geo-db holds no read of them. */
	"no_geodb_home",
	/** `metas`: geo-db's `cursors` collection holds no row for an indexer PostgreSQL has (the mirror is off, or has not written yet). */
	"not_mirrored",
	/** `metas`: a `cursors` value that is not `<block>:<cursor>`. */
	"bad_layout",
] as const

/**
 * Parts of a shadowed operation that were answered by PostGraphile but not checked against geo-db, by reason.
 * Counted once per operation and reason, whatever the result: a `match` covers the compared fields only.
 */
export const UNCOMPARED_REASONS = [
	"cross_space",
	"updated_at",
	"nested_names",
	"nested_entity",
	"property_info",
	"membership_only",
	"relation_names",
	"truncated",
	"root_relations",
	"relation_edge",
	"created_at",
	"relation_count",
	"backlink_count",
	/** `backlinks { nodes }` / `backlinksList` rows: the relations pointing at the entity, which a card does not hold. */
	"backlink_rows",
	"partial_values",
	"filter_completeness",
	/** An `orderBy` page: which entities come first, and where ties fall, is not checkable from cards. */
	"order_completeness",
	/** A single-key `orderBy` page was read from geo-db with `.pick`/`.ties`, but equal keys make the order (or the cut) geo-db's own: membership only, or nothing, was compared. */
	"order_ties",
	"sampled_rows",
	/** PostGraphile has the entity's row but geo-db holds no statement about it in any loaded Space. */
	"no_statements",
	/** PostGraphile lists the entity in the asked-for Space only through derived properties, which geo-db never holds. */
	"derived_only",
	/** Vote and ranking scores PostGraphile reads from Postgres (`upvotes`, `commentScore`, `rankingScore` ..); geo-db holds none of them. */
	"derived_fields",
	/** `typeIds` selected beside `types { id name }`: the same statements twice; only the `types` list is compared. */
	"types_alias",
	/** `entitiesConnection { totalCount }`: the size of the whole filtered set, which a sample of cards cannot settle. */
	"total_count",
	/** `spaces`: PostGraphile returned a Space geo-db does not hold (the feeder leaves unloaded Spaces out of its map), so there was nothing to compare it with. */
	"space_not_held",
	/** `spaces`: a Space's `members` / `editors` lists and counts, which geo-db holds no read for. */
	"space_members",
	/** `spaces`: a Space's own settings (`type`, `address`, `topicId`, voting settings), which no entity card holds. */
	"space_settings",
	/** `spacesByTopicIdConnection`: the Spaces whose topic is the entity, a Space read no entity card holds; the rest of the entity is compared. */
	"topic_spaces",
	/** `entitiesConnection`: a `metas` indexer-cursor root asked beside it, which geo-db cannot answer. */
	"metas_cursor",
	/** `metas`: geo-db's cursor row is at a later block than the one PostgreSQL returned (read a moment later), so only freshness, not the cursor, was checked. */
	"mirror_newer",
	/** `metas`: a `filter` / `condition` / paging argument, so which indexers the page holds is PostGraphile's. */
	"meta_filter",
] as const

export type ShadowResult = (typeof SHADOW_RESULTS)[number]
export type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]
export type UncomparedReason = (typeof UNCOMPARED_REASONS)[number]

const fields = new Set<string>()
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const uncompared = new Map<string, number>()
const shapes = new Map<string, number>()

/** Most distinct (field, reason, shape) series one process exports; later shapes count as `other`. */
export const MAX_UNSUPPORTED_SHAPES = 100
const SHAPE_PATTERN = /^[A-Za-z0-9_.,;:=()[\]|+-]{1,200}$/

const bump = (map: Map<string, number>, key: string) => map.set(key, (map.get(key) ?? 0) + 1)

export function activateGatewayMetrics(field: string): void {
	fields.add(field)
}

export function recordGatewayShadow(field: string, result: ShadowResult): void {
	bump(results, `${field}\t${result}`)
}

export function recordGatewayUnsupported(field: string, reason: UnsupportedReason): void {
	bump(unsupported, `${field}\t${reason}`)
}

export function recordGatewayUncompared(field: string, reason: UncomparedReason): void {
	bump(uncompared, `${field}\t${reason}`)
}

/**
 * The argument shape (names and filter keys, never values) of an operation counted under `reason`. Bounded: a
 * shape is GraphQL names only (else `other`), and once MAX_UNSUPPORTED_SHAPES series exist a new shape is `other`.
 */
export function recordGatewayUnsupportedShape(field: string, reason: UnsupportedReason, shape: string): void {
	const safe = SHAPE_PATTERN.test(shape) ? shape : "other"
	let key = `${field}\t${reason}\t${safe}`
	if (!shapes.has(key) && shapes.size >= MAX_UNSUPPORTED_SHAPES) key = `${field}\t${reason}\tother`
	bump(shapes, key)
}

export function renderGatewayMetrics(): string {
	if (fields.size === 0) return renderSearchMetrics()
	const lines = [
		"# HELP gaia_api_gateway_shadow_total Gateway shadow comparisons by root field and result: match, mismatch, unsupported_selection (geo-db cannot answer the selection) or failed (geo-db read failed).",
		"# TYPE gaia_api_gateway_shadow_total counter",
	]
	for (const field of fields)
		for (const result of SHADOW_RESULTS)
			lines.push(
				`gaia_api_gateway_shadow_total{field="${field}",result="${result}"} ${results.get(`${field}\t${result}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_gateway_shadow_unsupported_total Shadowed operations whose selection geo-db cannot answer, by root field and reason.",
		"# TYPE gaia_api_gateway_shadow_unsupported_total counter",
	)
	for (const field of fields)
		for (const reason of UNSUPPORTED_REASONS)
			lines.push(
				`gaia_api_gateway_shadow_unsupported_total{field="${field}",reason="${reason}"} ${unsupported.get(`${field}\t${reason}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_gateway_shadow_uncompared_total Selected parts of a shadowed operation that were not checked against geo-db, by root field and reason. A match or mismatch covers only the checked parts.",
		"# TYPE gaia_api_gateway_shadow_uncompared_total counter",
	)
	for (const field of fields)
		for (const reason of UNCOMPARED_REASONS)
			lines.push(
				`gaia_api_gateway_shadow_uncompared_total{field="${field}",reason="${reason}"} ${uncompared.get(`${field}\t${reason}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_gateway_shadow_unsupported_shape_total Unsupported shadowed operations by root field, reason and argument shape (argument names and filter keys with their operators, no values).",
		"# TYPE gaia_api_gateway_shadow_unsupported_shape_total counter",
	)
	for (const [key, count] of shapes) {
		const [field, reason, shape] = key.split("\t")
		lines.push(
			`gaia_api_gateway_shadow_unsupported_shape_total{field="${field}",reason="${reason}",shape="${shape}"} ${count}`,
		)
	}
	return `${lines.join("\n")}\n${renderSearchMetrics()}`
}

/** Reset accumulated state. Tests only. */
export function __resetGatewayMetricsForTests(): void {
	fields.clear()
	results.clear()
	unsupported.clear()
	uncompared.clear()
	shapes.clear()
	__resetSearchMetricsForTests()
}
