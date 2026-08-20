import {type DocumentNode, GraphQLError, print} from "graphql"
import type {Plugin} from "graphql-yoga"
import {graphqlQueryFingerprint} from "../services/queryFingerprint"
import {log} from "../services/telemetry"
import {extractClientIp} from "../utils/clientIp"

/**
 * Bounds how many bytes of GraphQL response one request is allowed to
 * materialise, and refuses the response rather than dying while serialising it.
 *
 * Why this exists
 * ---------------
 * On 2026-08-19 one client sent ~19 requests/min of a single query shape that
 * allocated ~1.5 GB each against a 2 GiB pod limit, killing 5 api pods. The
 * incident was closed by fixing the caller (geo-explorers/postgres_to_geo#36),
 * which required write access to another team's repository. That does not
 * generalise: the api has to be able to refuse what it cannot serve.
 *
 * What the canary measurements rule out (isolated pod, one request each, cgroup
 * sampled every 200 ms):
 *
 *   root `first` x nested `relations` | response  | duration | peak RSS delta
 *   ----------------------------------|-----------|----------|---------------
 *   50 x 1000                         |   3.05 MB |    6.6 s |   +152 Mi
 *   100 x 100                         |   5.89 MB |    5.8 s |   +260 Mi
 *   500 x 100                         |  29.0  MB |   18.1 s |   +724 Mi
 *   1000 x 1000                       |  62.9  MB |   33.3 s | +1,569 Mi
 *
 * - **Not a static budget over declared limits.** `50 x 1000` and `500 x 100`
 *   are the same 50k product but cost 152 Mi vs 724 Mi, because declared limits
 *   are upper bounds and only *realized* rows are paid for. A walker over the
 *   query document (which is what `computeQueryCost` is) over-rejects cheap
 *   queries and still misses expensive ones.
 * - **Not the cost ceiling.** `GRAPHQL_COST_REJECT_THRESHOLD` is armed at 500;
 *   the killer scored 274 and the 08-06 incident scored 228. A ceiling low
 *   enough to catch 274 also catches ~2-3% of ordinary traffic (<=260 is 96.8%
 *   of requests). Cost counts nodes; the OOM is driven by bytes hydrated per
 *   node.
 * - **Not concurrency.** See `admissionControl.ts`: the pod cap moved 10 -> 6 ->
 *   10, and 6 doubled 503s to 1414/h with no OOM reduction, because this is a
 *   single-request kill.
 * - **Not a memory-limit bump.** 4 of 8 pods share one node with 6.5 Gi
 *   allocatable, so 3 Gi limits risk node-level eviction — a worse failure mode.
 *
 * The only axis the data supports is realized bytes, counted at runtime, with
 * an abort. That is this plugin.
 *
 * Where the counting happens, and why here
 * ----------------------------------------
 * The open question was whether to count per-connection as each connection
 * resolves (aborting before the whole response exists) or over the finished
 * result. This counts over the finished result, deliberately:
 *
 * PostGraphile v4 builds **one** SQL query per root field, with nested
 * collections expressed as lateral joins returning nested JSON. By the time any
 * *nested* connection's resolver runs, the root field's SQL has already
 * returned and its whole subtree is hydrated in memory. Counting rows as nested
 * connections resolve therefore cannot pre-empt the allocation — it observes an
 * allocation that has already happened. The only shape where per-field
 * accounting genuinely prevents work is a multi-root document (`q1: entities
 * ... q2: entities ...`), where aborting after the first over-budget root stops
 * the remaining roots from being hydrated. That is a Phase 2 refinement worth
 * making only if the Phase 1 logs show over-budget requests are multi-root; the
 * 2026-08-19 killer was single-root, where it would have bought nothing.
 *
 * What counting here *does* prevent is the amplification on top of the hydrated
 * graph: yoga's `JSON.stringify` of the response, the copy ioredis makes if the
 * response is cacheable, and (before #901) a second stringify purely to measure
 * the response. Peak RSS for the 1000x1000 shape was ~25x its 62.9 MB response,
 * so bounding the string is a large fraction of the fix — but it is honestly
 * *not* all of it, and the remainder needs streaming serialization, which is a
 * separate and larger change.
 *
 * Phase 1 / Phase 2
 * -----------------
 * Same convention as `costLoggerPlugin`: land the accounting observational,
 * confirm against real traffic what a ceiling would have rejected, then enforce.
 *
 * - `GRAPHQL_RESPONSE_BYTE_LOG_THRESHOLD` (default 10 MB) — warns, always on.
 *   Every warning carries the fingerprint, caller identity and measured bytes,
 *   so "what would a ceiling of N have rejected" is a log query, not a guess.
 * - `GRAPHQL_RESPONSE_BYTE_CEILING` (default **0 = off**) — enforcement. Phase 2
 *   is setting this, not shipping more code.
 *
 * Sizing, from the 18 h window that contained the incident: a 10-15 MB ceiling
 * would have fired on ~78 requests (~4/h) while bounding peak RSS to roughly
 * 300-750 Mi.
 */

