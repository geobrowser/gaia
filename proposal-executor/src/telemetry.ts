/**
 * Telemetry — OTel tracing + Sentry error tracking.
 *
 * Adapted from api/src/services/telemetry.ts for a short-lived CronJob:
 * - No HTTP/GraphQL middleware (batch job, not a server)
 * - Exports flush() for graceful shutdown before process.exit
 * - Falls back to JSON console logging when SENTRY_DSN is not set
 *
 * Architecture: OTel spans → SentrySpanProcessor → Sentry.
 * Effect logs → SentryLogger (errors create issues, info/warn/debug are breadcrumbs).
 */

import {NodeSdk} from "@effect/opentelemetry"
import {trace} from "@opentelemetry/api"
import {resourceFromAttributes} from "@opentelemetry/resources"
import {BasicTracerProvider} from "@opentelemetry/sdk-trace-base"
import {ATTR_SERVICE_NAME} from "@opentelemetry/semantic-conventions"
import * as Sentry from "@sentry/node"
import {SentrySpanProcessor} from "@sentry/opentelemetry"
import {Effect, HashMap, Layer, Logger, LogLevel} from "effect"

const SERVICE_NAME = "proposal-executor"

let sentryInitialized = false
let spanProcessor: SentrySpanProcessor | undefined

// ---------------------------------------------------------------------------
// Sentry + OTel initialization (eager, at module load)
// ---------------------------------------------------------------------------

function initSentry() {
	const dsn = process.env.SENTRY_DSN

	if (!dsn) {
		console.log("[TELEMETRY] Sentry disabled (SENTRY_DSN not set)")
		return
	}

	const environment = process.env.SENTRY_ENVIRONMENT || "production"
	const release = process.env.SENTRY_RELEASE
	const tracesSampleRate = Number.parseFloat(process.env.SENTRY_TRACES_SAMPLE_RATE || "1.0")
	const debug = process.env.SENTRY_DEBUG === "true"

	Sentry.init({
		dsn,
		environment,
		release,
		tracesSampleRate,
		skipOpenTelemetrySetup: true, // We manage OTel setup ourselves
		debug,
	})

	spanProcessor = new SentrySpanProcessor()
	const globalProvider = new BasicTracerProvider({
		resource: resourceFromAttributes({
			[ATTR_SERVICE_NAME]: SERVICE_NAME,
		}),
		spanProcessors: [spanProcessor],
	})
	trace.setGlobalTracerProvider(globalProvider)

	sentryInitialized = true
	console.log(`[TELEMETRY] Sentry enabled (env: ${environment})`)
}

initSentry()

// ---------------------------------------------------------------------------
// Effect NodeSdk layer (OTel spans for Effect fibers)
// ---------------------------------------------------------------------------

const makeTelemetryConfig = Effect.sync(() => ({
	resource: {serviceName: SERVICE_NAME},
	spanProcessor: spanProcessor,
}))

export const NodeSdkLive = NodeSdk.layer(makeTelemetryConfig)

// ---------------------------------------------------------------------------
// Effect Logger → Sentry
// ---------------------------------------------------------------------------

type LogData = Record<string, unknown>

function formatConsoleLog(level: string, message: string, data?: LogData): string {
	return JSON.stringify({level, message, ...data})
}

/** Serialize values for logging — special-cases Error instances */
function serializeValue(value: unknown): unknown {
	if (value instanceof Error) {
		return {name: value.name, message: value.message, stack: value.stack}
	}
	if (typeof value === "object" && value !== null) {
		if (Array.isArray(value)) {
			return value.map(serializeValue)
		}
		const result: Record<string, unknown> = {}
		for (const [k, v] of Object.entries(value)) {
			result[k] = serializeValue(v)
		}
		return result
	}
	return value
}

/**
 * Parse Effect's log message format.
 * Effect wraps log args as ["message", {data}] or just "message".
 */
function parseLogMessage(message: unknown): {messageStr: string; inlineData?: LogData} {
	if (Array.isArray(message)) {
		const [first, second] = message
		const messageStr = typeof first === "string" ? first : String(first)
		if (second && typeof second === "object" && !Array.isArray(second)) {
			return {messageStr, inlineData: serializeValue(second) as LogData}
		}
		return {messageStr}
	}
	return {messageStr: typeof message === "string" ? message : String(message)}
}

function extractAnnotations(annotations: HashMap.HashMap<string, unknown>): LogData | undefined {
	if (HashMap.isEmpty(annotations)) return undefined
	const data: LogData = {}
	for (const [key, value] of annotations) {
		data[key] = serializeValue(value)
	}
	return data
}

/**
 * Effect Logger that routes to Sentry when available:
 * - ERROR/FATAL → Sentry.captureMessage (creates issues)
 * - INFO/WARN/DEBUG → Sentry.addBreadcrumb (context for subsequent errors)
 *
 * Falls back to structured JSON console logging when SENTRY_DSN is not set.
 */
const SentryLogger = Logger.make(({logLevel, message, annotations}) => {
	const {messageStr, inlineData} = parseLogMessage(message)
	const contextData = extractAnnotations(annotations)
	const data = inlineData || contextData ? {...inlineData, ...contextData} : undefined

	if (sentryInitialized) {
		if (logLevel === LogLevel.Error || logLevel === LogLevel.Fatal) {
			Sentry.captureMessage(messageStr, {level: "error", extra: data})
		} else if (logLevel === LogLevel.Warning) {
			Sentry.addBreadcrumb({message: messageStr, data, level: "warning", category: "effect"})
		} else if (logLevel === LogLevel.Info) {
			Sentry.addBreadcrumb({message: messageStr, data, level: "info", category: "effect"})
		} else {
			Sentry.addBreadcrumb({message: messageStr, data, level: "debug", category: "effect"})
		}
	}

	// Always emit to console — structured JSON is our primary log stream,
	// Sentry is supplementary. CronJob pods rely on kubectl logs / log aggregators.
	if (logLevel === LogLevel.Debug || logLevel === LogLevel.Trace) {
		console.debug(formatConsoleLog("debug", messageStr, data))
	} else if (logLevel === LogLevel.Info) {
		console.info(formatConsoleLog("info", messageStr, data))
	} else if (logLevel === LogLevel.Warning) {
		console.warn(formatConsoleLog("warn", messageStr, data))
	} else {
		console.error(formatConsoleLog("error", messageStr, data))
	}
})

export const SentryLoggerLive = Logger.replace(Logger.defaultLogger, SentryLogger)

// ---------------------------------------------------------------------------
// Combined telemetry layer
// ---------------------------------------------------------------------------

/** Provide to Effect.runPromise to get OTel spans + Sentry-aware logging */
export const TelemetryLive = Layer.merge(NodeSdkLive, SentryLoggerLive)

// ---------------------------------------------------------------------------
// Flush — must call before process.exit in short-lived CronJobs
// ---------------------------------------------------------------------------

/**
 * Flush pending Sentry events and OTel spans.
 * Call before process.exit() to avoid losing events in short-lived processes.
 * Timeout prevents hanging if Sentry is unreachable.
 */
export async function flush(timeoutMs = 5000): Promise<void> {
	if (sentryInitialized) {
		await Sentry.flush(timeoutMs)
	}
}
