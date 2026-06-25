/**
 * Configuration — parsed and validated once at startup.
 *
 * Fails fast (throws) on missing/invalid required values so the container
 * crash-loops loudly rather than silently mis-voting or accepting unverifiable
 * webhooks.
 */

import type {SupportedChainId} from "./contracts.js"

export interface AppConfig {
	/** Port the HTTP server listens on. */
	port: number
	/**
	 * Shared secret used to verify the `X-Geo-Signature` HMAC on incoming webhooks.
	 * Must match the `secret` column of this service's `app_webhooks` row.
	 */
	webhookSecret: string

	// --- Voting identity / chain ---
	/** Signing key for the Safe smart account that owns the acceptor's personal space. */
	acceptorPrivateKey: `0x${string}`
	/** The acceptor's personal-space id (bytes16 hex) — the enter() `_fromSpaceId`. */
	acceptorSpaceId: `0x${string}`
	/** SpaceRegistry contract address. */
	spaceRegistryAddress: `0x${string}`
	/** Chain RPC endpoint (may contain an API key). */
	rpcUrl: string
	/** Pimlico bundler/paymaster API key (gas sponsorship). */
	pimlicoApiKey: string
	/** 80451 (mainnet) or 19411 (testnet). */
	chainId: SupportedChainId
	/** Geo GraphQL endpoint — used by policies (e.g. the editor check). */
	graphqlEndpoint: string

	/**
	 * Spaces this acceptor auto-accepts (DAO space UUIDs, lowercased). A request
	 * for any other space is ignored. Empty ⇒ accept none (effective kill switch).
	 */
	autoacceptSpaceIds: ReadonlySet<string>
}

export class ConfigError extends Error {
	override readonly name = "ConfigError"
}

const DEFAULT_PORT = 8080
const PRIVATE_KEY_RE = /^0x[0-9a-fA-F]{64}$/
const BYTES16_RE = /^0x[0-9a-fA-F]{32}$/
const ADDRESS_RE = /^0x[0-9a-fA-F]{40}$/

function required(env: NodeJS.ProcessEnv, key: string): string {
	const value = env[key]?.trim()
	if (!value) throw new ConfigError(`${key} is required`)
	return value
}

/**
 * Parse config from the given environment (defaults to `process.env`).
 * Throws {@link ConfigError} with an actionable message on the first problem.
 */
export function parseConfig(env: NodeJS.ProcessEnv = process.env): AppConfig {
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

	// Accept the private key with or without the 0x prefix.
	let acceptorPrivateKey = required(env, "ACCEPTOR_PRIVATE_KEY")
	if (!acceptorPrivateKey.startsWith("0x")) acceptorPrivateKey = `0x${acceptorPrivateKey}`
	if (!PRIVATE_KEY_RE.test(acceptorPrivateKey)) {
		throw new ConfigError("Invalid ACCEPTOR_PRIVATE_KEY: expected a 32-byte hex key (0x + 64 hex chars)")
	}

	const acceptorSpaceId = required(env, "ACCEPTOR_SPACE_ID")
	if (!BYTES16_RE.test(acceptorSpaceId)) {
		throw new ConfigError("Invalid ACCEPTOR_SPACE_ID: expected bytes16 hex (0x + 32 hex chars)")
	}

	const spaceRegistryAddress = required(env, "SPACE_REGISTRY_ADDRESS")
	if (!ADDRESS_RE.test(spaceRegistryAddress)) {
		throw new ConfigError("Invalid SPACE_REGISTRY_ADDRESS: expected an address (0x + 40 hex chars)")
	}

	const rpcUrl = required(env, "RPC_URL")
	const pimlicoApiKey = required(env, "PIMLICO_API_KEY")
	const graphqlEndpoint = required(env, "GRAPHQL_ENDPOINT")

	const rawChainId = env.CHAIN_ID?.trim() || "80451"
	const chainId = Number(rawChainId)
	if (chainId !== 80451 && chainId !== 19411) {
		throw new ConfigError(`Invalid CHAIN_ID: expected 80451 (mainnet) or 19411 (testnet), got "${rawChainId}"`)
	}

	const autoacceptSpaceIds = new Set(
		(env.MEMBERSHIP_AUTOACCEPT_SPACE_IDS ?? "")
			.split(",")
			.map((s) => s.trim().toLowerCase())
			.filter((s) => s.length > 0),
	)

	return {
		port,
		webhookSecret,
		acceptorPrivateKey: acceptorPrivateKey as `0x${string}`,
		acceptorSpaceId: acceptorSpaceId as `0x${string}`,
		spaceRegistryAddress: spaceRegistryAddress as `0x${string}`,
		rpcUrl,
		pimlicoApiKey,
		chainId: chainId as SupportedChainId,
		graphqlEndpoint,
		autoacceptSpaceIds,
	}
}