/** Parse an env var that is allowed to be 0 (meaning "disabled"). */
function parseByteEnv(name: string, fallback: number): number {
	const raw = process.env[name]
	if (raw === undefined) return fallback
	const parsed = Number.parseInt(raw, 10)
	if (!Number.isFinite(parsed) || parsed < 0) return fallback
	return parsed
}

/** Warn at/above this many measured response bytes. Observational, always on. */
export const RESPONSE_BYTE_LOG_THRESHOLD = parseByteEnv("GRAPHQL_RESPONSE_BYTE_LOG_THRESHOLD", 10_000_000)

/**
 * Refuse the response at/above this many measured bytes. `0` disables
 * enforcement entirely, which is the shipped default — see the Phase 1 / Phase 2
 * note above.
 */
export const RESPONSE_BYTE_CEILING = parseByteEnv("GRAPHQL_RESPONSE_BYTE_CEILING", 0)

/**
 * Bound on the measuring walk itself, so observation can never become unbounded
 * work on an unbounded response. Above this the measurement reports a lower
 * bound rather than walking further. Set above the largest response ever
 * observed (62.9 MB) so Phase 1 numbers are exact in practice.
 */
export const RESPONSE_BYTE_WALK_LIMIT = parseByteEnv("GRAPHQL_RESPONSE_BYTE_WALK_LIMIT", 64_000_000)

/**
 * The limit actually handed to the walker. When enforcing, stopping at the
 * ceiling is both sufficient (we only need to know we are over it) and cheaper —
 * the 62.9 MB response is abandoned after ~16% of its nodes at a 10 MB ceiling.
 */
function effectiveWalkLimit(): number {
	return RESPONSE_BYTE_CEILING > 0 ? RESPONSE_BYTE_CEILING : RESPONSE_BYTE_WALK_LIMIT
}

/**
 * Context key under which the per-request measurement is shared with
 * `useGraphQLInstrumentation`, so the response is walked once per request rather
 * than once per plugin that wants its size.
 */
export const GRAPHQL_RESPONSE_BYTES_CONTEXT_KEY = "graphqlResponseBytes" as const

export type GraphqlResponseBytesContext = {
	[GRAPHQL_RESPONSE_BYTES_CONTEXT_KEY]?: ByteMeasurement
}

export type ByteMeasurement = {
	/**
	 * Bytes counted. When `exceeded` is true the walk stopped early, so this is
	 * a LOWER bound on the real size, not the size.
	 */
	bytes: number
	/** True if the walk passed `limit` and stopped short. */
	exceeded: boolean
}

// ---------------------------------------------------------------------------
// Budget-limited JSON byte walk
// ---------------------------------------------------------------------------

