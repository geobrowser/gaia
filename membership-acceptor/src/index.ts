/**
 * Membership Acceptor — entry point.
 *
 * A long-running HTTP server that receives notification webhooks from the Geo
 * notification service. This is the real-time successor to the proposal-executor
 * cron's membership-accept path.
 *
 * Verifies webhook signatures, detects membership requests, and casts the YES
 * vote on-chain via the acceptor's smart wallet.
 */

import {parseConfig} from "./config.js"
import {sanitizeError} from "./contracts.js"
import {createGraphQLClient} from "./graphql.js"
import {composePolicies, editorPolicy} from "./policy.js"
import {createApp} from "./server.js"
import {flush, log} from "./telemetry.js"
import {createAcceptor} from "./vote.js"
import {createSmartWallet} from "./wallet.js"

async function main() {
	let config: ReturnType<typeof parseConfig>
	try {
		config = parseConfig()
	} catch (err) {
		// Misconfiguration — crash loudly so the deployment is visibly broken.
		log.error("configuration error — refusing to start", {error: sanitizeError(err)})
		// Flush buffered telemetry before exiting, or the startup-failure event is
		// lost (same reason the graceful-shutdown path flushes before exit).
		await flush()
		process.exit(1)
	}

	if (config.autoacceptSpaceIds.size === 0) {
		// Not fatal, but the acceptor will ignore every request — make it obvious.
		log.warn("MEMBERSHIP_AUTOACCEPT_SPACE_IDS is empty — no requests will be accepted")
	}

	let acceptor: ReturnType<typeof createAcceptor>
	try {
		const wallet = await createSmartWallet({
			privateKey: config.acceptorPrivateKey,
			pimlicoApiKey: config.pimlicoApiKey,
			rpcUrl: config.rpcUrl,
			chainId: config.chainId,
			zerodevSponsorshipRpcUrl: config.zerodevSponsorshipRpcUrl,
		})
		// On 55516 this address is the EOA itself (EIP-7702), not a Safe proxy — it
		// is what must own ACCEPTOR_SPACE_ID, so log it either way.
		log.info("acceptor wallet ready", {
			account_address: wallet.safeAddress,
			chain_id: config.chainId,
			sponsorship: config.chainId === 55516 ? "zerodev-kernel-7702" : "safe-pimlico",
		})
		const graphql = createGraphQLClient({endpoint: config.graphqlEndpoint})
		// The editor check is the reference policy; space-defined policies compose here.
		const policy = composePolicies(editorPolicy)
		acceptor = createAcceptor({
			wallet,
			acceptorSpaceId: config.acceptorSpaceId,
			spaceRegistryAddress: config.spaceRegistryAddress,
			allowlist: config.autoacceptSpaceIds,
			policy,
			graphql,
		})
	} catch (err) {
		// Sanitize: wallet/bundler init errors can embed the Pimlico API key (it's
		// in the bundler URL), and this goes to logs/Sentry.
		log.error("failed to initialize acceptor wallet — refusing to start", {error: sanitizeError(err)})
		await flush()
		process.exit(1)
	}

	const app = createApp(config, acceptor)

	const server = Bun.serve({
		port: config.port,
		fetch: app,
	})

	log.info("membership-acceptor listening", {
		port: server.port,
		webhook_path: "/webhooks/geo",
		health_path: "/health",
	})

	// Graceful shutdown: stop accepting connections, flush telemetry, then exit.
	let shuttingDown = false
	const shutdown = async (signal: string) => {
		if (shuttingDown) return
		shuttingDown = true
		log.info("shutting down", {signal})
		await server.stop()
		await flush()
		process.exit(0)
	}

	process.on("SIGTERM", () => void shutdown("SIGTERM"))
	process.on("SIGINT", () => void shutdown("SIGINT"))
}

main()
