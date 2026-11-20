/**
 * Prometheus metrics for the GraphQL gateway's shadow comparisons, in the text format `/health/metrics` serves.
 *
 * Empty until a shadow is mounted (`activateGatewayMetrics`), so a flag-off deploy exports nothing. Once
 * active, every series for a registered field is created at zero so `increase()` sees the first event.
 */

import {FAIL_REASONS, notedFailReason} from "../geodb/failReason"
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
	/** An unordered `entities` type scan whose window (`offset + first`) is past the 4096 geo-db's one-row cut allows. */
	"type_scan_window",
	/** An unordered `entities` type scan the serve refused (`.args.types` is geo-db c2f9d09cfa or later): the serve has not rolled yet. */
	"type_scan_refused",
	/** An unordered `entities` type scan page that was cut at `first` (or has an offset) and holds a legacy (non-v4) id: Postgres pages by the raw id, geo-db by the conforming one, so which rows the window holds is not comparable. */
	"type_scan_legacy_ids",
	/** `metas`: the indexer cursor rows (id, cursor, block number) live only in Postgres; geo-db holds no read of them. */
	"no_geodb_home",
	/** `metas`: geo-db's `cursors` collection holds no row for an indexer PostgreSQL has (the mirror is off, or has not written yet). */
	"not_mirrored",
	/** `metas`: a `cursors` value that is not `<block>:<cursor>`. */
	"bad_layout",
	/** `entitiesConnection` asking nothing but `totalCount` (alone or as several aliased roots): geo-db holds no count of a filtered set, and no card is returned to judge. */
	"count_only",
	/** A `totalCount`-only `entitiesConnection` lowered to geo-db's count form that the serve refused (`.args.count` with `.where` is geo-db DR-154 item 8 or later): the serve has not rolled yet. */
	"count_refused",
	/** A `totalCount`-only `entitiesConnection` lowered to the count form whose walk did not reach the end within the pages the tool fetched, so the sum is not the count. */
	"count_incomplete",
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
	/** An unordered `entities` type scan page holding a legacy (non-v4) id: the whole page's members matched, but their order was not compared (Postgres orders by the raw id, geo-db by the conforming one). */
	"legacy_id_order",
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
	/** `space` / `spaces`: a Space's `subspaceTopics` list, a Space-to-Space link no entity card holds. */
	"space_subspace_topics",
	/** `entitiesOrderedByProperty`: part of the page's order was not checked: adjacent entities whose sort keys geo-db could not place (text under the database's collation, a value geo-db lacks, several text values). */
	"order_partial",
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
	/** `relation` / `relations`: the to-space and verified flag are selected but could not be compared (the relation's id, or geo-db's claims read, was missing). */
	"relation_facets",
] as const

/**
 * Which part of a shadowed answer differed, for a `mismatch`. A closed set, so it is safe as a label; `other` holds a
 * mismatch no named part explains. One operation counts each part once, so the series can sum to more than `mismatch`.
 */
export const MISMATCH_PARTS = [
	"statements_missing",
	"statements_extra",
	"name",
	"description",
	"types",
	"types_names",
	"space_ids",
	"updated_at",
	"created_at",
	"all_values_list",
	"property_names",
	"type_names",
	"to_entity_names",
	"from_entity_names",
	"nested_entity",
	"property_info",
	"relations_membership",
	"relation_facets",
	"relation_edge",
	"backlink_rows",
	"unmet_filter",
	"not_in_space",
	"filter_breach",
	"ordered_page",
	"type_scan",
	"property_order",
	"absent_in_postgres",
	"other",
] as const

export type ShadowResult = (typeof SHADOW_RESULTS)[number]
export type MismatchPart = (typeof MISMATCH_PARTS)[number]
export type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]
export type UncomparedReason = (typeof UNCOMPARED_REASONS)[number]

