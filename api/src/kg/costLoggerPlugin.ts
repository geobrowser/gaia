import * as Sentry from "@sentry/node"
import {
	type ArgumentNode,
	type DocumentNode,
	type FieldNode,
	type FragmentDefinitionNode,
	Kind,
	type OperationDefinitionNode,
	print,
	type SelectionNode,
	type SelectionSetNode,
	type ValueNode,
} from "graphql"
import type {Plugin} from "graphql-yoga"
import {graphqlQueryFingerprint} from "../services/queryFingerprint"
import {log} from "../services/telemetry"
import {MAX_PAGINATION_LIMIT} from "./paginationCapPlugin"

/**
 * Context-key used to share the per-request compressed cost score between
 * `useCostLogger` (where it's computed) and `useGraphQLInstrumentation`
 * (where it's attached to slow-query / large-response warnings so a 30s+
 * report carries cost as context). Stashing on the yoga `contextValue`
 * avoids recomputing in two plugins.
 */
export const GRAPHQL_QUERY_COST_CONTEXT_KEY = "graphqlQueryCost" as const

export type GraphqlCostContext = {
	[GRAPHQL_QUERY_COST_CONTEXT_KEY]?: number
}

// Parse a positive-integer env var, falling back to `fallback` on anything
// non-finite or non-positive. Guards against deploy-time misconfiguration
// (e.g. `GRAPHQL_COST_LOG_THRESHOLD=abc` → parseInt = NaN) silently disabling
// the threshold.
function parsePositiveIntEnv(name: string, fallback: number): number {
	const raw = process.env[name]
	if (raw === undefined) return fallback
	const parsed = Number.parseInt(raw, 10)
	return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback
}

// ---------------------------------------------------------------------------
// Compressed (log₁₀-scaled) cost
// ---------------------------------------------------------------------------
// The raw cost is a multiplicative estimate (`child × limit + 1` at every
// nested level) and can produce 20+-digit BigInts for pathological queries —
// well past Number.MAX_SAFE_INTEGER. Rather than cap the value and collapse
// the tail into a single "too big" bin, we walk in BigInt and surface a
// compressed score: `round(log₁₀(rawCost) × 10)`.
//
// Properties:
//   - 10-point step = 1 order of magnitude of raw cost. A score of 160 is
//     10× more expensive than 150.
//   - Fits Number. A 300-digit raw cost compresses to 3000 — still tiny.
//   - Cheap to read. Triviality vs pathology is a two-digit diff, not an
//     "is that 10¹⁷ or 10¹⁸ in scientific notation" squint.
//
// Approximate compressed landmarks, measured against real prod shapes:
//   0           trivial scalar (`{ __typename }`)
//   ~35         simple entity lookup (`entity(id) { id name }`)
//   ~50         medium 1-level nested list
//   ~160        non-nested heavy query (N=3 aliased relations, no nesting)
//   ~250–270    cap-hitting prod shapes (N=3–10 aliased relations with
//               nested `relations` inside `toEntity`) — these are the
//               structurally-unbounded queries worth flagging.
//   ~1500       50-level deeply nested adversarial probe.
const COST_LOG_THRESHOLD = parsePositiveIntEnv("GRAPHQL_COST_LOG_THRESHOLD", 200)

// ---------------------------------------------------------------------------
// Prometheus histogram metric — gaia_api_graphql_query_cost
// ---------------------------------------------------------------------------
// Exposed via /health/metrics. In-process monotonic counters (reset on pod
// restart) are converted by Prometheus over the scrape stream into rates +
// quantiles for the Grafana dashboard.
//
// Bucket edges now use the compressed (log₁₀×10) scale. Each 10-point step
// is a 10× jump in raw cost, so one linear scale captures everything from
// trivial scalars (score 0) to adversarial fractal nesting (score 1000+).
const COST_BUCKET_EDGES: readonly number[] = [
	30, 60, 90, 120, 150, 180, 200, 220, 240, 260, 280, 300, 320, 350, 400, 500, 1000,
]

