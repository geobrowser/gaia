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
 */

export type GatewayMode = "off" | "on"
export type FieldMode = "off" | "shadow" | "on"

export type GatewayConfig = {
	mode: GatewayMode
	fieldModes: ReadonlyMap<string, FieldMode>
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
	}
}
