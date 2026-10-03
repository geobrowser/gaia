/**
 * What a GraphQL request tells us about who is probing the api, and the counters that record it.
 *
 * Four signals, all cheap enough to compute on every request:
 *
 * - `parse_failed` — the document is not GraphQL. Clients we ship never send these; scanners do.
 * - `validation_failed` — valid GraphQL the schema rejects. Mostly client bugs and stale clients,
 *   but a request for fields that do not exist is how a scanner maps an api.
 * - `hidden_surface_probe` — a rejected request that names something we deliberately removed from
 *   the schema (see HIDDEN_SURFACE). No client of ours can send one, because none of them have ever
 *   used these names, so this is the high-signal tripwire.
 * - `introspection` — a successful `__schema` / `__type` query. Legitimate for GraphiQL and codegen,
 *   and also the first thing a scanner does.
 *
 * Every analysis here runs on the failure path, except introspection detection, which inspects only
 * the top level of each operation. A valid, non-introspection request pays for one shallow loop.
 */
import {type DocumentNode, type FragmentDefinitionNode, Kind, type SelectionSetNode} from "graphql"

/**
 * Names that were once public and were removed on purpose — gaia#1010 hid the notification
 * service's tables and three cron write paths after a scanner probed them on 2026-10-03.
 *
 * This is the single source of truth: the tripwire matches against it, and
 * `__tests__/hiddenSchemaSurface.test.ts` asserts none of it is in the schema. To hide something
 * new, add it here as well as omitting it, and both protections follow.
 */
export const HIDDEN_SURFACE = {
	/** GraphQL type names of the omitted tables. */
	types: ["AppWebhook", "NotificationOutbox", "NotificationDelivery", "NotificationPollCursor"],
	/** Root query fields are matched by prefix, which covers every inflection PostGraphile generated. */
	queryFieldPrefixes: ["appWebhook", "notificationOutbox", "notificationDeliver", "notificationPollCursor"],
	/** The omitted volatile functions, by their mutation names. */
	mutations: ["refreshSpaceTopicSuggestions", "sampleFeedComposition", "recordFeedCompositionSample"],
} as const

export type SignalKind = "parse_failed" | "validation_failed" | "hidden_surface_probe" | "introspection"

/** At most this many unknown names are kept per request, so a hostile document cannot bloat a log line. */
const MAX_REPORTED_NAMES = 10
/** GraphQL names are [_A-Za-z][_0-9A-Za-z]*, but truncate anyway in case that ever stops holding. */
const MAX_NAME_LENGTH = 64

export type RejectionAnalysis = {
	/** `Parent.field` for every field the schema does not have, deduplicated and capped. */
	unknownFields: string[]
	/** Type conditions naming types that do not exist, deduplicated and capped. */
	unknownTypes: string[]
	/** The HIDDEN_SURFACE entries this request named, in canonical form (`Query.<prefix>`, `Mutation.<name>`, `type:<Name>`). */
	hiddenSurfaceHits: string[]
}

function clip(name: string): string {
	return name.length > MAX_NAME_LENGTH ? `${name.slice(0, MAX_NAME_LENGTH)}…` : name
}

function hiddenQueryFieldHit(fieldName: string): string | null {
	const prefix = HIDDEN_SURFACE.queryFieldPrefixes.find((p) => fieldName.startsWith(p))
	return prefix ? `Query.${prefix}` : null
}

function hiddenMutationHit(fieldName: string): string | null {
	return (HIDDEN_SURFACE.mutations as readonly string[]).includes(fieldName) ? `Mutation.${fieldName}` : null
}

function hiddenTypeHit(typeName: string): string | null {
	return (HIDDEN_SURFACE.types as readonly string[]).includes(typeName) ? `type:${typeName}` : null
}

class CappedSet {
	private readonly items = new Set<string>()
	add(value: string): void {
		if (this.items.size < MAX_REPORTED_NAMES) this.items.add(clip(value))
	}
	toArray(): string[] {
		return [...this.items]
	}
}

/**
 * The slice of a GraphQL schema this module reads, typed structurally on purpose.
 *
 * gaia loads more than one copy of graphql-js, and the schema PostGraphile builds belongs to a
 * different copy than the one this file imports. graphql-js utilities such as TypeInfo guard with
 * `instanceof` and throw "from another module or realm" on that schema — the first version of this
 * module turned every rejected request into a 500 that way. Calling the schema's own methods never
 * crosses that boundary, so this reads nothing else.
 */