const bucketCounts = new Map<number, number>()
let totalCount = 0
let totalSum = 0

function recordQueryCost(cost: number): void {
	// Defensive: `computeQueryCost` shouldn't produce NaN / Infinity /
	// negative, but a future caller mustn't be able to poison the metric.
	if (!Number.isFinite(cost) || cost < 0) return
	totalCount++
	totalSum += cost
	for (const edge of COST_BUCKET_EDGES) {
		if (cost <= edge) bucketCounts.set(edge, (bucketCounts.get(edge) ?? 0) + 1)
	}
}

/**
 * Render the cost histogram in Prometheus text format. Emits the `le`
 * buckets, `+Inf` total, `_sum`, and `_count` lines.
 */
export function renderQueryCostHistogram(): string {
	const lines = [
		"# HELP gaia_api_graphql_query_cost GraphQL query complexity score (log10×10 compressed, see costLoggerPlugin.ts).",
		"# TYPE gaia_api_graphql_query_cost histogram",
	]
	for (const edge of COST_BUCKET_EDGES) {
		lines.push(`gaia_api_graphql_query_cost_bucket{le="${edge}"} ${bucketCounts.get(edge) ?? 0}`)
	}
	lines.push(`gaia_api_graphql_query_cost_bucket{le="+Inf"} ${totalCount}`)
	lines.push(`gaia_api_graphql_query_cost_sum ${totalSum}`)
	lines.push(`gaia_api_graphql_query_cost_count ${totalCount}`)
	return `${lines.join("\n")}\n`
}

/**
 * Reset accumulated histogram state. Tests only — not used in runtime.
 */
export function __resetQueryCostHistogramForTests(): void {
	bucketCounts.clear()
	totalCount = 0
	totalSum = 0
}

// ---------------------------------------------------------------------------
// BigInt cost walker
// ---------------------------------------------------------------------------
// Walks the operation AST in BigInt so the multiplicative cost chain can grow
// past Number.MAX_SAFE_INTEGER without losing precision or needing a cap.
// Intentionally schema-free: works regardless of which `graphql` module
// instance built the schema (PostGraphile v4 bundles its own copy).

/**
 * Raw (uncompressed, BigInt) query cost — exported for tests. Production
 * call sites should use `computeQueryCost` instead and work with the
 * compressed Number score.
 */
export function computeQueryCostRaw(
	doc: DocumentNode,
	variables: Record<string, unknown> = {},
	operationName?: string | null,
): bigint {
	const op = getOperationDefinition(doc, operationName)
	if (!op) return 0n
	return sumSelections(op.selectionSet, variables, doc, new Set(), 0)
}

/**
 * Compute a compressed query complexity score: `round(log₁₀(rawCost) × 10)`.
 * See the constant block above for interpretive landmarks.
 *
 * Returns 0 for trivial queries; typical heavy prod queries land 150–250;
 * structurally-unbounded shapes exceed 240.
 */
export function computeQueryCost(
	doc: DocumentNode,
	variables: Record<string, unknown> = {},
	operationName?: string | null,
): number {
	return compressCost(computeQueryCostRaw(doc, variables, operationName))
}

/** `round(log₁₀(n) × 10)` with BigInt-safe log10. Returns 0 for n ≤ 1. */
export function compressCost(n: bigint): number {
	if (n <= 1n) return 0
	// Approximate log₁₀ via string length + leading digits. Stable to ~15
	// significant digits regardless of how big n is.
	const s = n.toString()
	const leadDigits = Math.min(15, s.length)
	const lead = Number(s.slice(0, leadDigits)) // fits in Number, leadDigits ≤ 15
	const log10 = Math.log10(lead) + (s.length - leadDigits)
	return Math.round(log10 * 10)
}

