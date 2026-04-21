import * as Sentry from "@sentry/node"
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

// Threshold above which we warn (log + Sentry issue). Phase 1 tuning knob —
// start from a ceiling that accommodates the conservative model (fields
// without explicit first/last are scored as if they returned MAX items,
// so nested structured responses score in the millions), then re-tune from
// observed distribution.
const COST_WARN_THRESHOLD = Number.parseInt(process.env.GRAPHQL_COST_WARN_THRESHOLD ?? "1000000", 10)

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
 * Intentionally schema-free: walking only the document + variables means the
 * estimator works regardless of which `graphql` module instance built the
 * schema (PostGraphile v4 bundles its own copy). Schema-aware libraries like
 * graphql-query-complexity hit instanceof failures across the CJS/ESM module
 * boundary in that setup.
 */
export function computeQueryCost(doc: DocumentNode, variables: Record<string, unknown> = {}): number {
	const op = doc.definitions.find((d): d is OperationDefinitionNode => d.kind === Kind.OPERATION_DEFINITION)
	if (!op) return 0
	return sumSelections(op.selectionSet, variables, doc)
}

function sumSelections(selectionSet: SelectionSetNode, vars: Record<string, unknown>, doc: DocumentNode): number {
	let total = 0
	for (const sel of selectionSet.selections) {
		if (sel.kind === Kind.FIELD) {
			total += fieldCost(sel, vars, doc)
		} else if (sel.kind === Kind.INLINE_FRAGMENT) {
			total += sumSelections(sel.selectionSet, vars, doc)
		} else if (sel.kind === Kind.FRAGMENT_SPREAD) {
			const frag = doc.definitions.find(
				(d): d is FragmentDefinitionNode =>
					d.kind === Kind.FRAGMENT_DEFINITION && d.name.value === sel.name.value,
			)
			if (frag) total += sumSelections(frag.selectionSet, vars, doc)
		}
	}
	return total
}

function fieldCost(field: FieldNode, vars: Record<string, unknown>, doc: DocumentNode): number {
	// Check for the *syntactic* presence of first/last in the query, not the
	// resolved value. `entities(first: $first)` with $first unresolved means
	// the user intends pagination — we should still treat it as a list (and
	// fall back to MAX because the effective limit is undefined).
	const argNames = new Set((field.arguments ?? []).map((a) => a.name.value))
	const hasPagination = argNames.has("first") || argNames.has("last")

	const child = field.selectionSet ? sumSelections(field.selectionSet, vars, doc) : 0

	if (hasPagination) {
		const args = resolveArgs(field.arguments, vars)
		const raw = args.first ?? args.last
		const validExplicit = typeof raw === "number" && Number.isFinite(raw) && raw > 0
		const limit = validExplicit ? (raw as number) : MAX_PAGINATION_LIMIT
		return child * limit + 1
	}

	// Structured field without an explicit limit: conservatively assume the
	// SQL-injected default of MAX_PAGINATION_LIMIT applies. Without this we'd
	// miss every `{ entities { ... } }`-style query that leans on the implicit
	// cap. Leaf scalars (no selection set → child=0) collapse to `0 × N + 1 = 1`
	// here, so we don't need a separate branch.
	return child * MAX_PAGINATION_LIMIT + 1
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
 * Yoga plugin that computes query cost and logs / Sentry-alerts when it
 * exceeds `COST_WARN_THRESHOLD`. Phase 1 is strictly observational — the
 * plugin never rejects a query. Phase 2 will add a hard ceiling by comparing
 * the same computed cost against a configured limit.
 */
export function useCostLogger(): Plugin {
	return {
		onExecute({args}) {
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

			const operationLabel = getOperationLabel(args)
			const fullQuery = print(args.document)
			const queryFingerprint = graphqlQueryFingerprint(fullQuery)

			Sentry.addBreadcrumb({
				category: "graphql.cost",
				message: "Computed GraphQL query cost",
				data: {cost, operationName: operationLabel, queryFingerprint},
				level: "info",
			})

			if (cost >= COST_WARN_THRESHOLD) {
				log.warn("High GraphQL query cost", {
					cost,
					threshold: COST_WARN_THRESHOLD,
					operationName: operationLabel,
					queryFingerprint,
					query: fullQuery.slice(0, 2000),
					variables: args.variableValues,
				})

				Sentry.captureMessage("High GraphQL query cost", {
					level: "warning",
					tags: {
						"graphql.operation_name": operationLabel,
						"graphql.query_fingerprint": queryFingerprint,
					},
					extra: {
						cost,
						threshold: COST_WARN_THRESHOLD,
						query: fullQuery.slice(0, 2000),
						variables: args.variableValues,
					},
				})
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
