/**
 * Configuration for the GraphQL gateway (stage 4 of the Postgres -> geo-db cutover).
 *
 * GRAPHQL_GATEWAY selects whether the gateway sits in front of PostGraphile:
 * - "off" (default, also any unrecognized value): `/graphql` goes straight to PostGraphile.
 * - "on": the gateway classifies each operation by its root fields, then delegates it to PostGraphile.
 *   No root field is served from geo-db yet, so "on" must not change a response.
 *
 * GRAPHQL_GATEWAY_FIELDS sets a per-root-field mode, `entity=shadow,entities=on`, for the day a field has a
 * geo-db resolver. Modes are "off" (default), "shadow" and "on". A field without a registered resolver is
 * delegated whatever its mode says.
 *
 * Mode "shadow" needs no resolver: PostGraphile answers, and for GRAPHQL_GATEWAY_SHADOW_SAMPLE (a fraction from
 * 0 to 1, default 0.01) of the qualifying operations geo-db is asked the same question in the background and
 * the two answers are compared. The shadow never delays or fails the response.
 *
 * Mode "on" serves the field from geo-db when it has a registered resolver and the operation's only root field
 * is that field. The resolver answers only queries it can answer exactly; anything else (an unsupported query,
 * a geo-db error, or no answer within GRAPHQL_GATEWAY_SERVE_TIMEOUT_MS, default 250) is answered by PostGraphile
 * as before and counted in gaia_api_gateway_serve_fallback_total{field,reason}. A field in mode "on" is not
 * shadowed. With no field in mode "on" (the default) nothing is served from geo-db.
 */

export type GatewayMode = "off" | "on"
export type FieldMode = "off" | "shadow" | "on"

export type GatewayConfig = {
	mode: GatewayMode
	fieldModes: ReadonlyMap<string, FieldMode>
	/** Fraction of qualifying operations a shadow field compares, 0 to 1. */
	shadowSampleRate: number
	/** How long a served field waits for geo-db before falling back to PostGraphile, in milliseconds. */
	serveTimeoutMs: number
}

export const DEFAULT_SHADOW_SAMPLE_RATE = 0.01
export const DEFAULT_SERVE_TIMEOUT_MS = 250
const MAX_SERVE_TIMEOUT_MS = 5_000

/** A whole number of milliseconds in [1, 5000]; anything else is the default. */
export function parseServeTimeoutMs(raw: string | undefined): number {
	if (raw === undefined || raw.trim() === "") return DEFAULT_SERVE_TIMEOUT_MS
	const value = Number(raw)
	return Number.isInteger(value) && value >= 1 && value <= MAX_SERVE_TIMEOUT_MS ? value : DEFAULT_SERVE_TIMEOUT_MS
}

/** A fraction in [0, 1]; anything else (unset, empty, NaN, out of range) is the default. */
export function parseSampleRate(raw: string | undefined): number {
	if (raw === undefined || raw.trim() === "") return DEFAULT_SHADOW_SAMPLE_RATE
	const value = Number(raw)
	return Number.isFinite(value) && value >= 0 && value <= 1 ? value : DEFAULT_SHADOW_SAMPLE_RATE
}

export function parseGatewayMode(raw: string | undefined): GatewayMode {
	return raw?.trim().toLowerCase() === "on" ? "on" : "off"
}

export function parseFieldMode(raw: string | undefined): FieldMode {
	const value = raw?.trim().toLowerCase()
	return value === "on" || value === "shadow" ? value : "off"
}

/** Parses `field=mode,field=mode`. An entry without a valid field name is dropped; an unknown mode is "off". */
export function parseFieldModes(raw: string | undefined): Map<string, FieldMode> {
	const modes = new Map<string, FieldMode>()
	for (const entry of (raw ?? "").split(",")) {
		const [field, mode] = entry.split("=")
		const name = field?.trim()
		if (name && /^[_A-Za-z][_0-9A-Za-z]*$/.test(name)) {
			modes.set(name, parseFieldMode(mode))
		}
	}
	return modes
}

export function readGatewayConfig(env: Record<string, string | undefined>): GatewayConfig {
	return {
		mode: parseGatewayMode(env.GRAPHQL_GATEWAY),
		fieldModes: parseFieldModes(env.GRAPHQL_GATEWAY_FIELDS),
		shadowSampleRate: parseSampleRate(env.GRAPHQL_GATEWAY_SHADOW_SAMPLE),
		serveTimeoutMs: parseServeTimeoutMs(env.GRAPHQL_GATEWAY_SERVE_TIMEOUT_MS),
	}
}