function getOperationDefinition(doc: DocumentNode, operationName?: string | null): OperationDefinitionNode | undefined {
	if (!isDocumentLike(doc)) throw new TypeError("Invalid GraphQL document")
	const operations = doc.definitions.filter((d): d is OperationDefinitionNode => d.kind === Kind.OPERATION_DEFINITION)
	if (operationName) {
		return operations.find((op) => op.name?.value === operationName) ?? operations[0]
	}
	return operations[0]
}

function isDocumentLike(doc: unknown): doc is DocumentNode {
	return !!doc && typeof doc === "object" && Array.isArray((doc as {definitions?: unknown}).definitions)
}

function isIntrospectionOnlyOperation(doc: DocumentNode, operationName?: string | null): boolean {
	if (!isDocumentLike(doc)) return false
	const op = getOperationDefinition(doc, operationName)
	if (!op) return false

	const rootFields = collectRootFieldNames(op.selectionSet, doc, new Set())
	return (
		rootFields.length > 0 &&
		rootFields.every((name) => name.startsWith("__")) &&
		rootFields.some((name) => name === "__schema" || name === "__type")
	)
}

function collectRootFieldNames(
	selectionSet: SelectionSetNode,
	doc: DocumentNode,
	visitedFragments: Set<string>,
): string[] {
	const names: string[] = []
	for (const sel of selectionSet.selections) {
		if (sel.kind === Kind.FIELD) {
			names.push(sel.name.value)
		} else if (sel.kind === Kind.INLINE_FRAGMENT) {
			names.push(...collectRootFieldNames(sel.selectionSet, doc, visitedFragments))
		} else if (sel.kind === Kind.FRAGMENT_SPREAD) {
			const name = sel.name.value
			if (visitedFragments.has(name)) continue
			const frag = doc.definitions.find(
				(d): d is FragmentDefinitionNode => d.kind === Kind.FRAGMENT_DEFINITION && d.name.value === name,
			)
			if (frag) {
				visitedFragments.add(name)
				names.push(...collectRootFieldNames(frag.selectionSet, doc, visitedFragments))
				visitedFragments.delete(name)
			}
		}
	}
	return names
}

function shouldIncludeSelection(selection: SelectionNode, vars: Record<string, unknown>): boolean {
	for (const directive of selection.directives ?? []) {
		if (directive.name.value !== "skip" && directive.name.value !== "include") continue
		const ifArg = directive.arguments?.find((arg) => arg.name.value === "if")
		if (!ifArg) continue
		const value = resolveValue(ifArg.value, vars)
		if (typeof value !== "boolean") continue
		if (directive.name.value === "skip" && value) return false
		if (directive.name.value === "include" && !value) return false
	}
	return true
}

function sumSelections(
	selectionSet: SelectionSetNode,
	vars: Record<string, unknown>,
	doc: DocumentNode,
	visitedFragments: Set<string>,
	depth: number,
): bigint {
	let total = 0n
	for (const sel of selectionSet.selections) {
		if (!shouldIncludeSelection(sel, vars)) continue
		if (sel.kind === Kind.FIELD) {
			total += fieldCost(sel, vars, doc, visitedFragments, depth)
		} else if (sel.kind === Kind.INLINE_FRAGMENT) {
			total += sumSelections(sel.selectionSet, vars, doc, visitedFragments, depth)
		} else if (sel.kind === Kind.FRAGMENT_SPREAD) {
			// Fragment cycles (A spreads B spreads A) are normally blocked by
			// the NoFragmentCycles validation rule, but if something in the
			// pipeline ever skipped validation we'd recurse forever. Track
			// visited names on the way down, pop on the way back up so
			// siblings can reuse the same fragment legitimately.
			const name = sel.name.value
			if (visitedFragments.has(name)) continue
			const frag = doc.definitions.find(
				(d): d is FragmentDefinitionNode => d.kind === Kind.FRAGMENT_DEFINITION && d.name.value === name,
			)
			if (frag) {
				visitedFragments.add(name)
				total += sumSelections(frag.selectionSet, vars, doc, visitedFragments, depth)
				visitedFragments.delete(name)
			}
		}
	}
	return total
}

