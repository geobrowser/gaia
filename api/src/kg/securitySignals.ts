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
 * - `introspection` — the operation that executes selects `__schema` / `__type`. Legitimate for
 *   GraphiQL and codegen, and also the first thing a scanner does. This records what was ASKED for:
 *   `@skip` / `@include` are not evaluated, because for security the request is the signal.
 *
 * Every analysis here runs on the failure path, except introspection detection, which inspects only
 * the top level of each operation. A valid, non-introspection request pays for one shallow loop.
 */
import {
	type DocumentNode,
	type FragmentDefinitionNode,
	Kind,
	type OperationDefinitionNode,
	type SelectionSetNode,
	type TypeNode,
} from "graphql"

/**
 * Names that were once public and were removed on purpose — gaia#1010 hid the notification
 * service's tables and three cron write paths after a scanner probed them on 2026-10-03 — and
 * names of private data that must never become public (GEO-3088's per-user interest data).
 *
 * This is the single source of truth: the tripwire matches against it, and
 * `__tests__/hiddenSchemaSurface.test.ts` asserts none of it is in the schema. To hide something
 * new, add it here as well as omitting it, and both protections follow.
 */
export const HIDDEN_SURFACE = {
	/** GraphQL type names of the omitted tables. */
	types: [
		"AppWebhook",
		"NotificationOutbox",
		"NotificationDelivery",
		"NotificationPollCursor",
		// GEO-3088's per-user interest data (migration 0100). These were never public: they live in
		// the `personalization` schema, which PostGraphile does not introspect. Listed so the test
		// fails if they are ever moved into `public`, and so a request naming them is a probe.
		"UserTopicSignal",
		"TopicCooccurrence",
		"ExternalInterestSignal",
		"InterestConfig",
		"InterestSignalWeight",
		"InterestSweepState",
		"InterestRefitRun",
		// GEO-3141 account weights (migration 0101).
		"AccountExclusion",
		"AccountWeight",
		// GEO-3140 / GEO-3144 For you serving and feed experiments (migration 0102), private schema.
		"ForYouConfig",
		"FeedConfigRevision",
		"FeedExperiment",
		"FeedExperimentMember",
		// GEO-3143 primer and anchor selection state (migration 0103). Built from account weights,
		// so private; the public reads are `nextPrimerClaim` and `anchorClaims`, which return
		// entities only.
		"PrimerClaimStat",
		"AnchorClaimSet",
		"AnchorClaimMember",
		"ClaimOverlapSample",
	],
	/** Root query fields are matched by prefix, which covers every inflection PostGraphile generated. */
	queryFieldPrefixes: [
		"appWebhook",
		"notificationOutbox",
		"notificationDeliver",
		"notificationPollCursor",
		// GEO-3088 tables, and its read functions (STABLE, so they would be query fields).
		"userTopicSignal",
		"topicCooccurrence",
		"externalInterestSignal",
		"interestConfig",
		"interestSignalWeight",
		"interestSweepState",
		"interestRefitRun",
		"userTopicWeights",
		"userTopicContributions",
		"userTopicInterest",
		"userInterestEvents",
		"computeUserTopicSignals",
		"dirtyInterestUsers",
		"accountExclusion",
		"accountWeight",
		"accountVoteWeight",
		"forYouConfig",
		"forYouCandidates",
		"feedConfigRevision",
		"feedExperiment",
		"primerClaimStat",
		"primerCandidate",
		"anchorClaimSet",
		"anchorClaimMember",
		"claimOverlap",
		"binaryEntropyBits",
	],
	/** The omitted volatile functions, by their mutation names. */
	mutations: [
		"refreshSpaceTopicSuggestions",
		"sampleFeedComposition",
		"recordFeedCompositionSample",
		// GEO-3088's write paths, run by ranking-indexer's topic_interest CronJobs.
		"sweepUserTopicInterest",
		"refitUserTopicInterest",
		"recomputeUserTopicInterest",
		"refreshTopicCooccurrence",
		"refreshAccountWeights",
		"replaceAccountExclusions",
		// GEO-3088's Not interested sync (migration 0105), run by ranking-indexer's not_interested_sync.
		"replaceExternalInterestSignals",
		// GEO-3143's write paths, run by ranking-indexer's primer-claims CronJob.
		"refreshPrimerClaimStats",
		"refreshAnchorClaims",
		"recordClaimOverlapSample",
	],
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
 * not depend on graphql-js wording and sees every unknown name even when validation stops reporting
 * after the first few.
 *
 * Shaped to resist evasion, because gaia is public and anyone can read this function:
 * - Every operation and every fragment DEFINITION is walked exactly once, in its own type context —
 *   including unused fragments and duplicate names (which fail validation, but still say what the
 *   caller wanted). Spreads are never followed, so there is no cycle to guard and no way to make one
 *   definition shadow another.
 * - The walk is iterative with an explicit queue and no depth limit, so no amount of nesting reaches
 *   a cutoff. Each selection is visited once: work is linear in the size of the document, which the
 *   request body limit already bounds.
 * - Inside `__schema` / `__type` the walk continues against `__Schema` / `__Type`, variable
 *   declarations are checked for unknown types, and subscriptions are walked like any other root.
 */
export function analyzeRejectedDocument(schema: SchemaLike, document: DocumentNode): RejectionAnalysis {
	const unknownFields = new CappedSet()
	const unknownTypes = new CappedSet()
	// Not capped: every hit is one of ALL_HIDDEN_TARGETS, so the fixed list already bounds it, and
	// dropping one would lose that target's count and alert.
	const hits = new Set<string>()

	type Root = "Query" | "Mutation" | "Subscription"
	const roots: Record<Root, NamedTypeLike | null> = {
		Query: schema.getQueryType() ?? null,
		Mutation: schema.getMutationType() ?? null,
		Subscription: schema.getSubscriptionType?.() ?? null,
	}
	// Decided by NAME, not by resolving the type: with no Mutation type in the schema,
	// `... on Mutation { sampleFeedComposition }` resolves to nothing, but it is still a mutation-root
	// selection and must still be matched as one.
	const rootLabelFor = (typeName: string): Root | null => {
		for (const root of ["Query", "Mutation", "Subscription"] as const) {
			if (typeName === (roots[root]?.name ?? root)) return root
		}
		return null
	}

	const checkNamedType = (typeName: string): NamedTypeLike | null => {
		const type = schema.getType(typeName) ?? null
		if (!type) {
			unknownTypes.add(typeName)
			const hit = hiddenTypeHit(typeName)
			if (hit) hits.add(hit)
		}
		return type
	}

	/** `parent` is null when unknown; `root` is set only for selections made directly on a root type. */
	type Frame = {selectionSet: SelectionSetNode; parent: NamedTypeLike | null; root: Root | null}
	// A FIFO queue read by index: linear, and names come out in document order, which reads better in
	// a log line than the reverse order a stack would give.
	const queue: Frame[] = []

	for (const def of document.definitions) {
		if (def.kind === Kind.OPERATION_DEFINITION) {
			for (const variable of def.variableDefinitions ?? []) {
				let typeNode: TypeNode = variable.type
				while (typeNode.kind !== Kind.NAMED_TYPE) typeNode = typeNode.type
				checkNamedType(typeNode.name.value)
			}
			const root: Root =
				def.operation === "query" ? "Query" : def.operation === "mutation" ? "Mutation" : "Subscription"
			queue.push({selectionSet: def.selectionSet, parent: roots[root], root})
		} else if (def.kind === Kind.FRAGMENT_DEFINITION) {
			const typeName = def.typeCondition.name.value
			queue.push({selectionSet: def.selectionSet, parent: checkNamedType(typeName), root: rootLabelFor(typeName)})
		}
	}

	for (let i = 0; i < queue.length; i++) {
		const frame = queue[i] as Frame
		const {parent, root} = frame
		const fields = fieldsOf(parent)
		for (const sel of frame.selectionSet.selections) {
			if (sel.kind === Kind.FIELD) {
				const name = sel.name.value
				// Only the real meta-fields are exempt. Anything else with a `__` prefix is an unknown field
				// like any other — skipping the prefix would let `{ __anything }` pass as an ordinary
				// validation failure, which the surge alert does not count.
				if (name === "__typename") {
					// The field is fine; a selection set under it is not, and can still hide a type condition
					// (`{ __typename { ... on AppWebhook { secret } } }`). Walk it with no parent type.
					if (sel.selectionSet) queue.push({selectionSet: sel.selectionSet, parent: null, root: null})
					continue
				}
				if (root === "Query" && (name === "__schema" || name === "__type")) {
					if (sel.selectionSet) {
						const metaType = schema.getType(name === "__schema" ? "__Schema" : "__Type") ?? null
						queue.push({selectionSet: sel.selectionSet, parent: metaType, root: null})
					}
					continue
				}
				const field = fields?.[name]
				if (parent && fields && !field) {
					unknownFields.add(`${parent.name}.${name}`)
					const hit =
						root === "Query"
							? hiddenQueryFieldHit(name)
							: root === "Mutation"
								? hiddenMutationHit(name)
								: null
					if (hit) hits.add(hit)
				} else if (!parent && (root === "Mutation" || root === "Subscription")) {
					// The schema has no such root type, so every field on it is unknown.
					unknownFields.add(`${root}.${name}`)
					const hit = root === "Mutation" ? hiddenMutationHit(name) : null
					if (hit) hits.add(hit)
				}
				if (sel.selectionSet) {
					const child = field ? namedTypeOf(field.type) : null
					queue.push({
						selectionSet: sel.selectionSet,
						parent: child,
						// PostGraphile gives every type a `query: Query!` field, so `{ query { appWebhooks } }` is a
						// root-level probe one level down. Whether selections are "on a root" depends on the type
						// they are made on, not on how deep they sit.
						root: child ? rootLabelFor(child.name) : null,
					})
				}
			} else if (sel.kind === Kind.INLINE_FRAGMENT) {
				if (sel.typeCondition) {
					const typeName = sel.typeCondition.name.value
					queue.push({
						selectionSet: sel.selectionSet,
						parent: checkNamedType(typeName),
						root: rootLabelFor(typeName),
					})
				} else {
					queue.push({selectionSet: sel.selectionSet, parent, root})
				}
			}
			// FRAGMENT_SPREAD: nothing to do — every definition is walked on its own above.
		}
	}

	return {
		unknownFields: unknownFields.toArray(),
		unknownTypes: unknownTypes.toArray(),
		hiddenSurfaceHits: [...hits],
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

/**
 * True when the operation that will actually execute targets a root type the schema lacks, i.e. the
 * request fails. `targetsMissingRootType` asks the same of the whole document, which is right for
 * probe detection (naming a hidden mutation anywhere shows intent) but wrong for counting rejections:
 * an unused mutation beside a query that runs fine is not a rejected request.
 */
export function selectedOperationTargetsMissingRoot(
	schema: SchemaLike,
	document: DocumentNode,
	operationName?: string | null,
): boolean {
	const operation = selectedOperation(document, operationName)
	if (!operation) return false
	if (operation.operation === "mutation") return !schema.getMutationType()
	if (operation.operation === "subscription") return !schema.getSubscriptionType?.()
	return false
}

export type IntrospectionKind = "schema" | "type"

/**
 * The operation graphql-js will execute: the one named `operationName`, or the only one when no name
 * is given. Null when execution would fail to pick one — then nothing in the document runs.
 */
function selectedOperation(document: DocumentNode, operationName?: string | null): OperationDefinitionNode | null {
	const operations = document.definitions.filter(
		(def): def is OperationDefinitionNode => def.kind === Kind.OPERATION_DEFINITION,
	)
	if (operationName) return operations.find((op) => op.name?.value === operationName) ?? null
	return operations.length === 1 ? (operations[0] ?? null) : null
}

/**
 * Which introspection fields the operation that will actually execute selects. Other operations in
 * the document never run, so they are not counted.
 *
 * graphql-js resolves `__schema` / `__type` on the Query type wherever it appears, and PostGraphile
 * gives every type a `query: Query!` field, so `{ query { __schema { … } } }` is introspection too.
 * With a schema, the walk therefore follows any field whose type is Query, as well as fragments
 * (each once) on it; it never descends into other fields, so a normal query costs one field lookup
 * per top-level selection. Without a schema only the top level is checked. `__typename` is ignored.
 */
export function detectIntrospection(
	document: DocumentNode,
	operationName?: string | null,
	schema?: SchemaLike,
): IntrospectionKind[] {
	const operation = selectedOperation(document, operationName)
	if (!operation || operation.operation !== "query") return []
	const queryType = schema?.getQueryType() ?? null
	const queryName = queryType?.name ?? "Query"
	const queryFields = fieldsOf(queryType)
	const found = new Set<IntrospectionKind>()
	const fragments = new Map<string, FragmentDefinitionNode>()
	for (const def of document.definitions) {
		if (def.kind === Kind.FRAGMENT_DEFINITION) fragments.set(def.name.value, def)
	}

	// Every selection set queued here is made on the Query type.
	const visited = new Set<string>()
	const pending: SelectionSetNode[] = [operation.selectionSet]
	for (let i = 0; i < pending.length; i++) {
		for (const sel of (pending[i] as SelectionSetNode).selections) {
			if (sel.kind === Kind.FIELD) {
				const name = sel.name.value
				if (name === "__schema") found.add("schema")
				else if (name === "__type") found.add("type")
				else if (sel.selectionSet && namedTypeOf(queryFields?.[name]?.type)?.name === queryName) {
					pending.push(sel.selectionSet)
				}
			} else if (sel.kind === Kind.INLINE_FRAGMENT) {
				if (!sel.typeCondition || sel.typeCondition.name.value === queryName) pending.push(sel.selectionSet)
			} else if (sel.kind === Kind.FRAGMENT_SPREAD && !visited.has(sel.name.value)) {
				visited.add(sel.name.value)
				const frag = fragments.get(sel.name.value)
				if (frag && frag.typeCondition.name.value === queryName) pending.push(frag.selectionSet)
			}
		}
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
		"GraphQL requests whose executing operation selects introspection, by root field. Counts what was asked for; @skip/@include are not evaluated.",
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