export type SchemaLike = {
	getQueryType(): NamedTypeLike | null | undefined
	getMutationType(): NamedTypeLike | null | undefined
	getType(name: string): NamedTypeLike | null | undefined
	getSubscriptionType?(): NamedTypeLike | null | undefined
}
type NamedTypeLike = {name: string; getFields?: () => Record<string, {type: unknown}>}

/** Strip NonNull/List wrappers to the named type, by shape rather than by class. */
function namedTypeOf(type: unknown): NamedTypeLike | null {
	let current = type as {ofType?: unknown; name?: unknown} | null | undefined
	for (let depth = 0; current && depth < 16; depth++) {
		if (typeof current.name === "string") return current as NamedTypeLike
		current = current.ofType as typeof current
	}
	return null
}

/** Fields of an object or interface type; null for scalars, enums, unions and unknown types. */
function fieldsOf(type: NamedTypeLike | null): Record<string, {type: unknown}> | null {
	return type && typeof type.getFields === "function" ? type.getFields() : null
}

/**
 * Work out what a rejected document asked for that the schema does not have.
 *
 * Walks the document against the schema rather than reading validation error messages, so it does
 * not depend on graphql-js wording and sees every unknown field even when validation stops reporting
 * after the first few. Bounded: each fragment is expanded at most once, and recursion depth is capped
 * well beyond anything a real query nests.
 */
export function analyzeRejectedDocument(schema: SchemaLike, document: DocumentNode): RejectionAnalysis {
	const unknownFields = new CappedSet()
	const unknownTypes = new CappedSet()
	const hits = new CappedSet()
	const queryType = schema.getQueryType() ?? null
	const mutationType = schema.getMutationType() ?? null

	const fragments = new Map<string, FragmentDefinitionNode>()
	for (const def of document.definitions) {
		if (def.kind === Kind.FRAGMENT_DEFINITION) fragments.set(def.name.value, def)
	}
	const expanded = new Set<string>()

	const checkTypeCondition = (typeName: string): NamedTypeLike | null => {
		const type = schema.getType(typeName) ?? null
		if (!type) {
			unknownTypes.add(typeName)
			const hit = hiddenTypeHit(typeName)
			if (hit) hits.add(hit)
		}
		return type
	}

	/**
	 * `parent` is the type the selections are made on, or null when it is unknown — in which case the
	 * names are still walked for type conditions, but nothing below can be called an unknown field.
	 * `rootLabel` is set for an operation's top level, where hidden root fields are matched.
	 */
	const rootLabelFor = (type: NamedTypeLike | null): "Query" | "Mutation" | null =>
		type && queryType && type.name === queryType.name
			? "Query"
			: type && mutationType && type.name === mutationType.name
				? "Mutation"
				: null

	const walk = (
		selectionSet: SelectionSetNode,
		parent: NamedTypeLike | null,
		rootLabel: "Query" | "Mutation" | null,
		depth: number,
	): void => {
		if (depth > 64) return
		const fields = fieldsOf(parent)
		for (const sel of selectionSet.selections) {
			if (sel.kind === Kind.FIELD) {
				const name = sel.name.value
				if (name.startsWith("__")) continue
				const field = fields?.[name]
				if (parent && fields && !field) {
					unknownFields.add(`${parent.name}.${name}`)
					const hit =
						rootLabel === "Query"
							? hiddenQueryFieldHit(name)
							: rootLabel === "Mutation"
								? hiddenMutationHit(name)
								: null
					if (hit) hits.add(hit)
				} else if (!parent && rootLabel === "Mutation") {
					// The schema has no Mutation type, so every mutation field is unknown.
					unknownFields.add(`Mutation.${name}`)
					const hit = hiddenMutationHit(name)
					if (hit) hits.add(hit)
				}
				if (sel.selectionSet) walk(sel.selectionSet, field ? namedTypeOf(field.type) : null, null, depth + 1)
			} else if (sel.kind === Kind.INLINE_FRAGMENT) {
				const target = sel.typeCondition ? checkTypeCondition(sel.typeCondition.name.value) : parent
				// `... on Query { appWebhooks }` is still a root selection, so keep matching it as one.
				const label = sel.typeCondition ? rootLabelFor(target) : rootLabel
				walk(sel.selectionSet, target, label, depth + 1)
			} else if (sel.kind === Kind.FRAGMENT_SPREAD) {
				const frag = fragments.get(sel.name.value)
				if (!frag || expanded.has(frag.name.value)) continue
				expanded.add(frag.name.value)
				const target = checkTypeCondition(frag.typeCondition.name.value)
				walk(frag.selectionSet, target, rootLabelFor(target), depth + 1)
			}
		}
	}

	for (const def of document.definitions) {
		if (def.kind !== Kind.OPERATION_DEFINITION) continue
		if (def.operation === "query") walk(def.selectionSet, queryType, "Query", 0)
		else if (def.operation === "mutation") walk(def.selectionSet, mutationType, "Mutation", 0)
	}
	// Fragments never spread from an operation are still part of what the caller sent.
	for (const frag of fragments.values()) {
		if (expanded.has(frag.name.value)) continue
		expanded.add(frag.name.value)
		const target = checkTypeCondition(frag.typeCondition.name.value)
		walk(frag.selectionSet, target, rootLabelFor(target), 1)
	}

	return {
		unknownFields: unknownFields.toArray(),
		unknownTypes: unknownTypes.toArray(),
		hiddenSurfaceHits: hits.toArray(),
	}
}