/**
 * Cost model per field:
 *   - Explicit `first` / `last` arg with a valid positive value → `child × limit + 1`.
 *     `offset` contributes to the effective limit because the DB may still
 *     need to skip those rows before returning the page.
 *   - Pagination arg present but value unresolved / non-positive → fall back
 *     to MAX_PAGINATION_LIMIT (1000) — matches the effective cap at SQL build.
 *   - No pagination arg, has selections → `child × MAX_PAGINATION_LIMIT + 1`.
 *     PaginationCapPlugin injects `first: 1000` on every unpaginated
 *     collection field at SQL build, so modeling the default at 1000 matches
 *     the real fan-out ceiling. BigInt arithmetic means this doesn't overflow.
 *   - No pagination arg, no selections → 1 (scalar leaf).
 *   - Known SQL-heavy fields / args multiply the raw cost by conservative
 *     factors. On the compressed score, ×10 is roughly +10 points.
 */
const DEFAULT_LIMIT = BigInt(MAX_PAGINATION_LIMIT)
const TOTAL_COUNT_RAW_COST = 10_000n
const MAX_FILTER_FACTOR = 1_000n
const LARGE_ROOT_PAGE_LIMIT = 100n
const VERY_LARGE_ROOT_PAGE_LIMIT = 500n

const SEARCH_FIELD_NAMES: ReadonlySet<string> = new Set(["search", "searchConnection"])
const ENTITIES_ORDERED_BY_PROPERTY_FIELD_NAMES: ReadonlySet<string> = new Set([
	"entitiesOrderedByProperty",
	"entitiesOrderedByPropertyConnection",
])
const SCORE_ORDERED_FIELD_NAMES: ReadonlySet<string> = new Set(["values", "valuesConnection"])
const ENTITY_ROOT_COLLECTION_FIELD_NAMES: ReadonlySet<string> = new Set(["entities", "entitiesConnection"])
const BROAD_ROOT_COLLECTION_FIELD_NAMES: ReadonlySet<string> = new Set([
	"entities",
	"entitiesConnection",
	"values",
	"valuesConnection",
	"relations",
	"relationsConnection",
	"spaces",
	"spacesConnection",
])
const SELECTIVE_ROOT_ARGS: ReadonlySet<string> = new Set([
	"id",
	"nodeId",
	"spaceId",
	"spaceIds",
	"typeId",
	"typeIds",
	"condition",
	"filter",
])

const STRING_SCAN_FILTER_FACTORS: Readonly<Record<string, bigint>> = {
	includes: 20n,
	includesInsensitive: 30n,
	like: 20n,
	likeInsensitive: 30n,
	notIncludes: 30n,
	notIncludesInsensitive: 40n,
	notLike: 30n,
	notLikeInsensitive: 40n,
}

const STRUCTURAL_FILTER_FACTORS: Readonly<Record<string, bigint>> = {
	overlaps: 15n,
	contains: 8n,
	containedBy: 8n,
	containsKey: 5n,
	containsAnyKeys: 8n,
	containsAllKeys: 8n,
	isNot: 5n,
	notIn: 5n,
	not: 5n,
}

const RELATION_FILTER_KEYS: ReadonlySet<string> = new Set(["some", "every", "none"])
const COMPUTED_FILTER_FIELD_NAMES: ReadonlySet<string> = new Set([
	"name",
	"description",
	"spaceIds",
	"typeIds",
	"types",
	"spaces",
	"dataType",
	"properties",
	"property",
	"type",
])

const LARGE_PAYLOAD_SCALAR_COSTS: Readonly<Record<string, bigint>> = {
	name: 2n,
	description: 50n,
	text: 25n,
	string: 25n,
	value: 25n,
	schedule: 100n,
	embedding: 100n,
	bytes: 100n,
	metadata: 50n,
	data: 100n,
}

