import {NodeSdk} from "@effect/opentelemetry"
import {trace} from "@opentelemetry/api"
import {resourceFromAttributes} from "@opentelemetry/resources"
import {ATTR_SERVICE_NAME} from "@opentelemetry/semantic-conventions"
import {BasicTracerProvider} from "@opentelemetry/sdk-trace-base"
import * as Sentry from "@sentry/node"
import {SentrySpanProcessor} from "@sentry/opentelemetry"
import {Config, Effect, Layer, Logger, LogLevel, Option} from "effect"

const SERVICE_NAME = "gaia-api"

// Track whether Sentry is initialized
let sentryInitialized = false

// Sentry configuration read via Effect Config
const makeTelemetryConfig = Effect.gen(function* () {
	const maybeDsn = yield* Config.option(Config.string("SENTRY_DSN"))
	const dsn = Option.getOrNull(maybeDsn)

	if (!dsn) {
		console.log("[TELEMETRY] Sentry disabled (SENTRY_DSN not set)")
		return {
			resource: {serviceName: SERVICE_NAME},
			spanProcessor: undefined,
		}
	}

	const maybeEnvironment = yield* Config.option(Config.string("SENTRY_ENVIRONMENT"))
	const maybeRelease = yield* Config.option(Config.string("SENTRY_RELEASE"))
	const maybeSampleRate = yield* Config.option(Config.string("SENTRY_TRACES_SAMPLE_RATE"))
	const maybeDebug = yield* Config.option(Config.boolean("SENTRY_DEBUG"))

	const environment = Option.getOrElse(maybeEnvironment, () => "development")
	const release = Option.getOrUndefined(maybeRelease)
	const tracesSampleRate = parseFloat(Option.getOrElse(maybeSampleRate, () => "1.0"))
	const debug = Option.getOrElse(maybeDebug, () => false)

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
	const globalProvider = new BasicTracerProvider({
		resource: resourceFromAttributes({
			[ATTR_SERVICE_NAME]: SERVICE_NAME,
		}),
		spanProcessors: [new SentrySpanProcessor()],
	})
	trace.setGlobalTracerProvider(globalProvider)

	sentryInitialized = true
	console.log(`[TELEMETRY] Sentry enabled (env: ${environment})`)

	return {
		resource: {serviceName: SERVICE_NAME},
		spanProcessor: new SentrySpanProcessor(),
	}
})

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
