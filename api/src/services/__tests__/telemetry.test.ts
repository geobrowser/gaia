import {ROOT_CONTEXT, TraceFlags, trace} from "@opentelemetry/api"
import {resourceFromAttributes} from "@opentelemetry/resources"
import {BasicTracerProvider, type Sampler} from "@opentelemetry/sdk-trace-base"
import {ATTR_SERVICE_NAME} from "@opentelemetry/semantic-conventions"
import * as Sentry from "@sentry/node"
import {SentrySampler, SentrySpanProcessor} from "@sentry/opentelemetry"
import {afterEach, describe, expect, it} from "vitest"

/**
 * These tests pin the fix: `SENTRY_TRACES_SAMPLE_RATE` only takes effect
 * because telemetry.ts installs `SentrySampler` on its hand-built
 * `BasicTracerProvider`. With `skipOpenTelemetrySetup: true`, Sentry does NOT
 * install its sampler for us, and `SentrySpanProcessor` exports every span it
 * receives — so without an explicit sampler the OTEL default (AlwaysOnSampler)
 * records everything and the rate is silently ignored. We assert at the OTEL
 * sampling-decision boundary: a sampled span is recording (TraceFlags.SAMPLED),
 * a dropped one is not. That decision is what gates the SentrySpanProcessor.
 */

// A no-op transport so init never touches the network.
function noopTransport() {
	return {
		send: () => Promise.resolve({}),
		flush: () => Promise.resolve(true),
	}
}

function initSentryWithRate(tracesSampleRate: number) {
	Sentry.init({
		dsn: "https://abc123@o1.ingest.sentry.io/1",
		tracesSampleRate,
		skipOpenTelemetrySetup: true,
		transport: noopTransport,
	})
	const client = Sentry.getClient()
	if (!client) throw new Error("Sentry client not initialized")
	return client
}

// Mirror the production wiring from telemetry.ts: SentrySampler + SentrySpanProcessor
// on a BasicTracerProvider. Optionally omit the sampler to reproduce the original bug.
function makeTracer(sampler: Sampler | undefined) {
	const provider = new BasicTracerProvider({
		resource: resourceFromAttributes({[ATTR_SERVICE_NAME]: "gaia-api-test"}),
		...(sampler ? {sampler} : {}),
		spanProcessors: [new SentrySpanProcessor()],
	})
	return provider.getTracer("test")
}

afterEach(async () => {
	await Sentry.flush(0)
	await Sentry.close(0)
})

describe("telemetry trace sampling", () => {
	it("drops root spans when SENTRY_TRACES_SAMPLE_RATE is 0", () => {
		const client = initSentryWithRate(0)
		const tracer = makeTracer(new SentrySampler(client))

		const span = tracer.startSpan("GET /graphql")
		expect(span.isRecording()).toBe(false)
		expect(span.spanContext().traceFlags).toBe(TraceFlags.NONE)
		span.end()
	})

	it("keeps root spans when SENTRY_TRACES_SAMPLE_RATE is 1.0", () => {
		const client = initSentryWithRate(1.0)
		const tracer = makeTracer(new SentrySampler(client))

		const span = tracer.startSpan("GET /graphql")
		expect(span.isRecording()).toBe(true)
		expect(span.spanContext().traceFlags).toBe(TraceFlags.SAMPLED)
		span.end()
	})

	it("child spans inherit the parent's sampling decision (rate 1.0 parent → kept child)", () => {
		const client = initSentryWithRate(1.0)
		const tracer = makeTracer(new SentrySampler(client))

		// Mirror instrumentationPlugin.ts: the GraphQL op span is parented to the
		// HTTP root span via a manually-rebuilt context (OTEL async context does
		// not propagate through graphql-yoga). The child must inherit the root's
		// decision so all spans of a request are kept or dropped together.
		const parent = tracer.startSpan("GET /graphql")
		const parentContext = trace.setSpanContext(ROOT_CONTEXT, {
			...parent.spanContext(),
			isRemote: false,
		})
		const child = tracer.startSpan("graphql query spaces", {}, parentContext)

		expect(parent.isRecording()).toBe(true)
		expect(child.isRecording()).toBe(true)
		child.end()
		parent.end()
	})

	it("REGRESSION GUARD: without SentrySampler the rate is ignored (records at rate 0)", () => {
		initSentryWithRate(0)
		// No sampler → OTEL default AlwaysOnSampler → every span recorded regardless
		// of tracesSampleRate. This is the bug the SentrySampler change fixes.
		const tracer = makeTracer(undefined)

		const span = tracer.startSpan("GET /graphql")
		expect(span.isRecording()).toBe(true)
		span.end()
	})
})
