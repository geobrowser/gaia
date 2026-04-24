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
const COST_BUCKET_EDGES: readonly number[] = [30, 60, 90, 120, 150, 180, 200, 220, 240, 260, 280, 300, 500, 1000]

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
export function computeQueryCostRaw(doc: DocumentNode, variables: Record<string, unknown> = {}): bigint {
	const op = doc.definitions.find((d): d is OperationDefinitionNode => d.kind === Kind.OPERATION_DEFINITION)
	if (!op) return 0n
	return sumSelections(op.selectionSet, variables, doc, new Set())
}

/**
 * Compute a compressed query complexity score: `round(log₁₀(rawCost) × 10)`.
 * See the constant block above for interpretive landmarks.
 *
 * Returns 0 for trivial queries; typical heavy prod queries land 150–250;
 * structurally-unbounded shapes exceed 240.
 */
export function computeQueryCost(doc: DocumentNode, variables: Record<string, unknown> = {}): number {
	return compressCost(computeQueryCostRaw(doc, variables))
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

function sumSelections(
	selectionSet: SelectionSetNode,
	vars: Record<string, unknown>,
	doc: DocumentNode,
	visitedFragments: Set<string>,
): bigint {
	let total = 0n
	for (const sel of selectionSet.selections) {
		if (sel.kind === Kind.FIELD) {
			total += fieldCost(sel, vars, doc, visitedFragments)
		} else if (sel.kind === Kind.INLINE_FRAGMENT) {
			total += sumSelections(sel.selectionSet, vars, doc, visitedFragments)
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
				total += sumSelections(frag.selectionSet, vars, doc, visitedFragments)
				visitedFragments.delete(name)
			}
		}
	}
	return total
}

/**
 * Cost model per field:
 *   - Explicit `first` / `last` arg with a valid positive value → `child × limit + 1`.
 *   - Pagination arg present but value unresolved / non-positive → fall back
 *     to MAX_PAGINATION_LIMIT (1000) — matches the effective cap at SQL build.
 *   - No pagination arg, has selections → `child × MAX_PAGINATION_LIMIT + 1`.
 *     PaginationCapPlugin injects `first: 1000` on every unpaginated
 *     collection field at SQL build, so modeling the default at 1000 matches
 *     the real fan-out ceiling. BigInt arithmetic means this doesn't overflow.
 *   - No pagination arg, no selections → 1 (scalar leaf).
 */
const DEFAULT_LIMIT = BigInt(MAX_PAGINATION_LIMIT)

function fieldCost(
	field: FieldNode,
	vars: Record<string, unknown>,
	doc: DocumentNode,
	visitedFragments: Set<string>,
): bigint {
	// Check for the *syntactic* presence of first/last. `entities(first: $n)`
	// with `$n` unresolved still means the caller intended pagination — treat
	// it as a list (and fall back to MAX_PAGINATION_LIMIT since the effective
	// limit is undefined).
	const argNames = new Set((field.arguments ?? []).map((a) => a.name.value))
	const hasPagination = argNames.has("first") || argNames.has("last")

	const child = field.selectionSet ? sumSelections(field.selectionSet, vars, doc, visitedFragments) : 0n

	if (hasPagination) {
		const args = resolveArgs(field.arguments, vars)
		const raw = args.first ?? args.last
		const validExplicit = typeof raw === "number" && Number.isFinite(raw) && raw > 0
		const limit = validExplicit ? BigInt(raw as number) : DEFAULT_LIMIT
		return child * limit + 1n
	}

	return child * DEFAULT_LIMIT + 1n
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
 *     query's compressed score; charted in Grafana for distribution.
 *   - `log.warn("High GraphQL query cost", ...)` when score ≥
 *     `COST_LOG_THRESHOLD`. `log.warn` always writes to stdout (visible in
 *     kubectl + Axiom) *and* drops a Sentry breadcrumb, but does NOT create
 *     a Sentry issue.
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

	const operationDef = args.document.definitions.find(
		(def): def is OperationDefinitionNode =>
			typeof def === "object" && def !== null && "kind" in def && def.kind === Kind.OPERATION_DEFINITION,
	)
	if (!operationDef) return "anonymous"

	const firstField = operationDef.selectionSet.selections.find((sel): sel is FieldNode => sel.kind === Kind.FIELD)
	return firstField ? `${operationDef.operation} ${firstField.name.value}` : operationDef.operation
}
