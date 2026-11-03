/**
 * Configuration for the opt-in geo-db entity-card read (stage 1 of the Postgres -> geo-db cutover).
 *
 * GEODB_ENTITY_CARDS selects the mode:
 * - "off" (default, also any unrecognized value): the route is not mounted.
 * - "on": answer from geo-db; on any geo-db error or timeout fall back to Postgres for that request.
 * - "shadow": answer from Postgres, also ask geo-db, and log a structured diff when they differ.
 */

export type GeoDbCardMode = "off" | "on" | "shadow"

export type GeoDbCardConfig = {
	mode: GeoDbCardMode
	/** Path of the `geodb-client` binary. */
	clientBin: string
	/** Client key file (`--key`). */
	keyPath: string
	/** Binding file (`--binding`) naming the geo-db server's listener. */
	bindingPath: string
	/** JSON file mapping a gaia space id to its geo-db space id: `{"<space>": {"geo": "<id>"}}`. */
	spaceMapPath: string
	/** Per-request budget for the geo-db read, in milliseconds. */
	timeoutMs: number
	/** Long-lived `geodb-client pipe` workers, each one connection and one request at a time. */
	poolSize: number
}

const DEFAULT_TIMEOUT_MS = 500
const DEFAULT_POOL_SIZE = 4
const MAX_POOL_SIZE = 64

export function parseMode(raw: string | undefined): GeoDbCardMode {
	const value = raw?.trim().toLowerCase()
	return value === "on" || value === "shadow" ? value : "off"
}

export function readGeoDbCardConfig(env: Record<string, string | undefined>): GeoDbCardConfig {
	const timeout = Number(env.GEODB_TIMEOUT_MS)
	const pool = Math.floor(Number(env.GEODB_POOL_SIZE))
	return {
		mode: parseMode(env.GEODB_ENTITY_CARDS),
		clientBin: env.GEODB_CLIENT_BIN ?? "geodb-client",
		keyPath: env.GEODB_KEY_PATH ?? "",
		bindingPath: env.GEODB_BINDING_PATH ?? "",
		spaceMapPath: env.GEODB_SPACE_MAP_PATH ?? "",
		timeoutMs: Number.isFinite(timeout) && timeout > 0 ? timeout : DEFAULT_TIMEOUT_MS,
		poolSize: Number.isFinite(pool) && pool > 0 ? Math.min(pool, MAX_POOL_SIZE) : DEFAULT_POOL_SIZE,
	}
}
