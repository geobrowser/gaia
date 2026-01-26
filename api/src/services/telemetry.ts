import {NodeSdk} from "@effect/opentelemetry"
import * as Sentry from "@sentry/node"
import {SentrySpanProcessor} from "@sentry/opentelemetry"
import {Config, Effect, Layer, Logger, LogLevel, Option} from "effect"

// Track whether Sentry is initialized
let sentryInitialized = false

// Sentry configuration read via Effect Config
const makeTelemetryConfig = Effect.gen(function* () {
	const maybeDsn = yield* Config.option(Config.string("SENTRY_DSN"))
	const dsn = Option.getOrNull(maybeDsn)

	if (!dsn) {
		console.log("[TELEMETRY] Sentry disabled (SENTRY_DSN not set)")
		return {
			resource: {serviceName: "gaia.api"},
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
		skipOpenTelemetrySetup: true, // We use Effect's OTEL layer
		debug,
	})

	sentryInitialized = true
	console.log(`[TELEMETRY] Sentry enabled (env: ${environment})`)

	return {
		resource: {serviceName: "gaia.api"},
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
			Sentry.captureMessage(message, {level: "info", extra: data})
		} else {
			console.info(formatConsoleLog("info", message, data))
		}
	},
	warn: (message: string, data?: LogData) => {
		if (sentryInitialized) {
			Sentry.captureMessage(message, {level: "warning", extra: data})
		} else {
			console.warn(formatConsoleLog("warn", message, data))
		}
	},
	error: (message: string, data?: LogData) => {
		if (sentryInitialized) {
			Sentry.captureMessage(message, {level: "error", extra: data})
		} else {
			console.error(formatConsoleLog("error", message, data))
		}
	},
}

// Effect Logger that routes to Sentry
const SentryLogger = Logger.make(({logLevel, message}) => {
	const messageStr = String(message)

	if (sentryInitialized) {
		if (logLevel === LogLevel.Debug || logLevel === LogLevel.Trace) {
			Sentry.addBreadcrumb({message: messageStr, level: "debug", category: "effect"})
		} else if (logLevel === LogLevel.Info) {
			Sentry.captureMessage(messageStr, {level: "info"})
		} else if (logLevel === LogLevel.Warning) {
			Sentry.captureMessage(messageStr, {level: "warning"})
		} else if (logLevel === LogLevel.Error || logLevel === LogLevel.Fatal) {
			Sentry.captureMessage(messageStr, {level: "error"})
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

export {Sentry}