const PAYLOAD_COLLECTION_FACTORS: Readonly<Record<string, bigint>> = {
	values: 5n,
	valuesList: 5n,
	valuesConnection: 5n,
	relations: 4n,
	relationsList: 4n,
	relationsConnection: 4n,
	backlinks: 4n,
	backlinksList: 4n,
	backlinksConnection: 4n,
	relationsWhereEntity: 4n,
	relationsWhereEntityList: 4n,
	relationsWhereEntityConnection: 4n,
}

const ENTITY_REFERENCE_FIELD_NAMES: ReadonlySet<string> = new Set([
	"entity",
	"fromEntity",
	"toEntity",
	"relationEntity",
	"typeEntity",
	"propertyEntity",
])

function fieldCost(
	field: FieldNode,
	vars: Record<string, unknown>,
	doc: DocumentNode,
	visitedFragments: Set<string>,
	depth: number,
): bigint {
	// Check for the *syntactic* presence of first/last. `entities(first: $n)`
	// with `$n` unresolved still means the caller intended pagination — treat
	// it as a list (and fall back to MAX_PAGINATION_LIMIT since the effective
	// limit is undefined).
	const fieldName = field.name.value
	const argNames = new Set((field.arguments ?? []).map((a) => a.name.value))
	const hasPagination = argNames.has("first") || argNames.has("last")
	const args = resolveArgs(field.arguments, vars)

	const child = field.selectionSet ? sumSelections(field.selectionSet, vars, doc, visitedFragments, depth + 1) : 0n

	if (!field.selectionSet) {
		return scalarFieldCost(fieldName) * sqlRiskFactor(fieldName, args, argNames, depth, 0n)
	}

	const limit = effectiveLimit(args, hasPagination, argNames.has("offset"))
	const base = child * limit + 1n
	return base * sqlRiskFactor(fieldName, args, argNames, depth, limit)
}

function scalarFieldCost(fieldName: string): bigint {
	if (fieldName === "totalCount") return TOTAL_COUNT_RAW_COST
	return LARGE_PAYLOAD_SCALAR_COSTS[fieldName] ?? 1n
}

function effectiveLimit(args: Record<string, unknown>, hasPagination: boolean, hasOffset: boolean): bigint {
	const rawLimit = args.first ?? args.last
	const validLimit = typeof rawLimit === "number" && Number.isFinite(rawLimit) && rawLimit > 0
	const limit = hasPagination ? (validLimit ? BigInt(rawLimit) : DEFAULT_LIMIT) : DEFAULT_LIMIT
	if (!hasOffset) return limit

	const rawOffset = args.offset
	const validOffset = typeof rawOffset === "number" && Number.isFinite(rawOffset) && rawOffset > 0
	return limit + (validOffset ? BigInt(rawOffset) : DEFAULT_LIMIT)
}

function sqlRiskFactor(
	fieldName: string,
	args: Record<string, unknown>,
	argNames: ReadonlySet<string>,
	depth: number,
	limit: bigint,
): bigint {
	let factor = 1n

	if (SEARCH_FIELD_NAMES.has(fieldName)) factor *= 20n
	if (ENTITIES_ORDERED_BY_PROPERTY_FIELD_NAMES.has(fieldName)) factor *= 50n
	if (SCORE_ORDERED_FIELD_NAMES.has(fieldName) && usesScoreOrderBy(args.orderBy)) factor *= 20n
	if (isBroadRootCollection(fieldName, argNames, depth, limit)) factor *= 3n
	if (ENTITY_ROOT_COLLECTION_FIELD_NAMES.has(fieldName) && depth === 0) factor *= rootEntityPagePayloadFactor(limit)
	factor *= PAYLOAD_COLLECTION_FACTORS[fieldName] ?? 1n
	if (ENTITY_REFERENCE_FIELD_NAMES.has(fieldName) && depth >= 2) factor *= 3n

	if (argNames.has("filter")) factor *= 5n * filterRiskFactor(args.filter)
	if (argNames.has("condition")) factor *= 2n
	if (argNames.has("orderBy")) factor *= orderByRiskFactor(args.orderBy)
	if (argNames.has("after") || argNames.has("before")) factor *= 2n

	return factor
}