/**
 * Approximate the serialized JSON byte size of a value WITHOUT building the
 * string, stopping as soon as the running total passes `limit`.
 *
 * This replaced `JSON.stringify(data).length`. The problem was never the call
 * itself but where it ran: behind a `durationMs >= 1000` guard, which in this
 * service selects *exactly* the pathological responses. A 62.9 MB response was
 * therefore serialized twice — once by yoga to answer the request, once purely
 * to measure it — and stringify costs ~2.4x the response size in RSS. So the
 * instrumentation added hundreds of MiB to precisely the requests most likely to
 * OOM the pod. Measuring a failure must not help cause it.
 *
 * This walks the structure instead: O(nodes visited) CPU, O(depth) stack, and no
 * allocation proportional to the payload. `limit` bounds the CPU too, which is
 * what makes it safe to run on every response rather than only slow ones.
 *
 * The result is an ESTIMATE and deliberately so:
 *   - string escaping is not scanned for, so payloads full of quotes/newlines
 *     read slightly small;
 *   - lengths are UTF-16 code units, so non-ASCII text reads small against real
 *     UTF-8 bytes;
 *   - `undefined` / functions are skipped, matching what JSON.stringify omits.
 *
 * Every one of those biases is downward, which is the safe direction for a
 * ceiling: a response the estimate calls oversized is certainly oversized. It is
 * fine for a histogram bucketed by powers of ten and for a byte ceiling. Do not
 * use it where an exact Content-Length matters.
 */
export function measureJsonBytes(value: unknown, limit: number): ByteMeasurement {
	let total = 0
	let stopped = false

	/**
	 * Single place the limit is compared, so every exit point agrees. Latches
	 * `stopped` on the way out: once the walk is abandoned mid-structure the
	 * total is a lower bound, and callers have to be able to tell.
	 */
	function overLimit(): boolean {
		if (total > limit) stopped = true
		return stopped
	}

	function visit(v: unknown): void {
		if (overLimit()) return

		switch (typeof v) {
			case "undefined":
			case "function":
				return
			case "boolean":
				total += v ? 4 : 5
				return
			case "number":
				if (!Number.isFinite(v)) {
					total += 4 // JSON.stringify emits null
					return
				}
				// Avoid allocating a string per number on million-node payloads.
				if (Number.isInteger(v) && Math.abs(v) < 1e15) {
					const digits = v === 0 ? 1 : Math.floor(Math.log10(Math.abs(v))) + 1
					total += digits + (v < 0 ? 1 : 0)
					return
				}
				total += String(v).length
				return
			case "bigint":
				total += String(v).length
				return
			case "string":
				total += v.length + 2 // surrounding quotes
				return
			case "object":
				break
			default:
				return
		}

		if (v === null) {
			total += 4
			return
		}

		if (Array.isArray(v)) {
			// brackets + commas
			total += 2 + (v.length > 0 ? v.length - 1 : 0)
			for (const item of v) {
				if (overLimit()) return
				// `undefined` inside an array serializes as `null`, not omitted.
				if (item === undefined) total += 4
				else visit(item)
			}
			return
		}

		// `toJSON` (Date, etc.) — respect it rather than walking internals.
		const maybeToJson = (v as {toJSON?: unknown}).toJSON
		if (typeof maybeToJson === "function") {
			visit((v as {toJSON: () => unknown}).toJSON())
			return
		}

		// `for...in` + hasOwn rather than Object.entries/keys: those allocate an
		// array (of pairs, no less) for EVERY object visited, which on a
		// million-node payload costs more than the string this function exists to
		// avoid. Measured: entries() ran 2x slower than JSON.stringify with a
		// larger RSS delta.
		const record = v as Record<string, unknown>
		total += 2 // braces
		let first = true
		for (const key in record) {
			if (overLimit()) return
			if (!Object.hasOwn(record, key)) continue // stringify only emits own props
			const entry = record[key]
			if (entry === undefined || typeof entry === "function") continue // omitted by JSON.stringify
			if (!first) total += 1 // comma
			first = false
			total += key.length + 3 // "key":
			visit(entry)
		}
	}

	visit(value)
	// Final check as well as the in-walk ones: a scalar payload never re-enters
	// `visit`, so nothing would have compared it against the limit otherwise.
	return {bytes: total, exceeded: overLimit()}
}

