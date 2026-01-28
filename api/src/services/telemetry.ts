import {NodeSdk} from "@effect/opentelemetry"
import {trace} from "@opentelemetry/api"
import {resourceFromAttributes} from "@opentelemetry/resources"
import {ATTR_SERVICE_NAME} from "@opentelemetry/semantic-conventions"
import {BasicTracerProvider} from "@opentelemetry/sdk-trace-base"
import * as Sentry from "@sentry/node"
import {SentrySpanProcessor} from "@sentry/opentelemetry"
import {Effect, Layer, Logger, LogLevel} from "effect"

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
		} else {
			console.warn(formatConsoleLog("warn", message, data))
		}
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

// Effect Logger that routes to Sentry
// - DEBUG/TRACE/INFO/WARN: breadcrumbs (context for errors, no issues created)
// - ERROR/FATAL: captureMessage (creates Sentry issues)
const SentryLogger = Logger.make(({logLevel, message}) => {
	const messageStr = String(message)

	if (sentryInitialized) {
		if (logLevel === LogLevel.Error || logLevel === LogLevel.Fatal) {
			Sentry.captureMessage(messageStr, {level: "error"})
		} else if (logLevel === LogLevel.Warning) {
			Sentry.addBreadcrumb({message: messageStr, level: "warning", category: "effect"})
		} else if (logLevel === LogLevel.Info) {
			Sentry.addBreadcrumb({message: messageStr, level: "info", category: "effect"})
		} else {
			Sentry.addBreadcrumb({message: messageStr, level: "debug", category: "effect"})
		}
	} else {
		if (logLevel === LogLevel.Debug || logLevel === LogLevel.Trace) {
			console.debug(formatConsoleLog("debug", messageStr))
		} else if (logLevel === LogLevel.Info) {
			console.info(formatConsoleLog("info", messageStr))
		} else if (logLevel === LogLevel.Warning) {
			console.warn(formatConsoleLog("warn", messageStr))
		} else {
			console.error(formatConsoleLog("error", messageStr))
		}
	}
})

// Layer that replaces the default Effect logger with our Sentry-aware logger
export const SentryLoggerLive = Logger.replace(Logger.defaultLogger, SentryLogger)

// Combined telemetry layer: spans + logging
export const TelemetryLive = Layer.merge(NodeSdkLive, SentryLoggerLive)
