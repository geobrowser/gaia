/**
 * Request logging middleware.
 *
 * Provides canonical start/end logging for HTTP requests with:
 * - Request ID extraction/generation
 * - Start log with immutable context (method, path, request ID)
 * - End log with outcome (status, duration, error)
 * - Sentry span wrapping for HTTP request context
 */

import * as Sentry from "@sentry/node"
import type {Context, Next} from "hono"
import {log} from "../services/telemetry"

/**
 * Paths to skip canonical logging (they have their own instrumentation).
 */
const SKIP_PATHS = new Set(["/graphql", "/v2/graphql"])

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
 * Wraps request in Sentry span for HTTP context.
 * Skips /graphql endpoints (they have operation-level instrumentation).
 */
export function canonicalRequestLogging() {
	return async (c: Context, next: Next) => {
		const path = c.req.path

		// Skip GraphQL - it has its own operation-level instrumentation
		if (SKIP_PATHS.has(path)) {
			await next()
			return
		}

		const requestId = c.get("requestId") || "unknown"
		const method = c.req.method
		const startTime = Date.now()

		// Canonical START log
		log.info(`${method} ${path} started`, {
			requestId,
			method,
			path,
			query: c.req.query(),
		})

		// Wrap in Sentry span for HTTP request context
		await Sentry.startSpan(
			{
				name: `${method} ${path}`,
				op: "http.server",
				attributes: {
					"http.method": method,
					"http.route": path,
					"http.request_id": requestId,
				},
			},
			async (span) => {
				try {
					await next()

					const status = c.res.status
					const duration = Date.now() - startTime

					// Add response attributes to span
					span.setAttribute("http.status_code", status)
					span.setAttribute("http.response_time_ms", duration)

					if (status >= 400) {
						span.setStatus({code: 2, message: `HTTP ${status}`})
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

					span.setStatus({code: 2, message: "error"})
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
				}
			},
		)
	}
}