/**
 * Unbounded variant: the full estimate, no early exit. For callers that want the
 * number whatever the size — instrumentationPlugin's fallback when no shared
 * measurement is on the context — and for the tests that pin agreement with
 * `JSON.stringify().length`.
 */
export function estimateJsonBytes(value: unknown): number {
	return measureJsonBytes(value, Number.POSITIVE_INFINITY).bytes
}

// ---------------------------------------------------------------------------
// Prometheus metrics
// ---------------------------------------------------------------------------
// Exposed via /health/metrics.
//
// Distinct from `gaia_api_graphql_response_size_bytes` in instrumentationPlugin,
// which only records responses that took >= 1s and is therefore biased away from
// small/fast ones. This histogram covers EVERY response, because the walk that
// produces it is now cheap enough to run unconditionally — which is what makes
// it usable for choosing a ceiling.
const RESPONSE_BYTE_BUCKET_EDGES: readonly number[] = [
	10_000, // 10 KB
	100_000, // 100 KB
	500_000, // 500 KB
	1_000_000, // 1 MB
	2_000_000, // 2 MB
	5_000_000, // 5 MB
	10_000_000, // 10 MB — low end of the candidate ceiling range
	15_000_000, // 15 MB — high end of the candidate ceiling range
	25_000_000, // 25 MB
	50_000_000, // 50 MB
	75_000_000, // 75 MB
]

const responseByteBucketCounts = new Map<number, number>()
let responseByteTotalCount = 0
let responseByteTotalSum = 0
let responseBudgetExceededCount = 0
let responseBudgetRefusedCount = 0

function recordResponseBytes(bytes: number): void {
	if (!Number.isFinite(bytes) || bytes < 0) return
	responseByteTotalCount++
	responseByteTotalSum += bytes
	for (const edge of RESPONSE_BYTE_BUCKET_EDGES) {
		if (bytes <= edge) {
			responseByteBucketCounts.set(edge, (responseByteBucketCounts.get(edge) ?? 0) + 1)
		}
	}
}

/**
 * Render the response-byte histogram and budget counters in Prometheus text
 * format.
 *
 * `_exceeded_total` counts responses over the log threshold whether or not
 * enforcement is armed; `_refused_total` counts the subset actually refused. In
 * Phase 1 (`GRAPHQL_RESPONSE_BYTE_CEILING=0`) the first climbs and the second
 * stays flat, which is precisely the "what would we have rejected" signal.
 */
export function renderResponseByteMetrics(): string {
	const lines = [
		"# HELP gaia_api_graphql_response_bytes Estimated serialized GraphQL response size in bytes (all responses, unlike gaia_api_graphql_response_size_bytes which samples only >=1s queries).",
		"# TYPE gaia_api_graphql_response_bytes histogram",
	]
	for (const edge of RESPONSE_BYTE_BUCKET_EDGES) {
		lines.push(`gaia_api_graphql_response_bytes_bucket{le="${edge}"} ${responseByteBucketCounts.get(edge) ?? 0}`)
	}
	lines.push(`gaia_api_graphql_response_bytes_bucket{le="+Inf"} ${responseByteTotalCount}`)
	lines.push(`gaia_api_graphql_response_bytes_sum ${responseByteTotalSum}`)
	lines.push(`gaia_api_graphql_response_bytes_count ${responseByteTotalCount}`)
	lines.push(
		"# HELP gaia_api_graphql_response_budget_exceeded_total Responses whose estimated size reached GRAPHQL_RESPONSE_BYTE_LOG_THRESHOLD.",
		"# TYPE gaia_api_graphql_response_budget_exceeded_total counter",
		`gaia_api_graphql_response_budget_exceeded_total ${responseBudgetExceededCount}`,
		"# HELP gaia_api_graphql_response_budget_refused_total Responses refused because they reached GRAPHQL_RESPONSE_BYTE_CEILING.",
		"# TYPE gaia_api_graphql_response_budget_refused_total counter",
		`gaia_api_graphql_response_budget_refused_total ${responseBudgetRefusedCount}`,
	)
	return `${lines.join("\n")}\n`
}

