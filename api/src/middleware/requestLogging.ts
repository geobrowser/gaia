/**
 * Request logging middleware.
 *
 * Provides canonical start/end logging for HTTP requests with:
 * - Request ID extraction/generation
 * - Start log with immutable context (method, path, request ID)
 * - End log with outcome (status, duration, error)
 * - OTEL span wrapping for HTTP request context (flows through SentrySpanProcessor)
 */

import {SpanStatusCode, trace} from "@opentelemetry/api"
import * as Sentry from "@sentry/node"
import type {Context, Next} from "hono"
import {detectDbFailureClass} from "../services/dbFailures"
import {log} from "../services/telemetry"
import {extractClientIp} from "../utils/clientIp"

/**
 * HTTP status code used for "client closed request" — matches nginx's
 * `499` convention. The client is already gone, so the status is purely
 * for our own metrics/logs (it never reaches a wire).
 */
const CLIENT_CLOSED_REQUEST_STATUS = 499

/**
 * Detect a client-side connection-close. graphql-yoga's
 * `useExecutionCancellation` propagates the request's AbortSignal as a
 * `DOMException` with `name === "AbortError"` and `code === 20` (ABORT_ERR).
 * Node's stdlib variant uses `code === "ABORT_ERR"`. Treat all of these as
 * "the client hung up" — they are not server faults.
 */
export function isClientAbortError(error: unknown): boolean {
	if (typeof error !== "object" || error === null) return false
	const candidate = error as {name?: unknown; code?: unknown}
	if (candidate.name === "AbortError") return true
	if (candidate.code === 20) return true
	if (candidate.code === "ABORT_ERR") return true
	return false
}

/**
 * Extract or generate a request ID.
 * Checks common headers, falls back to generating a UUID.
 */
function getRequestId(c: Context): string {
	return (
		c.req.header("x-request-id") ||
		c.req.header("x-correlation-id") ||
		c.req.header("traceparent")?.split("-")[1] ||
		crypto.randomUUID()
	)
}

/**
 * Middleware that adds request ID to response headers and context.
 */
export function requestId() {
	return async (c: Context, next: Next) => {
		const id = getRequestId(c)
		c.set("requestId", id)
		c.header("x-request-id", id)
		await next()
	}
}

/**
 * Middleware that provides canonical request logging.
 *
 * Logs:
 * - START: method, path, request ID, query params
 * - END: status, duration, error (if any)
 *
 * Wraps request in OTEL span for HTTP context (flows through SentrySpanProcessor).
 *
 * Also opens a Sentry isolation scope around the request so that every
 * `captureException` / `captureMessage` fired downstream (GraphQL errors,
 * PostGraphile pool-connect failures, `log.error` calls, Effect `logError` /
 * `logFatal`) inherits caller-identity tags — `origin`, `client_ip` — plus
 * `user-agent` in the context. These are the signals you need to filter
 * Sentry Issues by which frontend / IP / script is triggering them.
 */
export function canonicalRequestLogging() {
	return async (c: Context, next: Next) => {
		const requestId = c.get("requestId") || "unknown"
		const method = c.req.method
		const path = c.req.path
		const startTime = Date.now()

		// Caller identity for Sentry alerts. `origin` + `user-agent` are
		// client-settable but help distinguish frontends from ad-hoc scripts.
		// `clientIp` uses the trust rules documented in searchInvocationLogger.
		const headers = c.req.raw.headers
		const origin = headers.get("origin")
		const userAgent = headers.get("user-agent")
		const clientIp = extractClientIp(headers)

		// withIsolationScope gives us a per-request Sentry scope so tags
		// attach to every capture fired inside this handler without bleeding
		// across concurrent requests.
		return Sentry.withIsolationScope(async (scope) => {
			if (origin) scope.setTag("origin", origin)
			if (clientIp) scope.setTag("client_ip", clientIp)
			scope.setContext("request", {
				requestId,
				method,
				path,
				origin,
				clientIp,
				userAgent,
			})

			// Canonical START log
			log.info(`${method} ${path} started`, {
				requestId,
				method,
				path,
				query: c.req.query(),
			})

			// Get tracer lazily at request time (not module load) to ensure OTEL SDK is initialized
			const tracer = trace.getTracer("gaia-api-http")
			const span = tracer.startSpan(`${method} ${path}`, {
				attributes: {
					"http.method": method,
					"http.route": path,
					"http.request_id": requestId,
					...(origin && {"http.request.header.origin": origin}),
				},
			})

			// Store span context for GraphQL plugin (OTEL context doesn't propagate through graphql-yoga)
			c.set("traceContext", {
				traceId: span.spanContext().traceId,
				spanId: span.spanContext().spanId,
				traceFlags: span.spanContext().traceFlags,
			})

			try {
				await next()

				const status = c.res.status
				const duration = Date.now() - startTime
				const graphqlOperationName = c.get("graphqlOperationName") as string | undefined

				span.setAttribute("http.status_code", status)
				span.setAttribute("http.response_time_ms", duration)
				if (graphqlOperationName) {
					span.setAttribute("graphql.operation_name", graphqlOperationName)
				}

				// 499 is "client closed request" — by design not a server fault, so
				// leave the span Unset (OK). Otherwise SentrySpanProcessor would
				// flag the transaction as failed and inflate error-rate metrics
				// alongside genuine 4xx/5xx responses.
				if (status >= 400 && status !== CLIENT_CLOSED_REQUEST_STATUS) {
					span.setStatus({code: SpanStatusCode.ERROR, message: `HTTP ${status}`})
				}

				// Canonical END log — promote to error level for 5xx responses
				// so handlers that return a 5xx without throwing (e.g. c.text("…", 500),
				// GraphQL errors surfaced by graphql-yoga) still produce a Sentry
				// issue.
				//
				// Status 499 is reserved for "client closed request" — set by
				// `app.onError` in main.ts when an AbortError bubbles up. These
				// are not server faults; log as `warn` (breadcrumb only, no Sentry
				// issue) so dashboards stop counting them as 5xx.
				const endLogFields = {
					requestId,
					method,
					path,
					status,
					durationMs: duration,
					...(graphqlOperationName ? {graphqlOperationName} : {}),
				}
				if (status === CLIENT_CLOSED_REQUEST_STATUS) {
					log.warn(`${method} ${path} aborted by client`, endLogFields)
				} else if (status >= 500) {
					log.error(`${method} ${path} returned ${status}`, endLogFields)
				} else {
					log.info(`${method} ${path} completed`, endLogFields)
				}
			} catch (error) {
				const duration = Date.now() - startTime
				const failureClass = detectDbFailureClass(error)

				span.setStatus({code: SpanStatusCode.ERROR, message: "error"})
				span.setAttribute("http.response_time_ms", duration)
				if (failureClass) {
					span.setAttribute("db.failure_class", failureClass)
				}

				// Canonical ERROR log
				log.error(`${method} ${path} failed`, {
					requestId,
					method,
					path,
					durationMs: duration,
					...(failureClass ? {failureClass} : {}),
					error: error instanceof Error ? error.message : String(error),
				})

				throw error
			} finally {
				span.end()
			}
		})
	}
}
