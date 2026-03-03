/**
 * Telemetry — OTel tracing + Sentry error tracking.
 *
 * Adapted from api/src/services/telemetry.ts for a short-lived CronJob:
 * - No HTTP/GraphQL middleware (batch job, not a server)
 * - Exports flush Effect for graceful shutdown before process.exit
 * - Falls back to JSON console logging when SENTRY_DSN is not set
 *
 * Architecture: OTel spans → SentrySpanProcessor → Sentry.
 * Effect logs → SentryLogger (errors create issues, info/warn/debug are breadcrumbs).
 */

import {NodeSdk} from "@effect/opentelemetry"
import * as Sentry from "@sentry/node"
import {SentrySpanProcessor} from "@sentry/opentelemetry"
import {Duration, Effect, HashMap, Layer, Logger, LogLevel} from "effect"

const SERVICE_NAME = "proposal-executor"

let sentryInitialized = false
let spanProcessor: SentrySpanProcessor | undefined

// ---------------------------------------------------------------------------
// Sentry + OTel initialization (eager, at module load)
// ---------------------------------------------------------------------------

function initSentry() {
	if (sentryInitialized) return

	const dsn = process.env.SENTRY_DSN

	if (!dsn) {
		console.log("[TELEMETRY] Sentry disabled (SENTRY_DSN not set)")
		return
	}

	try {
		const environment = process.env.SENTRY_ENVIRONMENT || "production"
		const release = process.env.SENTRY_RELEASE
		const rawRate = Number.parseFloat(process.env.SENTRY_TRACES_SAMPLE_RATE || "1.0")
		const tracesSampleRate = Number.isNaN(rawRate) || rawRate < 0 || rawRate > 1 ? 1.0 : rawRate
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

		sentryInitialized = true
		console.log(`[TELEMETRY] Sentry enabled (env: ${environment})`)
	} catch (err) {
		console.error(`[TELEMETRY] Sentry init failed, continuing without Sentry: ${err}`)
	}
}

initSentry()

// ---------------------------------------------------------------------------
// Effect NodeSdk layer (OTel spans for Effect fibers)
// ---------------------------------------------------------------------------

const makeTelemetryConfig = Effect.sync(() => ({
	resource: {serviceName: SERVICE_NAME},
	spanProcessor: spanProcessor,
}))

const NodeSdkLive = NodeSdk.layer(makeTelemetryConfig)

// ---------------------------------------------------------------------------
// Effect Logger → Sentry
// ---------------------------------------------------------------------------

type LogData = Record<string, unknown>

function formatConsoleLog(level: string, message: string, data?: LogData): string {
	return JSON.stringify({level, message, ...data})
}

/** Serialize a single value for logging — special-cases Error instances */
function serializeValue(value: unknown): unknown {
	if (value instanceof Error) {
		return {name: value.name, message: value.message, stack: value.stack}
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
			const serialized = serializeValue(second)
			if (serialized && typeof serialized === "object" && !Array.isArray(serialized)) {
				return {messageStr, inlineData: serialized as LogData}
			}
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

// Maps Effect log levels to Sentry breadcrumb level + console method.
// ERROR/FATAL → Sentry.captureMessage (creates issues); others → breadcrumbs.
const LOG_LEVEL_CONFIG: ReadonlyMap<
	LogLevel.LogLevel,
	{
		sentryLevel: "error" | "warning" | "info" | "debug"
		console: "error" | "warn" | "info" | "debug"
		capture: boolean
	}
> = new Map([
	[LogLevel.Fatal, {sentryLevel: "error", console: "error", capture: true}],
	[LogLevel.Error, {sentryLevel: "error", console: "error", capture: true}],
	[LogLevel.Warning, {sentryLevel: "warning", console: "warn", capture: false}],
	[LogLevel.Info, {sentryLevel: "info", console: "info", capture: false}],
	[LogLevel.Debug, {sentryLevel: "debug", console: "debug", capture: false}],
	[LogLevel.Trace, {sentryLevel: "debug", console: "debug", capture: false}],
])

const DEFAULT_LOG_CONFIG = {sentryLevel: "debug" as const, console: "debug" as const, capture: false}

/**
 * Effect Logger that dual-writes to console (primary) and Sentry (supplementary).
 * CronJob pods rely on kubectl logs / log aggregators as the primary observability channel.
 */
const SentryLogger = Logger.make(({logLevel, message, annotations}) => {
	const {messageStr, inlineData} = parseLogMessage(message)
	const contextData = extractAnnotations(annotations)
	const data = inlineData || contextData ? {...inlineData, ...contextData} : undefined
	const config = LOG_LEVEL_CONFIG.get(logLevel) ?? DEFAULT_LOG_CONFIG

	if (sentryInitialized) {
		if (config.capture) {
			Sentry.captureMessage(messageStr, {level: config.sentryLevel, extra: data})
		} else {
			Sentry.addBreadcrumb({message: messageStr, data, level: config.sentryLevel, category: "effect"})
		}
	}

	console[config.console](formatConsoleLog(config.console, messageStr, data))
})

const SentryLoggerLive = Logger.replace(Logger.defaultLogger, SentryLogger)

// ---------------------------------------------------------------------------
// Combined telemetry layer
// ---------------------------------------------------------------------------

/** Provide to Effect.runPromise to get OTel spans + Sentry-aware logging */
export const TelemetryLive = Layer.merge(NodeSdkLive, SentryLoggerLive)

// ---------------------------------------------------------------------------
// Flush — must call before process.exit in short-lived CronJobs
// ---------------------------------------------------------------------------

/**
 * Flush pending Sentry events and OTel spans in parallel.
 * Must be called before process.exit() — short-lived processes lose events otherwise.
 * Each flush is individually ignored so one hanging/failing doesn't block the other.
 */
const flushOTel = spanProcessor
	? Effect.promise(() => {
			const sp = spanProcessor
			return sp ? sp.forceFlush() : Promise.resolve()
		}).pipe(Effect.timeout(Duration.seconds(5)), Effect.ignore)
	: Effect.void

const flushSentry = sentryInitialized ? Effect.promise(() => Sentry.flush(5000)).pipe(Effect.ignore) : Effect.void

export const flush = Effect.all([flushOTel, flushSentry], {concurrency: "unbounded"}).pipe(Effect.asVoid)
