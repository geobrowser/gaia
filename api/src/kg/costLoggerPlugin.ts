import {
	type ArgumentNode,
	type DocumentNode,
	type FieldNode,
	type FragmentDefinitionNode,
	Kind,
	type OperationDefinitionNode,
	print,
	type SelectionSetNode,
	type ValueNode,
} from "graphql"
import type {Plugin} from "graphql-yoga"
import {graphqlQueryFingerprint} from "../services/queryFingerprint"
import {log} from "../services/telemetry"
import {MAX_PAGINATION_LIMIT} from "./paginationCapPlugin"

// Parse a positive-integer env var, falling back to `fallback` on anything
// non-finite or non-positive. Guards against deploy-time misconfiguration
// (e.g. `GRAPHQL_COST_CALC_LIMIT=abc` → parseInt = NaN) silently disabling
// the walker's safety cap.
function parsePositiveIntEnv(name: string, fallback: number): number {
	const raw = process.env[name]
	if (raw === undefined) return fallback
	const parsed = Number.parseInt(raw, 10)
	return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback
}

// Threshold above which we emit an info-level log line with the full query +
// variables. Not an alert — just "this one is worth finding in logs if we
// need to investigate a slow response later". Kept separate from the
// histogram metric (which records every query regardless of size).
const COST_LOG_THRESHOLD = parsePositiveIntEnv("GRAPHQL_COST_LOG_THRESHOLD", 1_000_000)

// Hard ceiling on the cost calculation itself. Once the walker's running
// total reaches this value it short-circuits and returns the cap instead of
// continuing to multiply. Caps memory (totalSum can't drift to Infinity),
// caps CPU (adversarial or pathological queries don't walk forever), and
// keeps the metric values in a sane range for Prometheus / Grafana.
const MAX_COST_CALC_LIMIT = parsePositiveIntEnv("GRAPHQL_COST_CALC_LIMIT", 1_000_000_000)

// ---------------------------------------------------------------------------
// Prometheus histogram metric — gaia_api_graphql_query_cost
// ---------------------------------------------------------------------------
// Exposed via /health/metrics (see renderQueryCostHistogram). We accumulate
// in-process monotonic counters (process start = 0), and Prometheus uses
// rate()/histogram_quantile() over the scrape stream to derive time-windowed
// distributions for the Grafana dashboard.

// Upper bucket edges, spanning the realistic range of the conservative model
// (trivial scalar ≈ 1 at the low end, nested no-pagination queries near the
// 1B cap at the high end). `+Inf` is emitted via the total count at render.
const COST_BUCKET_EDGES: readonly number[] = [
	10, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000, 1_000_000_000,
]

// noUncheckedIndexedAccess widens `number[]`'s index access to `number | undefined`,
// so we go via a Map which has a crisper `get(k) | undefined` story and always
// normalize via `?? 0`. A bit more allocation than a typed array but the map
// is bounded to 9 entries and only written on the hot path; effect is nil.
const bucketCounts = new Map<number, number>()
let totalCount = 0
let totalSum = 0

function recordQueryCost(cost: number): void {
	// Defensive: `computeQueryCost` already caps at MAX_COST_CALC_LIMIT and
	// can't produce NaN / Infinity / negative, but a future caller mustn't be
	// able to poison the metric. Non-finite / negative inputs are dropped.
	if (!Number.isFinite(cost) || cost < 0) return
	const clamped = Math.min(cost, MAX_COST_CALC_LIMIT)
	totalCount++
	totalSum += clamped
	for (const edge of COST_BUCKET_EDGES) {
		if (clamped <= edge) bucketCounts.set(edge, (bucketCounts.get(edge) ?? 0) + 1)
	}
}

/**
 * Render the cost histogram in Prometheus text format. Emits the `le`
 * buckets, `+Inf` total, `_sum`, and `_count` lines.
 */
