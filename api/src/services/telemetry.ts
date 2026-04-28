import {NodeSdk} from "@effect/opentelemetry"
import {trace} from "@opentelemetry/api"
import {resourceFromAttributes} from "@opentelemetry/resources"
import {BasicTracerProvider} from "@opentelemetry/sdk-trace-base"
import {ATTR_SERVICE_NAME} from "@opentelemetry/semantic-conventions"
import * as Sentry from "@sentry/node"
import {SentrySpanProcessor} from "@sentry/opentelemetry"
import {Effect, HashMap, Layer, Logger, LogLevel} from "effect"

const SERVICE_NAME = "gaia-api"

// Track whether Sentry is initialized
let sentryInitialized = false
let spanProcessor: SentrySpanProcessor | undefined

// Initialize Sentry and OTEL eagerly at module load time
// This ensures tracing is ready before any requests arrive
function initSentry() {
	const dsn = process.env.SENTRY_DSN

	if (!dsn) {
		console.log("[TELEMETRY] Sentry disabled (SENTRY_DSN not set)")
		return
	}

	const environment = process.env.SENTRY_ENVIRONMENT || "development"
	const release = process.env.SENTRY_RELEASE
	const tracesSampleRate = parseFloat(process.env.SENTRY_TRACES_SAMPLE_RATE || "1.0")
	const debug = process.env.SENTRY_DEBUG === "true"

	Sentry.init({
		dsn,
		environment,
		release,
		tracesSampleRate,
		skipOpenTelemetrySetup: true, // We manage OTEL setup ourselves
		debug,
		// Drop client-disconnect events. graphql-yoga's response stream throws a
		// DOMException (name="AbortError", code=20) when the client closes the
		// socket mid-write; Sentry's auto-capture of unhandled rejections would
		// otherwise create issues for what isn't a server fault. Synchronous
		// AbortErrors are also handled at the HTTP layer via `app.onError` →
		// 499; this filter is the safety net for the async path.
		beforeSend: (event, hint) => {
			const err = hint?.originalException as {name?: unknown; code?: unknown} | undefined
			if (err && (err.name === "AbortError" || err.code === 20 || err.code === "ABORT_ERR")) {
				return null
			}
			return event
		},
	})

	// Set up a minimal global TracerProvider for non-Effect code (GraphQL, HTTP middleware).
	// We intentionally skip SentryContextManager to avoid conflicts with Effect's Fiber-based context.
	// Effect has its own scoped provider; this global provider is only for trace.getTracer() calls.
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

// Initialize immediately when module is loaded
initSentry()

// Effect layer config - reuses the already-initialized Sentry
const makeTelemetryConfig = Effect.sync(() => ({
	resource: {serviceName: SERVICE_NAME},
	spanProcessor: spanProcessor,
}))

// Effect layer with SentrySpanProcessor
export const NodeSdkLive = NodeSdk.layer(makeTelemetryConfig)

// Logger that routes to Sentry when initialized, falls back to console
type LogData = Record<string, unknown>

function formatConsoleLog(level: string, message: string, data?: LogData): string {
	return JSON.stringify({level, message, ...data})
}

export const log = {
	debug: (message: string, data?: LogData) => {
		if (sentryInitialized) {
			Sentry.addBreadcrumb({message, data, level: "debug", category: "log"})
		} else {
			console.debug(formatConsoleLog("debug", message, data))
		}
	},
	info: (message: string, data?: LogData) => {
		if (sentryInitialized) {
			// Breadcrumb only - provides context for subsequent errors without creating issues
			Sentry.addBreadcrumb({message, data, level: "info", category: "log"})
		} else {
			console.info(formatConsoleLog("info", message, data))
		}
	},
	warn: (message: string, data?: LogData) => {
		if (sentryInitialized) {
			// Breadcrumb only - provides context for subsequent errors without creating issues
			Sentry.addBreadcrumb({message, data, level: "warning", category: "log"})
		}
		// Always write to stdout so warnings are visible in kubectl logs
		console.warn(formatConsoleLog("warn", message, data))
	},
	error: (message: string, data?: LogData) => {
		if (sentryInitialized) {
			// Only errors create Sentry issues
			Sentry.captureMessage(message, {level: "error", extra: data})
		} else {
			console.error(formatConsoleLog("error", message, data))
		}
	},
}

// Serialize a value for logging, with special handling for errors
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

// Parse Effect's log message format and extract structured data
// Effect's log functions like logError("msg", {data}) produce a tuple [string, object]
// We extract the string as message and merge the object into annotations
function parseLogMessage(message: unknown): {messageStr: string; inlineData?: LogData} {
	// Effect wraps log args in an array: ["message", {data}] or just "message"
	if (Array.isArray(message)) {
		const [first, second] = message
		const messageStr = typeof first === "string" ? first : String(first)
		if (second && typeof second === "object" && !Array.isArray(second)) {
			// Serialize inline data, handling errors and nested objects
			return {messageStr, inlineData: serializeValue(second) as LogData}
		}
		return {messageStr}
	}
	return {messageStr: typeof message === "string" ? message : String(message)}
}

// Extract annotations from Effect logger context into a plain object
function extractAnnotations(annotations: HashMap.HashMap<string, unknown>): LogData | undefined {
	if (HashMap.isEmpty(annotations)) return undefined
	const data: LogData = {}
	for (const [key, value] of annotations) {
		data[key] = serializeValue(value)
	}
	return data
}

// Effect Logger that routes to Sentry
// - DEBUG/TRACE/INFO/WARN: breadcrumbs (context for errors, no issues created)
// - ERROR/FATAL: captureMessage (creates Sentry issues)
const SentryLogger = Logger.make(({logLevel, message, annotations}) => {
	const {messageStr, inlineData} = parseLogMessage(message)
	const contextData = extractAnnotations(annotations)
	// Merge inline data (from log call) with context annotations (from annotateLogs)
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
	} else {
		if (logLevel === LogLevel.Debug || logLevel === LogLevel.Trace) {
			console.debug(formatConsoleLog("debug", messageStr, data))
		} else if (logLevel === LogLevel.Info) {
			console.info(formatConsoleLog("info", messageStr, data))
		} else if (logLevel === LogLevel.Warning) {
			console.warn(formatConsoleLog("warn", messageStr, data))
		} else {
			console.error(formatConsoleLog("error", messageStr, data))
		}
	}
})

// Layer that replaces the default Effect logger with our Sentry-aware logger
export const SentryLoggerLive = Logger.replace(Logger.defaultLogger, SentryLogger)

// Combined telemetry layer: spans + logging
export const TelemetryLive = Layer.merge(NodeSdkLive, SentryLoggerLive)