function rootEntityPagePayloadFactor(limit: bigint): bigint {
	if (limit >= VERY_LARGE_ROOT_PAGE_LIMIT) return 10n
	if (limit >= LARGE_ROOT_PAGE_LIMIT) return 3n
	return 1n
}

function isBroadRootCollection(
	fieldName: string,
	argNames: ReadonlySet<string>,
	depth: number,
	limit: bigint,
): boolean {
	if (depth !== 0 || !BROAD_ROOT_COLLECTION_FIELD_NAMES.has(fieldName)) return false
	if (limit < DEFAULT_LIMIT / 2n) return false
	for (const argName of SELECTIVE_ROOT_ARGS) {
		if (argNames.has(argName)) return false
	}
	return true
}

function orderByRiskFactor(value: unknown): bigint {
	if (usesScoreOrderBy(value)) return 20n
	return value === undefined || value === null ? 1n : 3n
}

function usesScoreOrderBy(value: unknown): boolean {
	if (typeof value === "string") return value.includes("SCORE")
	if (Array.isArray(value)) return value.some(usesScoreOrderBy)
	return false
}

function filterRiskFactor(value: unknown): bigint {
	return capFilterFactor(filterRiskFactorInner(value))
}

function filterRiskFactorInner(value: unknown): bigint {
	if (Array.isArray(value)) {
		if (value.length === 0) return 1n
		return value.reduce((factor, item) => capFilterFactor(factor * filterRiskFactorInner(item)), 1n)
	}
	if (!value || typeof value !== "object") return 1n

	let factor = 1n
	for (const [key, childValue] of Object.entries(value as Record<string, unknown>)) {
		const stringScanFactor = STRING_SCAN_FILTER_FACTORS[key]
		if (stringScanFactor !== undefined) factor = capFilterFactor(factor * stringScanFactor)

		const structuralFactor = STRUCTURAL_FILTER_FACTORS[key]
		if (structuralFactor !== undefined) factor = capFilterFactor(factor * structuralFactor)

		if (key === "or" && Array.isArray(childValue) && childValue.length > 1) {
			factor = capFilterFactor(factor * BigInt(Math.min(childValue.length, 10)))
		}
		if ((key === "in" || key === "notIn") && Array.isArray(childValue) && childValue.length > 10) {
			factor = capFilterFactor(factor * BigInt(Math.min(Math.ceil(childValue.length / 10), 10)))
		}
		if (RELATION_FILTER_KEYS.has(key)) factor = capFilterFactor(factor * 10n)
		if (COMPUTED_FILTER_FIELD_NAMES.has(key)) factor = capFilterFactor(factor * 10n)

		factor = capFilterFactor(factor * filterRiskFactorInner(childValue))
	}
	return factor
}

function capFilterFactor(value: bigint): bigint {
	return value > MAX_FILTER_FACTOR ? MAX_FILTER_FACTOR : value
}

function resolveArgs(
	args: readonly ArgumentNode[] | undefined,
	vars: Record<string, unknown>,
): Record<string, unknown> {
	const out: Record<string, unknown> = {}
	for (const a of args ?? []) {
		out[a.name.value] = resolveValue(a.value, vars)
	}
	return out
}

function resolveValue(value: ValueNode, vars: Record<string, unknown>): unknown {
	switch (value.kind) {
		case Kind.INT:
			return Number.parseInt(value.value, 10)
		case Kind.FLOAT:
			return Number.parseFloat(value.value)
		case Kind.STRING:
		case Kind.ENUM:
			return value.value
		case Kind.BOOLEAN:
			return value.value
		case Kind.NULL:
			return null
		case Kind.VARIABLE:
			return vars[value.name.value]
		case Kind.LIST:
			return value.values.map((v) => resolveValue(v, vars))
		case Kind.OBJECT:
			return Object.fromEntries(value.fields.map((f) => [f.name.value, resolveValue(f.value, vars)]))
		default:
			return undefined
	}
}

