/**
 * Yoga plugin that turns rejected and introspection requests into security signals.
 *
 * For each signal (see securitySignals.ts) it does three things, each with its own budget:
 *
 * 1. COUNT every occurrence in Prometheus. Counters are what alert rules read, so they are never
 *    sampled. Labels are a fixed set — never an IP or a client-chosen name.
 * 2. LOG a structured `warn` line, with the caller and what they asked for, through a per-source
 *    token bucket and a global ceiling (`sourceTracker.ts`). The first events from a source are
 *    logged in full and the rest are counted into `suppressedSinceLastEmit` on the next line that is
 *    logged, so a flood stays visible as a number rather than as a flood. These lines go to stdout
 *    so a log shipper can retain them; nothing else here depends on that.
 * 3. RAISE a Sentry issue for a hidden-surface probe, at most once per source per hour and a few per
 *    ten minutes overall, so a scanner rotating addresses cannot spend the Sentry quota (GEO-2845
 *    is what happens when that quota runs out). The Prometheus alert is what pages; the Sentry issue
 *    carries the context the metric cannot.
 *
 * Detection reads the parsed document and the validation result only. It never touches the
 * database, and on a valid request it costs one loop over the top-level selections.
 */
import type {DocumentNode, GraphQLError} from "graphql"
import type {Plugin} from "graphql-yoga"
import {log} from "../services/telemetry"
import {extractClientIp} from "../utils/clientIp"
import {createErrorEpisodeTracker} from "./errorEpisodeTracker"
import {
	analyzeRejectedDocument,
	countHiddenSurfaceProbe,
	countInternalError,
	countIntrospection,
	countRejection,
	countSuppressedLogLine,
	detectIntrospection,
	type IntrospectionKind,
	type RejectionAnalysis,
	renderSecuritySignalMetrics,
	type SchemaLike,
	type SignalKind,
	targetsMissingRootType,
} from "./securitySignals"
import {createSourceTracker, type SourceTracker} from "./sourceTracker"

/** Budgets for the structured log stream. */
const STREAM_LIMITS = {
	maxSources: 10_000,
	// A source's first 20 signals are logged in full — enough to see what a scan was doing — then
	// one every five seconds, with the count of what was withheld in between.
	burst: 20,
	refillPerSec: 0.2,
	quietMs: 10 * 60_000,
	// Across all sources: bursts of 200 lines, 20 a second sustained. Far above organic volume, far
	// below what would strain stdout or a shipper.
	global: {burst: 200, refillPerSec: 20},
}

/** Budgets for Sentry issues raised by hidden-surface probes. */
const ALERT_LIMITS = {
	maxSources: 10_000,
	burst: 1,
	refillPerSec: 1 / 3600,
	quietMs: 30 * 60_000,
	global: {burst: 5, refillPerSec: 1 / 600},
}

const UNKNOWN_SOURCE = "unknown"

type RequestFacts = {
	clientIp: string | null
	userAgent: string | null
	origin: string | null
	operationName: string | null
	requestId?: string
}

type YogaContext = {
	request?: Request
	requestId?: string
	params?: {operationName?: string | null}
}

function truncate(value: string | null | undefined, max: number): string | null {
	if (value == null) return null
	return value.length > max ? `${value.slice(0, max)}…` : value
}

function requestFacts(context: unknown, document?: DocumentNode): RequestFacts {
	const ctx = (context ?? {}) as YogaContext
	const headers = ctx.request?.headers
	let operationName = ctx.params?.operationName ?? null
	if (!operationName && document) {
		for (const def of document.definitions) {
			if (def.kind === "OperationDefinition" && def.name) {
				operationName = def.name.value
				break
			}
		}
	}
	return {
		clientIp: headers ? extractClientIp(headers) : null,
		userAgent: truncate(headers?.get("user-agent"), 256),
		origin: truncate(headers?.get("origin"), 256),
		operationName: truncate(operationName, 128),
		requestId: ctx.requestId,
	}
}

export type SecuritySignalsPluginOptions = {
	/** Injected in tests; production uses the module defaults above. */
	streamTracker?: SourceTracker
	alertTracker?: SourceTracker
	analyze?: (schema: SchemaLike, document: DocumentNode) => RejectionAnalysis
}

// Detection failing is logged at onset and then once per heartbeat, never per request.
const internalErrorEpisodes = createErrorEpisodeTracker({quietMs: 10 * 60_000, heartbeatMs: 60 * 60_000})

/**
 * Run detection without any possibility of affecting the request.
 *
 * This plugin observes; it must never decide. A throw inside a validation hook surfaces to the
 * client as a 500 — the first version of this plugin did exactly that to every rejected request — so
 * every hook body runs inside this guard, and a failure is counted, logged rarely, and dropped.
 */
