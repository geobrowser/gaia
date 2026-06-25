/**
 * Configuration — parsed and validated once at startup.
 *
 * Fails fast (throws) on missing/invalid required values so the container
 * crash-loops loudly rather than silently accepting every (unverifiable) webhook.
 */

export interface AcceptorConfig {
	/** Port the HTTP server listens on. */
	port: number
	/**
	 * Shared secret used to verify the `X-Geo-Signature` HMAC on incoming webhooks.
	 * Must match the `secret` column of this service's `app_webhooks` row.
	 */
	webhookSecret: string
}

export class ConfigError extends Error {
	override readonly name = "ConfigError"
}

const DEFAULT_PORT = 8080

/**
 * Parse config from the given environment (defaults to `process.env`).
 * Throws {@link ConfigError} with an actionable message on the first problem.
 */
export function parseConfig(env: NodeJS.ProcessEnv = process.env): AcceptorConfig {
	const webhookSecret = env.GEO_WEBHOOK_SECRET?.trim()
	if (!webhookSecret) {
		throw new ConfigError(
			"GEO_WEBHOOK_SECRET is required — it must equal the `secret` of this service's app_webhooks row",
		)
	}

	const rawPort = env.PORT?.trim()
	let port = DEFAULT_PORT
	if (rawPort) {
		const parsed = Number(rawPort)
		if (!Number.isInteger(parsed) || parsed < 1 || parsed > 65535) {
			throw new ConfigError(`Invalid PORT: expected an integer in 1..65535, got "${rawPort}"`)
		}
		port = parsed
	}

	return {port, webhookSecret}
}
