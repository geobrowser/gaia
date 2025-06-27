import {NodeSdk} from "@effect/opentelemetry"
import {OTLPTraceExporter} from "@opentelemetry/exporter-trace-otlp-proto"
import {BatchSpanProcessor} from "@opentelemetry/sdk-trace-base"
import {Redacted} from "effect"
import {EnvironmentLive} from "./environment"

const exporter = EnvironmentLive.telemetryToken
	? new OTLPTraceExporter({
			url: "https://api.axiom.co/v1/traces", // Axiom API endpoint for trace data
			headers: {
				Authorization: Redacted.value(EnvironmentLive.telemetryToken),
				"X-Axiom-Dataset": "gaia.api",
			},
		})
	: undefined

// Set up tracing with the OpenTelemetry SDK
export const NodeSdkLive = NodeSdk.layer(() => ({
	resource: {serviceName: "gaia.api"},
	spanProcessor: exporter ? new BatchSpanProcessor(exporter) : undefined,
}))
