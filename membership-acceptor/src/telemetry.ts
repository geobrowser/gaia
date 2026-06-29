/**
 * Telemetry — Sentry error tracking + structured console logging.
 *
 * Adapted from proposal-executor/src/telemetry.ts, but without the Effect-TS
 * runtime: membership-acceptor is a long-running HTTP server, so the request
 * path is plain async code rather than Effect fibers. We keep the same
 * observability conventions (console JSON as the primary channel, Sentry as a
 * supplementary one) so logs read the same across the two services.
 *
 * - console is the primary channel (kubectl logs / log aggregators).
 * - Sentry is supplementary: error/fatal create issues, lower levels become breadcrumbs.
 * - Falls back to console-only when SENTRY_DSN is not set.
 */

import * as Sentry from "@sentry/node"

const SERVICE_NAME = "membership-acceptor"

let sentryInitialized = false

// ---------------------------------------------------------------------------
// Sentry initialization (eager, at module load)
// ---------------------------------------------------------------------------

function initSentry() {
	const dsn = process.env.SENTRY_DSN

	if (!dsn) {
		console.log(JSON.stringify({level: "info", message: "[TELEMETRY] Sentry disabled (SENTRY_DSN not set)"}))
		return
	}

	try {
		const environment = process.env.SENTRY_ENVIRONMENT || "production"
		const release = process.env.SENTRY_RELEASE
		const rawRate = Number.parseFloat(process.env.SENTRY_TRACES_SAMPLE_RATE || "1.0")
		const tracesSampleRate = Number.isNaN(rawRate) || rawRate < 0 || rawRate > 1 ? 1.0 : rawRate
		const debug = process.env.SENTRY_DEBUG === "true"

		Sentry.init({dsn, environment, release, tracesSampleRate, debug})

		sentryInitialized = true
		console.log(JSON.stringify({level: "info", message: `[TELEMETRY] Sentry enabled (env: ${environment})`}))
	} catch (err) {
		console.error(`[TELEMETRY] Sentry init failed, continuing without Sentry: ${err}`)
	}
}

initSentry()

// ---------------------------------------------------------------------------
// Structured logger — dual-writes console (primary) + Sentry (supplementary)
// ---------------------------------------------------------------------------

type LogData = Record<string, unknown>

type Level = "debug" | "info" | "warn" | "error"

const SENTRY_LEVEL: Record<Level, Sentry.SeverityLevel> = {
	debug: "debug",
	info: "info",
	warn: "warning",
	error: "error",
}

/** Serialize a value for logging — special-cases Error instances. */
function serializeValue(value: unknown): unknown {
	if (value instanceof Error) {
		return {name: value.name, message: value.message, stack: value.stack}
	}
	return value
}

function serializeData(data?: LogData): LogData | undefined {
	if (!data) return undefined
	const out: LogData = {}
	for (const key of Object.keys(data)) {
		out[key] = serializeValue(data[key])
	}
	return out
}

function emit(level: Level, message: string, data?: LogData): void {
	const serialized = serializeData(data)

	if (sentryInitialized) {
		if (level === "error") {
			Sentry.captureMessage(message, {level: SENTRY_LEVEL[level], extra: serialized})
		} else {
			Sentry.addBreadcrumb({message, data: serialized, level: SENTRY_LEVEL[level], category: SERVICE_NAME})
		}
	}

	const consoleMethod = level === "warn" ? "warn" : level === "error" ? "error" : "log"
	console[consoleMethod](JSON.stringify({level, message, ...serialized}))
}

export const log = {
	debug: (message: string, data?: LogData) => emit("debug", message, data),
	info: (message: string, data?: LogData) => emit("info", message, data),
	warn: (message: string, data?: LogData) => emit("warn", message, data),
	error: (message: string, data?: LogData) => emit("error", message, data),
}

// ---------------------------------------------------------------------------
// Flush — call before process exit so buffered Sentry events are not lost
// ---------------------------------------------------------------------------

export async function flush(timeoutMs = 5000): Promise<void> {
	if (!sentryInitialized) return
	try {
		await Sentry.flush(timeoutMs)
	} catch {
		// best-effort — never block shutdown on telemetry
	}
}