/** Reset accumulated metric state. Tests only. */
export function __resetResponseByteMetricsForTests(): void {
	responseByteBucketCounts.clear()
	responseByteTotalCount = 0
	responseByteTotalSum = 0
	responseBudgetExceededCount = 0
	responseBudgetRefusedCount = 0
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

function getOperationLabel(args: {operationName?: string | null}): string {
	return args.operationName ?? "anonymous"
}

/**
 * Build the error that replaces an over-budget response.
 *
 * `BAD_USER_INPUT` rather than a bespoke code, for two reasons that both come
 * from existing behaviour: `errorMasking.shouldUnmaskError` only passes
 * `BAD_USER_INPUT` / `SERVICE_UNAVAILABLE` through to the client, so any other
 * code would reach the caller as "Unexpected error."; and
 * `instrumentationPlugin.isClientError` uses the same set to decide what not to
 * report to Sentry, so any other code would open a Sentry issue per refusal.
 * A client asking for more than the server can serialize is a client-input
 * problem, and the refusal is already logged here with full caller identity.
 */
function budgetError(measured: ByteMeasurement, ceiling: number): GraphQLError {
	const measuredDescription = measured.exceeded ? `at least ${measured.bytes}` : `${measured.bytes}`
	return new GraphQLError(
		`Response size (${measuredDescription} bytes) exceeds the maximum of ${ceiling} bytes. ` +
			"Request fewer rows (`first`) or fewer nested collections, and paginate.",
		{
			extensions: {
				code: "BAD_USER_INPUT",
				responseSizeBytes: measured.bytes,
				responseSizeBytesIsLowerBound: measured.exceeded,
				maxResponseSizeBytes: ceiling,
				http: {status: 400},
			},
		},
	)
}

/**
 * Yoga plugin that measures every response and, when armed, refuses the ones
 * over budget.
 *
 * Ordering (see `sharedPlugins` in postgraphile.ts): this must run after the
 * response-cache plugin's `onExecuteDone` and before the instrumentation
 * plugin's.
 *
 * - After the cache, because the cache plugin injects `__responseCacheId` /
 *   `__typename` metadata fields into every selection set and strips them again
 *   in its own `onExecuteDone`. Measuring before that strip would count fields
 *   that are never sent to the client, inflating the number the ceiling is
 *   compared against — by a lot on a wide response. `valkeyCache` does its own
 *   pre-stringify size check, so an over-budget response is not serialized into
 *   the cache on the way past.
 * - Before instrumentation, so the measurement can be shared instead of the
 *   response being walked twice, and so a refusal replaces the result before
 *   instrumentation logs a "Large GraphQL response" for a response that was
 *   never sent.
 *
 * Failure mode: a bug in here must never break a request that would otherwise
 * have succeeded, so the measurement runs inside a try/catch and a refusal is
 * only ever raised from a value the measurement actually produced.
 */
export function useResponseBudget(): Plugin {
	return {
		onExecute({args}) {
			const startedAtMs = Date.now()

			return {
				onExecuteDone({result, setResult}) {
					// Held outside the try so the shadow-mode catch below cannot
					// swallow a deliberate refusal. The catch exists to stop
					// *plugin bugs* breaking a request; it must not also suppress
					// enforcement. Mirrors useCostLogger.
					let refusal: GraphQLError | null = null
					try {
						// Incremental delivery (@defer / @stream) hands us an async
						// iterable rather than a result. The api does not use it,
						// and measuring a stream is a different problem (each chunk
						// is small; the sum is what matters), so leave it alone
						// rather than half-handle it.
						if (!result || typeof result !== "object" || Symbol.asyncIterator in result) return

						const data = "data" in result ? result.data : undefined
						if (data === undefined || data === null) return

						let measured: ByteMeasurement
						try {
							measured = measureJsonBytes(data, effectiveWalkLimit())
						} catch (error) {
							log.warn("[response-budget] measurement failed", {
								error: error instanceof Error ? error.message : String(error),
								operationName: args.operationName ?? undefined,
							})
							return
						}

						recordResponseBytes(measured.bytes)

						// Share with useGraphQLInstrumentation so the response is
						// walked once per request, not once per interested plugin.
						const ctx = args.contextValue as GraphqlResponseBytesContext | undefined
						if (ctx && typeof ctx === "object") {
							ctx[GRAPHQL_RESPONSE_BYTES_CONTEXT_KEY] = measured
						}

						const overCeiling = RESPONSE_BYTE_CEILING > 0 && measured.bytes >= RESPONSE_BYTE_CEILING
						const overLogThreshold =
							RESPONSE_BYTE_LOG_THRESHOLD > 0 && measured.bytes >= RESPONSE_BYTE_LOG_THRESHOLD

						if (!overCeiling && !overLogThreshold) return

						responseBudgetExceededCount++
						if (overCeiling) {
							responseBudgetRefusedCount++
							refusal = budgetError(measured, RESPONSE_BYTE_CEILING)
						}

						const headers = (args.contextValue as {request?: Request} | undefined)?.request?.headers
						const query = args.document ? printDocumentSafely(args.document) : undefined
						log.warn(
							overCeiling
								? "GraphQL response refused: byte ceiling"
								: "GraphQL response over byte budget",
							{
								responseSizeBytes: measured.bytes,
								responseSizeBytesIsLowerBound: measured.exceeded,
								responseSizeMB: Math.round((measured.bytes / 1_000_000) * 100) / 100,
								logThreshold: RESPONSE_BYTE_LOG_THRESHOLD,
								ceiling: RESPONSE_BYTE_CEILING,
								enforced: overCeiling,
								durationMs: Date.now() - startedAtMs,
								operationName: getOperationLabel(args),
								...(query !== undefined && {queryFingerprint: graphqlQueryFingerprint(query), query}),
								variables: args.variableValues,
								origin: headers?.get("origin") ?? null,
								userAgent: headers?.get("user-agent") ?? null,
								clientIp: headers ? extractClientIp(headers) : null,
							},
						)
					} catch (error) {
						// Shadow-mode guarantee: never break a request over a bug in
						// here. log.warn itself can throw (a circular value in
						// `variables` defeats its JSON.stringify), which is why the
						// warning is inside the try and the refusal is not.
						try {
							log.warn("[response-budget] plugin onExecuteDone threw", {
								error: error instanceof Error ? error.message : String(error),
								operationName: args.operationName ?? undefined,
							})
						} catch {
							// Nothing we can do; keep the request flowing.
						}
					}

					// Outside the catch on purpose. A plugin bug must not change the
					// response; a response over the ceiling must. Dropping `data`
					// entirely rather than nulling fields inside it is the point —
					// this payload is never serialized, and a partial one is no use
					// to the caller either.
					if (refusal) setResult({errors: [refusal]})
				},
			}
		},
	}
}

/**
 * `print` on a document a plugin mangled can throw. The response has already
 * been measured by the time we want to log it, so losing the query text is
 * strictly better than losing the warning.
 */
function printDocumentSafely(document: DocumentNode): string | undefined {
	try {
		return print(document)
	} catch {
		return undefined
	}
}