/**
 * Yoga plugin that computes query cost on every request. Three observability
 * channels:
 *   - Prometheus histogram `gaia_api_graphql_query_cost` — records every
 *     query's compressed score; charted in Grafana for distribution.
 *   - Sentry metric `graphql.query_cost` (distribution) — same value, tagged
 *     by operation, lets Sentry surface cost percentiles next to its own
 *     duration / error metrics. Sentry only — no issue is created here.
 *   - `log.warn("High GraphQL query cost", ...)` when score ≥
 *     `COST_LOG_THRESHOLD`. `log.warn` always writes to stdout (visible in
 *     kubectl + Axiom) *and* drops a Sentry breadcrumb, but does NOT create
 *     a Sentry issue.
 *
 * The cost is also stashed on the yoga `contextValue` under
 * `GRAPHQL_QUERY_COST_CONTEXT_KEY` so `useGraphQLInstrumentation` can attach
 * it to the slow-query / large-response warnings without recomputing.
 *
 * Phase 1 is strictly observational. The outer try/catch is a shadow-mode
 * safety invariant: a failure inside the plugin must never break the
 * request it was observing.
 */
export function useCostLogger(): Plugin {
	return {
		onExecute({args}) {
			try {
				if (isIntrospectionOnlyOperation(args.document, args.operationName)) return

				let cost: number
				try {
					cost = computeQueryCost(args.document, args.variableValues ?? {}, args.operationName)
				} catch (error) {
					log.warn("[cost] complexity calculation failed", {
						error: error instanceof Error ? error.message : String(error),
						operationName: args.operationName ?? undefined,
					})
					return
				}

				recordQueryCost(cost)

				const operationLabel = getOperationLabel(args)

				// Sentry distribution metric — operation-tagged so dashboards can
				// surface p50 / p95 / p99 cost per operation alongside duration.
				// Cost is the compressed log10×10 score (dimensionless), so no unit.
				Sentry.metrics.distribution("graphql.query_cost", cost, {
					attributes: {operation: operationLabel},
				})

				// Stash on the request context so the instrumentation plugin can
				// include cost in slow-query / large-response warnings without
				// having to walk the AST again. `contextValue` is the same object
				// passed through both plugins for a single request.
				const ctx = args.contextValue as GraphqlCostContext | undefined
				if (ctx && typeof ctx === "object") {
					ctx[GRAPHQL_QUERY_COST_CONTEXT_KEY] = cost
				}

				if (cost >= COST_LOG_THRESHOLD) {
					const fullQuery = print(args.document)
					log.warn("High GraphQL query cost", {
						cost,
						threshold: COST_LOG_THRESHOLD,
						operationName: operationLabel,
						queryFingerprint: graphqlQueryFingerprint(fullQuery),
						query: fullQuery.slice(0, 2000),
						variables: args.variableValues,
					})
				}
			} catch (error) {
				// Shadow-mode guarantee: never break the request.
				try {
					log.warn("[cost] plugin onExecute threw", {
						error: error instanceof Error ? error.message : String(error),
						operationName: args.operationName ?? undefined,
					})
				} catch {
					// Nothing we can do; keep the request flowing.
				}
			}
		},
	}
}

function getOperationLabel(args: {operationName?: string | null; document: {definitions: readonly unknown[]}}): string {
	if (args.operationName) return args.operationName
	if (!args.document || !Array.isArray(args.document.definitions)) return "anonymous"

	const operationDef = args.document.definitions.find(
		(def): def is OperationDefinitionNode =>
			typeof def === "object" && def !== null && "kind" in def && def.kind === Kind.OPERATION_DEFINITION,
	)
	if (!operationDef) return "anonymous"

	const firstField = operationDef.selectionSet.selections.find((sel): sel is FieldNode => sel.kind === Kind.FIELD)
	return firstField ? `${operationDef.operation} ${firstField.name.value}` : operationDef.operation
}