export function renderQueryCostHistogram(): string {
	const lines = [
		"# HELP gaia_api_graphql_query_cost GraphQL query complexity score distribution (conservative model, see costLoggerPlugin.ts).",
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

/**
 * Compute a query complexity score from the operation AST alone.
 *
 * Cost model, per field:
 *   - Explicit `first` / `last` arg → `child × limit + 1`. Limit clamps to
 *     MAX_PAGINATION_LIMIT when non-positive / non-finite / missing (from an
 *     unresolved variable), mirroring what PaginationCapPlugin actually
 *     applies at SQL-build time and closing the bogus-arg attack vector.
 *   - No pagination arg, has selections → `child × MAX_PAGINATION_LIMIT + 1`.
 *     Conservative: we treat any structured field without an explicit limit
 *     as if it were a list taking the 1000 default. Over-counts single-entity
 *     lookups, but we prefer over-counting to under-counting — PaginationCap
 *     injects the 1000 default on every collection field at SQL time, so the
 *     real work done under those queries scales that way whether or not we
 *     can see it in the AST.
 *   - No pagination arg, no selections → 1 (scalar leaf).
 *
 * Every recursive step checks a hard `MAX_COST_CALC_LIMIT` budget and short-
 * circuits once crossed — prevents Number overflow, unbounded walking on
 * adversarial inputs, and Infinity / NaN leaking into the Prometheus metric.
 *
 * Intentionally schema-free: walking only the document + variables means the
 * estimator works regardless of which `graphql` module instance built the
 * schema (PostGraphile v4 bundles its own copy). Schema-aware libraries like
 * graphql-query-complexity hit instanceof failures across the CJS/ESM module
 * boundary in that setup.
 */
export function computeQueryCost(doc: DocumentNode, variables: Record<string, unknown> = {}): number {
	const op = doc.definitions.find((d): d is OperationDefinitionNode => d.kind === Kind.OPERATION_DEFINITION)
	if (!op) return 0
	return sumSelections(op.selectionSet, variables, doc, new Set())
}

function sumSelections(
	selectionSet: SelectionSetNode,
	vars: Record<string, unknown>,
	doc: DocumentNode,
	visitedFragments: Set<string>,
): number {
	let total = 0
	for (const sel of selectionSet.selections) {
		if (sel.kind === Kind.FIELD) {
			total += fieldCost(sel, vars, doc, visitedFragments)
		} else if (sel.kind === Kind.INLINE_FRAGMENT) {
			total += sumSelections(sel.selectionSet, vars, doc, visitedFragments)
		} else if (sel.kind === Kind.FRAGMENT_SPREAD) {
			// Fragment cycles (A spreads B spreads A) are normally blocked by
			// the NoFragmentCycles validation rule, but if something in the
			// pipeline ever skipped validation we'd recurse forever. Cheap
			// insurance: track visited names on the way down, pop on the way
			// back up so siblings can reuse the same fragment legitimately.
			const name = sel.name.value
			if (visitedFragments.has(name)) continue
			const frag = doc.definitions.find(
				(d): d is FragmentDefinitionNode => d.kind === Kind.FRAGMENT_DEFINITION && d.name.value === name,
			)
			if (frag) {
				visitedFragments.add(name)
				total += sumSelections(frag.selectionSet, vars, doc, visitedFragments)
				visitedFragments.delete(name)
			}
		}
		if (total >= MAX_COST_CALC_LIMIT) return MAX_COST_CALC_LIMIT
	}
	return total
}

function fieldCost(
	field: FieldNode,
	vars: Record<string, unknown>,
	doc: DocumentNode,
	visitedFragments: Set<string>,
): number {
	// Check for the *syntactic* presence of first/last in the query, not the
	// resolved value. `entities(first: $first)` with $first unresolved means
	// the user intends pagination — we should still treat it as a list (and
	// fall back to MAX because the effective limit is undefined).
	const argNames = new Set((field.arguments ?? []).map((a) => a.name.value))
	const hasPagination = argNames.has("first") || argNames.has("last")

	const child = field.selectionSet ? sumSelections(field.selectionSet, vars, doc, visitedFragments) : 0

	// Propagate the cap so we never try to multiply a near-cap value and
	// overflow. Math.min below also clamps the final result.
	if (child >= MAX_COST_CALC_LIMIT) return MAX_COST_CALC_LIMIT

	if (hasPagination) {
		const args = resolveArgs(field.arguments, vars)
		const raw = args.first ?? args.last
		const validExplicit = typeof raw === "number" && Number.isFinite(raw) && raw > 0
		const limit = validExplicit ? (raw as number) : MAX_PAGINATION_LIMIT
		return Math.min(child * limit + 1, MAX_COST_CALC_LIMIT)
	}

	// Structured field without an explicit limit: conservatively assume the
	// SQL-injected default of MAX_PAGINATION_LIMIT applies. Leaf scalars
	// (no selection set → child=0) collapse to `0 × N + 1 = 1` here, so we
	// don't need a separate branch.
	return Math.min(child * MAX_PAGINATION_LIMIT + 1, MAX_COST_CALC_LIMIT)
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
 * Yoga plugin that computes query cost on every request. Two observability
 * channels, no alerts:
 *   - Prometheus histogram `gaia_api_graphql_query_cost` — records every
 *     query's cost; charted in Grafana for distribution / outliers.
 *   - `log.warn("High GraphQL query cost", ...)` when cost exceeds
 *     `COST_LOG_THRESHOLD`. `log.warn` in this codebase always writes to
 *     stdout (visible in kubectl + Axiom) *and* drops a Sentry breadcrumb,
 *     but does NOT create a Sentry issue — that's the captureMessage path
 *     we deliberately don't use. `log.info`, by contrast, only writes a
 *     breadcrumb in production (invisible to log search), which defeats the
 *     point of the threshold log.
 *
 * Phase 1 is strictly observational. The outer try/catch is a shadow-mode
 * safety invariant: a failure inside the plugin must never break the
 * request it was observing.
 */
export function useCostLogger(): Plugin {
	return {
		onExecute({args}) {
			try {
				if (args.operationName === "IntrospectionQuery") return

				let cost: number
				try {
					cost = computeQueryCost(args.document, args.variableValues ?? {})
				} catch (error) {
					log.warn("[cost] complexity calculation failed", {
						error: error instanceof Error ? error.message : String(error),
						operationName: args.operationName ?? undefined,
					})
					return
				}

				recordQueryCost(cost)

				if (cost >= COST_LOG_THRESHOLD) {
					const fullQuery = print(args.document)
					log.warn("High GraphQL query cost", {
						cost,
						threshold: COST_LOG_THRESHOLD,
						operationName: getOperationLabel(args),
						queryFingerprint: graphqlQueryFingerprint(fullQuery),
						query: fullQuery.slice(0, 2000),
						variables: args.variableValues,
					})
				}
			} catch (error) {
				// Shadow-mode guarantee: never break the request. If anything at
				// all throws — log stringification bombing on a circular variable,
				// print() choking on an unusual AST, anything else — swallow it.
				// The nested try around the diagnostic log itself protects against
				// a log writer that's itself broken.
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

	const operationDef = args.document.definitions.find(
		(def): def is OperationDefinitionNode =>
			typeof def === "object" && def !== null && "kind" in def && def.kind === Kind.OPERATION_DEFINITION,
	)
	if (!operationDef) return "anonymous"

	const firstField = operationDef.selectionSet.selections.find((sel): sel is FieldNode => sel.kind === Kind.FIELD)
	return firstField ? `${operationDef.operation} ${firstField.name.value}` : operationDef.operation
}