const fields = new Set<string>()
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const failed = new Map<string, number>()
const uncompared = new Map<string, number>()
const shapes = new Map<string, number>()
const mismatchParts = new Map<string, number>()
const requests = new Map<string, number>()
// gaia_api_gateway_requests_total: every operation the gateway routed to a field in mode shadow or on, whichever way it was answered.
const fieldRequestFields = new Set<string>()
const fieldRequests = new Map<string, number>()
const postgresLatency = new Map<string, Histogram>()
const geodbLatency = new Map<string, Histogram>()

/** Upper bounds in seconds of a shadowed field's PostgreSQL answer and of its geo-db shadow run. */
export const SHADOW_LATENCY_BUCKETS_SECONDS = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5] as const

type Histogram = {counts: number[]; sum: number}

/** Most distinct (field, reason, shape) series one process exports; later shapes count as `other`. */
export const MAX_UNSUPPORTED_SHAPES = 100
const SHAPE_PATTERN = /^[A-Za-z0-9_.,;:=()[\]|+-]{1,200}$/

const bump = (map: Map<string, number>, key: string) => map.set(key, (map.get(key) ?? 0) + 1)

export function activateGatewayMetrics(field: string): void {
	fields.add(field)
	fieldRequestFields.add(field)
}

/** Registers `field` for gaia_api_gateway_requests_total at zero (a field that is served, or shadowed). */
export function activateGatewayRequests(field: string): void {
	fieldRequestFields.add(field)
}

/**
 * Counts an operation routed to `field` (shadow or on) as it arrives: the denominator of a served share and of a
 * fallback ratio. A series of its own; gaia_api_gateway_shadow_requests_total keeps counting shadowed operations only.
 */
export function recordGatewayFieldRequest(field: string): void {
	fieldRequestFields.add(field)
	bump(fieldRequests, field)
}

function renderFieldRequests(): string {
	if (fieldRequestFields.size === 0) return ""
	const lines = [
		"# HELP gaia_api_gateway_requests_total GraphQL operations routed to a root field in mode shadow or on, counted when they arrive and whichever store answers them, by root field.",
		"# TYPE gaia_api_gateway_requests_total counter",
	]
	for (const field of fieldRequestFields)
		lines.push(`gaia_api_gateway_requests_total{field="${field}"} ${fieldRequests.get(field) ?? 0}`)
	return `${lines.join("\n")}\n`
}

export function recordGatewayShadow(field: string, result: ShadowResult): void {
	bump(results, `${field}\t${result}`)
	// Why a read failed comes from the pool's failure scope (see geodb/failReason.ts); `other` when none was noted.
	if (result === "failed") bump(failed, `${field}\t${notedFailReason()}`)
}

/** Counts each distinct part of one mismatching operation once. An empty list counts `other`. */
export function recordGatewayMismatchParts(field: string, parts: Iterable<MismatchPart>): void {
	const distinct = new Set(parts)
	if (distinct.size === 0) distinct.add("other")
	for (const part of distinct) bump(mismatchParts, `${field}\t${part}`)
}

/** Counts a shadowable operation for `field` as it arrives, before the sample decides whether to compare it. */
export function recordGatewayRequest(field: string): void {
	bump(requests, field)
}

function observe(map: Map<string, Histogram>, field: string, seconds: number): void {
	const histogram = map.get(field) ?? {
		counts: new Array<number>(SHADOW_LATENCY_BUCKETS_SECONDS.length + 1).fill(0),
		sum: 0,
	}
	const index = SHADOW_LATENCY_BUCKETS_SECONDS.findIndex((edge) => seconds <= edge)
	const slot = index === -1 ? SHADOW_LATENCY_BUCKETS_SECONDS.length : index
	histogram.counts[slot] = (histogram.counts[slot] ?? 0) + 1
	histogram.sum += seconds
	map.set(field, histogram)
}

/** Time PostgreSQL (PostGraphile) took to answer one shadowable operation, sampled or not. */
export function observeGatewayPostgres(field: string, seconds: number): void {
	observe(postgresLatency, field, seconds)
}

