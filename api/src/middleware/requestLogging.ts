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
import type {Context, Next} from "hono"
import {detectDbFailureClass} from "../services/dbFailures"
import {log} from "../services/telemetry"

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
 */
export function canonicalRequestLogging() {
	return async (c: Context, next: Next) => {
		const requestId = c.get("requestId") || "unknown"
		const method = c.req.method
		const path = c.req.path
		const startTime = Date.now()

		// Canonical START log
		log.info(`${method} ${path} started`, {
			requestId,
			method,
			path,
			query: c.req.query(),
		})

		// Get tracer lazily at request time (not module load) to ensure OTEL SDK is initialized
		const tracer = trace.getTracer("gaia-api-http")
		const origin = c.req.header("origin")
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

			if (status >= 400) {
				span.setStatus({code: SpanStatusCode.ERROR, message: `HTTP ${status}`})
			}

			// Canonical END log
			log.info(`${method} ${path} completed`, {
				requestId,
				method,
				path,
				status,
				durationMs: duration,
				...(graphqlOperationName ? {graphqlOperationName} : {}),
			})
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
	}
}