/**
 * True when the document runs an operation type the schema does not have — today, any mutation.
 *
 * graphql-js 16 does not treat this as a validation error: the document validates and execution
 * fails with "Schema is not configured to execute mutation operation". So a probe for a hidden
 * mutation arrives on the *valid* path, and the plugin has to ask this to see it.
 */
export function targetsMissingRootType(schema: SchemaLike, document: DocumentNode): boolean {
	for (const def of document.definitions) {
		if (def.kind !== Kind.OPERATION_DEFINITION) continue
		if (def.operation === "mutation" && !schema.getMutationType()) return true
		if (def.operation === "subscription" && !schema.getSubscriptionType?.()) return true
	}
	return false
}

export type IntrospectionKind = "schema" | "type"

/**
 * Which introspection root fields a document selects at the top level of any operation, following
 * fragment spreads one level. `__typename` is not introspection in any useful sense and is ignored.
 */
export function detectIntrospection(document: DocumentNode): IntrospectionKind[] {
	const found = new Set<IntrospectionKind>()
	const fragments = new Map<string, SelectionSetNode>()
	for (const def of document.definitions) {
		if (def.kind === Kind.FRAGMENT_DEFINITION) fragments.set(def.name.value, def.selectionSet)
	}

	const scan = (selectionSet: SelectionSetNode, followSpreads: boolean): void => {
		for (const sel of selectionSet.selections) {
			if (sel.kind === Kind.FIELD) {
				if (sel.name.value === "__schema") found.add("schema")
				else if (sel.name.value === "__type") found.add("type")
			} else if (sel.kind === Kind.INLINE_FRAGMENT) {
				scan(sel.selectionSet, followSpreads)
			} else if (sel.kind === Kind.FRAGMENT_SPREAD && followSpreads) {
				const frag = fragments.get(sel.name.value)
				if (frag) scan(frag, false)
			}
		}
	}

	for (const def of document.definitions) {
		if (def.kind === Kind.OPERATION_DEFINITION) scan(def.selectionSet, true)
	}
	return [...found]
}

// ---------------------------------------------------------------------------------------------
// Counters. Accumulated since process start; Prometheus derives rates. Every label set is fixed and
// small — never an IP, a field name a client chose, or anything else an attacker can make unique.
// ---------------------------------------------------------------------------------------------

export type RejectionReason = "parse" | "unknown_field" | "other_validation"

const rejectedByReason = new Map<RejectionReason, number>()
const hiddenSurfaceProbesByTarget = new Map<string, number>()
const introspectionByKind = new Map<IntrospectionKind, number>()
let logLinesSuppressed = 0
let internalErrors = 0

export function countRejection(reason: RejectionReason): void {
	rejectedByReason.set(reason, (rejectedByReason.get(reason) ?? 0) + 1)
}

