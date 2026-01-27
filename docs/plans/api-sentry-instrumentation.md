# API Sentry Instrumentation Plan

**Status:** Ready to Implement

## Problem

The API currently exports telemetry to Axiom.co via OTLP, but:
1. No GraphQL-specific instrumentation
2. Inconsistent with other services (kg-indexer uses Sentry)
3. Limited error tracking
4. Axiom adds cost/complexity

## Goals

1. Replace Axiom with Sentry via `SentrySpanProcessor`
2. Keep Effect's OpenTelemetry layer pattern
3. Route ALL telemetry to Sentry (spans, logs, errors)
4. Fallback to console only when `SENTRY_DSN` not set

## Key Decisions

### Effect + Sentry Integration
- Use `@sentry/opentelemetry`'s `SentrySpanProcessor` with Effect's `NodeSdk.layer()`
- `Sentry.init()` with `skipOpenTelemetrySetup: true` - let Effect manage OTEL
- Use Effect's `Config` module for env vars (not `process.env` directly)

### Logging Strategy
- Spans ARE the structured events - no redundant console logging
- Create unified `log` utility for non-Effect code
- Create `SentryLoggerLive` layer for `Effect.log()` calls
- Debug/trace → breadcrumbs, info/warn/error → `captureMessage()`

### GraphQL Instrumentation
- Use `Sentry.startSpan()` directly (GraphQL-Yoga is callback-based, not Effect)
- Capture errors via `Sentry.captureException()` with tags/extras
- No console logging in the plugin - span data is sufficient

## Implementation

### 1. Add Dependencies

```bash
cd api
bun add @sentry/node @sentry/opentelemetry
bun remove @opentelemetry/exporter-trace-otlp-proto
```

### 2. Rewrite `api/src/services/telemetry.ts`

```typescript
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
```

### 3. Create `api/src/kg/instrumentationPlugin.ts`

```typescript
import * as Sentry from "@sentry/node"
import {print} from "graphql"
import type {Plugin} from "graphql-yoga"

export function useGraphQLInstrumentation(): Plugin {
	return {
		onExecute({args}) {
			const operationName = args.operationName || "anonymous"
			const query = print(args.document)

			// Wrap GraphQL execution in a Sentry span
			return Sentry.startSpan(
				{
					name: `graphql ${operationName}`,
					op: "graphql.execute",
					attributes: {
						"graphql.operation_name": operationName,
						"graphql.document": query.slice(0, 2000),
					},
				},
				(span) => {
					return {
						onExecuteDone({result}) {
							const errors = "errors" in result ? result.errors : undefined
							const hasErrors = errors && errors.length > 0

							if (hasErrors) {
								span.setStatus({code: 2, message: "error"})
								span.setAttribute("graphql.error_count", errors.length)

								for (const error of errors) {
									Sentry.captureException(error.originalError || error, {
										tags: {"graphql.operation_name": operationName},
										extra: {
											query: query.slice(0, 2000),
											path: error.path,
										},
									})
								}
							}
						},
					}
				},
			)
		},
	}
}
```

### 4. Update `api/src/kg/postgraphile.ts`

Add import at top:
```typescript
import {useGraphQLInstrumentation} from "./instrumentationPlugin"
```

Add to sharedPlugins array:
```typescript
const sharedPlugins = [
	useExecutionCancellation(),
	useResponseCache({
		session: () => null,
		ttl: 10_000,
	}),
	useGraphQLInstrumentation(),  // Add this
]
```

### 5. Update `api/src/services/environment.ts`

Remove `telemetryToken` from:
- `IEnvironment` type
- `make` Effect (remove Config.option for TELEMETRY_TOKEN)
- Return object

### 6. Update `api/main.ts`

Replace import:
```typescript
// Old:
import { NodeSdkLive } from "./src/services/telemetry"
// New:
import { log, TelemetryLive } from "./src/services/telemetry"
```

Replace all `NodeSdkLive` → `TelemetryLive`

Replace all console calls:
```typescript
// Old:
console.log(`[SEARCH] Search routes enabled...`)
console.error(`[SPACE][deploy] Failed...`)

// New:
log.info("Search routes enabled", {url: opensearchUrl})
log.error("Failed to deploy space", {route: "/deploy", error: error.message})
```

### 7. Update Other Files

**`api/src/services/storage/storage.ts`:**
```typescript
import {log} from "../telemetry"
// ...
_pool.on("error", (err) => {
	log.error("PostgreSQL pool error", {error: String(err)})
})
```

**`api/src/services/ipfs.ts`:**
```typescript
// Replace console.log in Schedule.tapInput with Effect.logInfo
Schedule.tapInput(() => Effect.logInfo("[IPFS][upload] Retrying"))
```

**`api/src/space/deploy-dao.ts`:**
```typescript
import {log} from "../services/telemetry"
// Rename local `log` variable to `daoLog` to avoid conflict
// Replace console.error with log.error
```

**`api/src/search/index.ts`:**
```typescript
import {log} from "../services/telemetry"
// Replace console.error with log.error
```

**`api/src/versioned/router.ts`:**
```typescript
import {log} from "../services/telemetry"
// Replace console.error with log.error
```

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `SENTRY_DSN` | Sentry DSN (required for Sentry) | - |
| `SENTRY_ENVIRONMENT` | Environment name | `development` |
| `SENTRY_RELEASE` | Release version | - |
| `SENTRY_TRACES_SAMPLE_RATE` | Trace sampling (0.0-1.0) | `1.0` |
| `SENTRY_DEBUG` | Enable debug logging | `false` |

**Remove:** `TELEMETRY_TOKEN`

## Telemetry Flow

```
┌─────────────────────────────────────────────────────────┐
│                    Sentry.init()                        │
│              (global singleton client)                  │
└─────────────────────────────────────────────────────────┘
         ▲                ▲                 ▲
         │                │                 │
┌────────┴────────┐ ┌─────┴─────┐ ┌────────┴────────┐
│ SentrySpanProcessor │ │ log utility │ │ SentryLogger    │
│ (OTEL → Sentry)     │ │ (non-Effect) │ │ (Effect.log)    │
└─────────────────────┘ └─────────────┘ └─────────────────┘
         ▲                ▲                 ▲
         │                │                 │
┌────────┴────────┐ ┌─────┴─────┐ ┌────────┴────────┐
│ Effect.withSpan()   │ │ Hono routes │ │ Effect code     │
│ GraphQL plugin      │ │ Error handlers│ │                 │
└─────────────────────┘ └─────────────┘ └─────────────────┘
```

## Verification

After implementation:
1. `bun run check` - TypeScript passes
2. `bun run lint` - No new lint errors in modified files
3. Test locally with `SENTRY_DSN` unset - should see console output
4. Test with `SENTRY_DSN` set - should see Sentry events