/** Time one sampled shadow run took: the geo-db read plus the comparison. Observed even when the shadow throws. */
export function observeGatewayGeoDb(field: string, seconds: number): void {
	observe(geodbLatency, field, seconds)
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
	if (fields.size === 0) return `${renderFieldRequests()}${renderSearchMetrics()}`
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
		"# HELP gaia_api_gateway_shadow_mismatch_total Gateway shadow comparisons counted mismatch, by root field and the part that differed. An operation counts each differing part once, so the parts can sum to more than the mismatch result.",
		"# TYPE gaia_api_gateway_shadow_mismatch_total counter",
	)
	for (const field of fields)
		for (const part of MISMATCH_PARTS)
			lines.push(
				`gaia_api_gateway_shadow_mismatch_total{field="${field}",part="${part}"} ${mismatchParts.get(`${field}\t${part}`) ?? 0}`,
			)
	lines.push(
		"# HELP gaia_api_gateway_shadow_requests_total Operations whose root field has a shadow mounted, counted before the sample rate decides whether one is compared, by root field.",
		"# TYPE gaia_api_gateway_shadow_requests_total counter",
	)
	for (const field of fields)
		lines.push(`gaia_api_gateway_shadow_requests_total{field="${field}"} ${requests.get(field) ?? 0}`)
	for (const [name, help, map] of [
		[
			"gaia_api_gateway_shadow_postgres_seconds",
			"Time PostgreSQL took to answer an operation whose root field has a shadow mounted, by root field. Counted for every such operation, before the sample rate decides whether one is compared.",
			postgresLatency,
		],
		[
			"gaia_api_gateway_shadow_geodb_seconds",
			"Time a sampled shadow run took (the geo-db read plus the comparison, a failed or timed-out read included), by root field. Compare with the PostgreSQL series of the same field for the p99 within 2x test.",
			geodbLatency,
		],
	] as const) {
		lines.push(`# HELP ${name} ${help}`, `# TYPE ${name} histogram`)
		for (const field of fields) {
			const histogram = map.get(field) ?? {
				counts: new Array<number>(SHADOW_LATENCY_BUCKETS_SECONDS.length + 1).fill(0),
				sum: 0,
			}
			let cumulative = 0
			SHADOW_LATENCY_BUCKETS_SECONDS.forEach((edge, i) => {
				cumulative += histogram.counts[i] ?? 0
				lines.push(`${name}_bucket{field="${field}",le="${edge}"} ${cumulative}`)
			})
			cumulative += histogram.counts[SHADOW_LATENCY_BUCKETS_SECONDS.length] ?? 0
			lines.push(`${name}_bucket{field="${field}",le="+Inf"} ${cumulative}`)
			lines.push(`${name}_sum{field="${field}"} ${histogram.sum}`)
			lines.push(`${name}_count{field="${field}"} ${cumulative}`)
		}
	}
	lines.push(
		"# HELP gaia_api_gateway_shadow_failed_total Gateway shadow comparisons whose geo-db read failed (the failed result above), by root field and reason: timeout, client_failed, cooling_down or other.",
		"# TYPE gaia_api_gateway_shadow_failed_total counter",
	)
	for (const field of fields)
		for (const reason of FAIL_REASONS)
			lines.push(
				`gaia_api_gateway_shadow_failed_total{field="${field}",reason="${reason}"} ${failed.get(`${field}\t${reason}`) ?? 0}`,
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
	return `${lines.join("\n")}\n${renderFieldRequests()}${renderSearchMetrics()}`
}

/** Reset accumulated state. Tests only. */
export function __resetGatewayMetricsForTests(): void {
	fields.clear()
	results.clear()
	unsupported.clear()
	uncompared.clear()
	failed.clear()
	shapes.clear()
	mismatchParts.clear()
	requests.clear()
	fieldRequestFields.clear()
	fieldRequests.clear()
	postgresLatency.clear()
	geodbLatency.clear()
	__resetSearchMetricsForTests()
}