function guarded(stage: string, fn: () => void): void {
	try {
		fn()
	} catch (err) {
		countInternalError()
		// Reporting the failure must not become a failure of its own. The logger may be what threw in
		// the first place, and `String(err)` can throw for a hostile value (a throwing toString or
		// Symbol.toPrimitive), so this path gets its own guard and gives up silently: the counter above
		// has already recorded it.
		try {
			const episode = internalErrorEpisodes.record(stage)
			if (episode.shouldLog) {
				log.warn("Security signal detection failed; request unaffected", {
					stage,
					error: truncate(String(err), 300),
					occurrencesInEpisode: episode.total,
					suppressedSinceLastLog: episode.suppressed,
				})
			}
		} catch {
			// Deliberately empty: see above.
		}
	}
}

let activeStreamTracker: SourceTracker | null = null

/**
 * Prometheus exposition for the signal counters and the live tracker's health.
 *
 * Empty until `useSecuritySignals` has run. `/health/metrics` calls this whether or not the plugin is
 * registered, and exporting zeros without it would make a removed plugin look healthy — the counters
 * would still be present, so `ApiSecuritySignalsMissing` could never fire for the one failure it is
 * there to catch.
 */
export function renderSecuritySignalsPrometheus(): string {
	if (!activeStreamTracker) return ""
	return renderSecuritySignalMetrics({
		sources: activeStreamTracker.size(),
		evictions: activeStreamTracker.evictions(),
	})
}

export function useSecuritySignals(options: SecuritySignalsPluginOptions = {}): Plugin {
	const stream = options.streamTracker ?? createSourceTracker(STREAM_LIMITS)
	const alerts = options.alertTracker ?? createSourceTracker(ALERT_LIMITS)
	const analyze = options.analyze ?? analyzeRejectedDocument
	activeStreamTracker = stream

	const emit = (signal: SignalKind, facts: RequestFacts, details: Record<string, unknown>): void => {
		const decision = stream.record(facts.clientIp ?? UNKNOWN_SOURCE)
		if (!decision.emit) {
			countSuppressedLogLine()
			return
		}
		log.warn("GraphQL security signal", {
			signal,
			...facts,
			...details,
			newEpisode: decision.isNewEpisode,
			episodeEvents: decision.episodeTotal,
			suppressedSinceLastEmit: decision.suppressedSinceLastEmit,
		})
	}

	return {
		onParse() {
			return ({result, context}) =>
				guarded("parse", () => {
					// Duck-typed: the error may come from another copy of graphql-js (see SchemaLike).
					if (
						!result ||
						typeof (result as {message?: unknown}).message !== "string" ||
						"definitions" in result
					)
						return
					countRejection("parse")
					emit("parse_failed", requestFacts(context), {error: truncate((result as Error).message, 200)})
				})
		},

		onValidate({params, context}) {
			const schema = params.schema as unknown as SchemaLike
			const document = params.documentAST as DocumentNode
			// Rejected documents, and the ones graphql-js lets through only to fail at execution.
			const handleRejection = (errorCount: number, firstError: string | undefined): void => {
				const analysis = analyze(schema, document)
				const namedUnknown = analysis.unknownFields.length > 0 || analysis.unknownTypes.length > 0
				countRejection(namedUnknown ? "unknown_field" : "other_validation")
				for (const target of analysis.hiddenSurfaceHits) countHiddenSurfaceProbe(target)

				const facts = requestFacts(context, document)
				const isProbe = analysis.hiddenSurfaceHits.length > 0
				const details = {
					unknownFields: analysis.unknownFields,
					unknownTypes: analysis.unknownTypes,
					hiddenSurface: analysis.hiddenSurfaceHits,
					errorCount,
					firstError: truncate(firstError, 200),
				}
				emit(isProbe ? "hidden_surface_probe" : "validation_failed", facts, details)

				if (isProbe && alerts.record(facts.clientIp ?? UNKNOWN_SOURCE).emit) {
					// One stable message so Sentry groups every probe into a single issue that regresses,
					// rather than opening one per address.
					log.error("Hidden GraphQL surface probed", {...facts, ...details})
				}
			}

			return ({valid, result}) =>
				guarded("validate", () => {
					if (!valid) {
						const errors = result as readonly GraphQLError[]
						handleRejection(errors.length, errors[0]?.message)
						return
					}
					if (targetsMissingRootType(schema, document)) {
						handleRejection(0, "operation type not in schema")
						return
					}
					const operationName = (context as YogaContext | undefined)?.params?.operationName
					const kinds: IntrospectionKind[] = detectIntrospection(document, operationName)
					if (kinds.length === 0) return
					for (const kind of kinds) countIntrospection(kind)
					emit("introspection", requestFacts(context, document), {introspection: kinds})
				})
		},
	}
}

export default useSecuritySignals
