/**
 * Request logging middleware.
 *
 * Provides canonical start/end logging for HTTP requests with:
 * - Request ID extraction/generation
 * - Start log with immutable context (method, path, request ID)
 * - End log with outcome (status, duration, error)
 * - OTEL span wrapping for HTTP request context (flows through SentrySpanProcessor)
 * - Distributed tracing via W3C traceparent and Sentry trace headers
 */

import {SpanStatusCode, trace, context, SpanKind} from "@opentelemetry/api"
import {W3CTraceContextPropagator} from "@opentelemetry/core"
import type {Context, Next} from "hono"
import {log} from "../services/telemetry"

const propagator = new W3CTraceContextPropagator()


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
 * Carrier to extract trace context from Hono request headers.
 */
function createHeaderCarrier(c: Context) {
	return {
		get(key: string): string | undefined {
			return c.req.header(key) ?? undefined
		},
		keys(): string[] {
			return [...c.req.raw.headers.keys()]
		},
	}
}

/**
 * Parse sentry-trace header into W3C traceparent format.
 * sentry-trace format: <trace_id>-<span_id>-<sampled>
 * traceparent format: 00-<trace_id>-<span_id>-<flags>
 */
function sentryTraceToTraceparent(sentryTrace: string): string | null {
	const parts = sentryTrace.split("-")
	if (parts.length < 2) return null

	const [traceId, spanId, sampled] = parts
	// Ensure trace ID is 32 chars (pad if necessary)
	const normalizedTraceId = traceId.padStart(32, "0")
	// Ensure span ID is 16 chars
	const normalizedSpanId = spanId.padStart(16, "0")
	// Convert sampled to flags (01 = sampled, 00 = not sampled)
	const flags = sampled === "1" ? "01" : "00"

	return `00-${normalizedTraceId}-${normalizedSpanId}-${flags}`
}

/**
 * Middleware that provides canonical request logging.
 *
 * Logs:
 * - START: method, path, request ID, query params
 * - END: status, duration, error (if any)
 *
 * Wraps request in OTEL span for HTTP context (flows through SentrySpanProcessor).
 * Supports distributed tracing via W3C traceparent or Sentry trace headers.
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

		// Extract parent trace context from incoming headers
		// First try W3C traceparent, then fall back to sentry-trace
		let parentContext = context.active()
		const traceparent = c.req.header("traceparent")
		const sentryTrace = c.req.header("sentry-trace")

		if (traceparent) {
			// Use W3C propagator to extract context
			const carrier = createHeaderCarrier(c)
			parentContext = propagator.extract(context.active(), carrier, {
				get: (carrier, key) => carrier.get(key),
				keys: (carrier) => carrier.keys(),
			})
		} else if (sentryTrace) {
			// Convert sentry-trace to traceparent and extract
			const convertedTraceparent = sentryTraceToTraceparent(sentryTrace)
			if (convertedTraceparent) {
				const syntheticCarrier = {
					get: (key: string) => (key === "traceparent" ? convertedTraceparent : undefined),
					keys: () => ["traceparent"],
				}
				parentContext = propagator.extract(context.active(), syntheticCarrier, {
					get: (carrier, key) => carrier.get(key),
					keys: (carrier) => carrier.keys(),
				})
			}
		}

		// Get tracer lazily at request time (not module load) to ensure OTEL SDK is initialized
		const tracer = trace.getTracer("gaia-api-http")

		// Start span as a child of the extracted parent context
		const span = tracer.startSpan(
			`${method} ${path}`,
			{
				kind: SpanKind.SERVER,
				attributes: {
					"http.method": method,
					"http.route": path,
					"http.request_id": requestId,
				},
			},
			parentContext,
		)

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

			span.setAttribute("http.status_code", status)
			span.setAttribute("http.response_time_ms", duration)

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
			})
		} catch (error) {
			const duration = Date.now() - startTime

			span.setStatus({code: SpanStatusCode.ERROR, message: "error"})
			span.setAttribute("http.response_time_ms", duration)

			// Canonical ERROR log
			log.error(`${method} ${path} failed`, {
				requestId,
				method,
				path,
				durationMs: duration,
				error: error instanceof Error ? error.message : String(error),
			})

			throw error
		} finally {
			span.end()
		}
	}
}