/** Every canonical hit `analyzeRejectedDocument` can report, which is the whole label value set. */
export const ALL_HIDDEN_TARGETS: readonly string[] = [
	...HIDDEN_SURFACE.queryFieldPrefixes.map((p) => `Query.${p}`),
	...HIDDEN_SURFACE.mutations.map((m) => `Mutation.${m}`),
	...HIDDEN_SURFACE.types.map((t) => `type:${t}`),
]
const KNOWN_TARGETS = new Set(ALL_HIDDEN_TARGETS)

/** Counts only canonical targets, so this label can never take a value an attacker chose. */
export function countHiddenSurfaceProbe(target: string): void {
	if (!KNOWN_TARGETS.has(target)) return
	hiddenSurfaceProbesByTarget.set(target, (hiddenSurfaceProbesByTarget.get(target) ?? 0) + 1)
}

export function countIntrospection(kind: IntrospectionKind): void {
	introspectionByKind.set(kind, (introspectionByKind.get(kind) ?? 0) + 1)
}

export function countSuppressedLogLine(): void {
	logLinesSuppressed++
}

/** The signal pipeline itself failed. It must never affect the request, so this is the only trace. */
export function countInternalError(): void {
	internalErrors++
}

export type SourceTrackerStats = {sources: number; evictions: number}

/** Prometheus exposition for the counters above, plus the source tracker's own health. */
export function renderSecuritySignalMetrics(tracker: SourceTrackerStats): string {
	const lines: string[] = []
	const family = (name: string, type: "counter" | "gauge", help: string) => {
		lines.push(`# HELP ${name} ${help}`, `# TYPE ${name} ${type}`)
	}

	family(
		"gaia_api_graphql_rejected_total",
		"counter",
		"GraphQL requests rejected before execution, by reason: parse (not GraphQL), unknown_field (named a field or type the schema lacks), other_validation.",
	)
	for (const reason of ["parse", "unknown_field", "other_validation"] as const) {
		lines.push(`gaia_api_graphql_rejected_total{reason="${reason}"} ${rejectedByReason.get(reason) ?? 0}`)
	}

	family(
		"gaia_api_graphql_hidden_surface_probe_total",
		"counter",
		"Rejected GraphQL requests that named deliberately hidden schema surface. No first-party client sends these.",
	)
	// Every target is exposed from process start, at 0 until probed. A series that is born at 1 has no
	// earlier sample, so `increase()` reads it as no change and the alert would miss the first probe
	// after every deploy — exactly the one that matters.
	for (const target of ALL_HIDDEN_TARGETS) {
		lines.push(
			`gaia_api_graphql_hidden_surface_probe_total{target="${target}"} ${hiddenSurfaceProbesByTarget.get(target) ?? 0}`,
		)
	}

	family(
		"gaia_api_graphql_introspection_total",
		"counter",
		"Successful GraphQL introspection requests, by root field.",
	)
	for (const kind of ["schema", "type"] as const) {
		lines.push(`gaia_api_graphql_introspection_total{kind="${kind}"} ${introspectionByKind.get(kind) ?? 0}`)
	}

	family(
		"gaia_api_security_signal_log_suppressed_total",
		"counter",
		"Security signal log lines withheld by per-source or global rate limits. The events are still counted above.",
	)
	lines.push(`gaia_api_security_signal_log_suppressed_total ${logLinesSuppressed}`)

	family(
		"gaia_api_security_signal_internal_errors_total",
		"counter",
		"Failures inside security signal detection itself. Requests are unaffected; nonzero means detection is partly blind.",
	)
	lines.push(`gaia_api_security_signal_internal_errors_total ${internalErrors}`)

	family(
		"gaia_api_security_signal_sources",
		"gauge",
		"Distinct caller addresses currently tracked for log rate limiting.",
	)
	lines.push(`gaia_api_security_signal_sources ${tracker.sources}`)

	family(
		"gaia_api_security_signal_source_evictions_total",
		"counter",
		"Tracked caller addresses evicted to keep the tracker within its memory bound.",
	)
	lines.push(`gaia_api_security_signal_source_evictions_total ${tracker.evictions}`)

	return `${lines.join("\n")}\n`
}

/** Reset accumulated counters. Tests only. */
export function __resetSecuritySignalMetricsForTests(): void {
	rejectedByReason.clear()
	hiddenSurfaceProbesByTarget.clear()
	introspectionByKind.clear()
	logLinesSuppressed = 0
	internalErrors = 0
}
